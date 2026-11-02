//! `rc -p`: run one prompt non-interactively.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use ruckcode_acp::schema::{
    self, ContentBlock, ContentChunk, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, SessionUpdate, StopReason, TextContent, ToolCallStatus,
};
use ruckcode_acp::{Handler, RpcError};
use serde_json::{json, Value};

use crate::client::{self, AskResponse};
use crate::{Cli, OutputFormat};

/// Exit code of `rc -p` when a rate limit ended the turn (`EX_TEMPFAIL`).
pub const EXIT_RATE_LIMITED: u8 = 75;

pub struct Policy {
    pub allow: Vec<String>,
    pub yolo: bool,
}

impl Policy {
    /// Tool name is the first word of the call title (`Bash: ls` -> `Bash`).
    pub fn allows(&self, title: &str) -> bool {
        if self.yolo {
            return true;
        }
        let name: String = title
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        self.allow.iter().any(|a| a.eq_ignore_ascii_case(&name))
    }
}

struct HeadlessHandler {
    policy: Policy,
    format: OutputFormat,
    quiet: AtomicBool,
    wrote_text: AtomicBool,
    ends_newline: AtomicBool,
}

#[async_trait]
impl Handler for HeadlessHandler {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            schema::SESSION_REQUEST_PERMISSION => {
                let req: RequestPermissionRequest = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                let title = req.tool_call.fields.title.clone().unwrap_or_default();
                let allow = self.policy.allows(&title);
                eprintln!(
                    "\x1b[2m{} {}\x1b[0m",
                    if allow {
                        "permission granted:"
                    } else {
                        "permission denied:"
                    },
                    title
                );
                let outcome = client::pick_option(&req, allow);
                Ok(serde_json::to_value(RequestPermissionResponse::new(outcome)).unwrap())
            }
            schema::ASK_USER_QUESTION => {
                Ok(serde_json::to_value(AskResponse::cancelled()).unwrap())
            }
            m => Err(RpcError::method_not_found(m)),
        }
    }

    async fn notification(&self, method: &str, params: Value) {
        if self.quiet.load(Ordering::Relaxed) {
            return;
        }
        if method == schema::RATE_LIMIT {
            // Same envelope as `session/update` so stream-json consumers read one stream.
            if self.format == OutputFormat::StreamJson {
                println!("{}", rate_limit_stream_line(&params));
            } else if let Some(line) = rate_limit_text(&params) {
                eprintln!("\x1b[2mrc: {line}\x1b[0m");
            }
            return;
        }
        if method != schema::SESSION_UPDATE {
            return;
        }
        if self.format == OutputFormat::StreamJson {
            println!("{params}");
            return;
        }
        let Ok(n) = serde_json::from_value::<SessionNotification>(params) else {
            return;
        };
        match n.update {
            SessionUpdate::AgentMessageChunk(ContentChunk {
                content: ContentBlock::Text(TextContent { text, .. }),
                ..
            }) => {
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(text.as_bytes());
                let _ = out.flush();
                if !text.is_empty() {
                    self.wrote_text.store(true, Ordering::Relaxed);
                    self.ends_newline
                        .store(text.ends_with('\n'), Ordering::Relaxed);
                }
            }
            SessionUpdate::ToolCall(tc) => {
                eprintln!("\x1b[2m● {}\x1b[0m", tc.title);
            }
            SessionUpdate::ToolCallUpdate(u) if u.fields.status == Some(ToolCallStatus::Failed) => {
                eprintln!("\x1b[2m✗ tool call {} failed\x1b[0m", u.tool_call_id);
            }
            SessionUpdate::Notice(n) => {
                eprintln!("\x1b[2m{}\x1b[0m", n.title);
            }
            _ => {}
        }
    }
}

/// stream-json line for a `_ruckcode/rate_limit` notification:
/// `{"sessionId", "update": {"sessionUpdate": "rate_limit", "status": "waiting"|"resumed"|"stopped", ...}}`.
fn rate_limit_stream_line(params: &Value) -> Value {
    let mut update = params.clone();
    let session_id = update
        .as_object_mut()
        .and_then(|o| o.remove("sessionId"))
        .unwrap_or(Value::Null);
    if let Some(o) = update.as_object_mut() {
        o.insert("sessionUpdate".into(), json!("rate_limit"));
    }
    json!({"sessionId": session_id, "update": update})
}

/// One human line for stderr: "Rate limited (tokens/minute) - retrying in 51s (retry 1/6)".
fn rate_limit_text(params: &Value) -> Option<String> {
    if params["status"] != "waiting" {
        return None;
    }
    let secs = params["waitMs"].as_u64().unwrap_or(0).div_ceil(1000);
    let retry = match params["attempt"].as_u64() {
        Some(n) if n > 0 => format!(" (retry {n}/{})", params["maxAttempts"]),
        _ => String::new(),
    };
    Some(format!(
        "{} - retrying in {}{retry}",
        params["reason"].as_str().unwrap_or("Rate limited"),
        ruckcode_core::retry::format_wait(std::time::Duration::from_secs(secs))
    ))
}

