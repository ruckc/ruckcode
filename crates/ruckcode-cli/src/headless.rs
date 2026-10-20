//! `rc -p`: run one prompt non-interactively.

use std::io::{IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use ruckcode_acp::schema::{
    self, ContentBlock, RequestPermissionRequest, RequestPermissionResponse, SessionNotification,
    SessionUpdate, StopReason, ToolCallStatus,
};
use ruckcode_acp::{Handler, RpcError};
use serde_json::{json, Value};

use crate::client::{self, AskResponse};
use crate::{Cli, OutputFormat};

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
                let title = req.tool_call.title.clone().unwrap_or_default();
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
                Ok(serde_json::to_value(RequestPermissionResponse { outcome }).unwrap())
            }
            schema::ASK_USER_QUESTION => {
                Ok(serde_json::to_value(AskResponse::cancelled()).unwrap())
            }
            m => Err(RpcError::method_not_found(m)),
        }
    }

    async fn notification(&self, method: &str, params: Value) {
        if method != schema::SESSION_UPDATE || self.quiet.load(Ordering::Relaxed) {
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
            SessionUpdate::AgentMessageChunk {
                content: ContentBlock::Text { text },
            } => {
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
            SessionUpdate::ToolCallUpdate(u) if u.status == Some(ToolCallStatus::Failed) => {
                eprintln!("\x1b[2m✗ tool call {} failed\x1b[0m", u.tool_call_id);
            }
            _ => {}
        }
    }
}

fn read_prompt(cli: &Cli) -> Result<String> {
    let mut prompt = cli.prompt.clone().unwrap_or_default();
    if !std::io::stdin().is_terminal() {
        let mut piped = String::new();
        std::io::stdin().read_to_string(&mut piped).ok();
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
        json!({"sessionId": sid, "prompt": [ContentBlock::text(prompt)]}),
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
    let v = res
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("prompt failed")?;
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