const STDIN_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// Read all of stdin. With `first_data_timeout`, return empty if neither data nor EOF
/// arrives within that time; once data starts flowing it is read to the end.
fn read_piped_stdin(first_data_timeout: Option<std::time::Duration>) -> String {
    use std::io::Read as _;
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 8192];
        while let Ok(n @ 1..) = stdin.read(&mut buf) {
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut bytes = match first_data_timeout {
        Some(t) => match rx.recv_timeout(t) {
            Ok(first) => first,
            Err(_) => return String::new(),
        },
        None => Vec::new(),
    };
    bytes.extend(rx.iter().flatten());
    String::from_utf8_lossy(&bytes).into_owned()
}

fn read_prompt(cli: &Cli) -> Result<String> {
    let mut prompt = cli.prompt.clone().unwrap_or_default();
    if !std::io::stdin().is_terminal() {
        // With a prompt argument, stdin is optional extra context: a parent process may
        // leave it open without ever writing (CI, subprocesses), so give up if no data
        // arrives promptly instead of blocking forever.
        let wait = (!prompt.trim().is_empty()).then_some(STDIN_GRACE);
        let piped = read_piped_stdin(wait);
        if !piped.trim().is_empty() {
            prompt = if prompt.is_empty() {
                piped
            } else {
                format!("{prompt}\n\n{piped}")
            };
        }
    }
    if prompt.trim().is_empty() {
        bail!("--print needs a prompt (argument or stdin)");
    }
    Ok(prompt)
}

pub async fn run(cli: &Cli) -> Result<u8> {
    let prompt = read_prompt(cli)?;
    let cwd = cli.project_dir()?;
    let handler = Arc::new(HeadlessHandler {
        policy: Policy {
            allow: cli.allow.clone(),
            yolo: cli.yolo,
        },
        format: cli.output_format,
        quiet: AtomicBool::new(true),
        wrote_text: AtomicBool::new(false),
        ends_newline: AtomicBool::new(true),
    });
    let conn = crate::connect(cli, handler.clone()).await?;
    // Once-a-day update hint: shown on stderr only when it is a terminal, never delays the run.
    let update_plan = crate::update::startup_plan(crate::tui::update_checks_enabled(&cwd));
    let update_refresh = update_plan
        .refresh
        .then(|| tokio::spawn(crate::update::refresh_cache()));

    let started = if let Some(id) = cli.resume.as_deref().filter(|s| !s.is_empty()) {
        client::load_session(&conn, &cwd, id).await?
    } else if cli.resume.is_some() {
        bail!("--resume needs a session id in --print mode");
    } else if cli.continue_last {
        let sessions = client::list_sessions(&conn, &cwd).await?;
        let Some(s) = sessions.first() else {
            bail!("no previous session to continue");
        };
        client::load_session(&conn, &cwd, &s.id).await?
    } else {
        client::new_session(&conn, &cwd).await?
    };
    if let Some(mode) = &cli.mode {
        client::set_mode(&conn, &started.session_id, mode).await?;
    }
    // Replayed history (resume) is not part of this run's output.
    handler.quiet.store(false, Ordering::Relaxed);

    let sid = started.session_id.clone();
    let turn = conn.request(
        schema::SESSION_PROMPT,
        json!({"sessionId": sid, "prompt": [ContentBlock::from(prompt)]}),
    );
    tokio::pin!(turn);
    let res = tokio::select! {
        r = &mut turn => r,
        _ = tokio::signal::ctrl_c() => {
            let _ = conn.notify(schema::SESSION_CANCEL, json!({"sessionId": started.session_id}));
            (&mut turn).await
        }
    };
    if cli.output_format == OutputFormat::Text
        && handler.wrote_text.load(Ordering::Relaxed)
        && !handler.ends_newline.load(Ordering::Relaxed)
    {
        println!();
    }
    if std::io::stderr().is_terminal() {
        let mut newer = update_plan.cached;
        if let Some(task) = update_refresh {
            if let Ok(Ok(Some(v))) =
                tokio::time::timeout(std::time::Duration::from_millis(500), task).await
            {
                newer = Some(v);
            }
        }
        if let Some(v) = newer {
            eprintln!("rc: {}", crate::update::hint_text(&v));
        }
    }
    let v = res
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("prompt failed")?;
    if v["_meta"]["ruckcode"]["stopReason"] == "rate_limited" {
        // The explanation (budget, reset time) was printed as a notice. 75 = EX_TEMPFAIL:
        // retry later, which is exactly what the session allows (`--continue`).
        eprintln!("rc: stopped: rate limited; continue the session after the limit resets");
        return Ok(EXIT_RATE_LIMITED);
    }
    let stop: StopReason = serde_json::from_value(v.get("stopReason").cloned().unwrap_or_default())
        .context("bad prompt response")?;
    Ok(match stop {
        StopReason::EndTurn => 0,
        StopReason::Cancelled => 130,
        other => {
            eprintln!("rc: stopped: {other:?}");
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_matches_tool_name() {
        let p = Policy {
            allow: vec!["Bash".into(), "edit".into()],
            yolo: false,
        };
        assert!(p.allows("Bash: ls -la"));
        assert!(p.allows("Edit src/a.rs"));
        assert!(!p.allows("Write src/a.rs"));
        assert!(!p.allows("Bashful"));
        assert!(Policy {
            allow: vec![],
            yolo: true
        }
        .allows("anything"));
    }
}
