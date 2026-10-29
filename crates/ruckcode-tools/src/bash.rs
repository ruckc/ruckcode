use crate::{str_arg, truncate_middle, Tool, ToolContext, ToolKind, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_OUTPUT: usize = 30_000;

pub struct Bash;

#[async_trait]
impl Tool for Bash {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> String {
        "Run a shell command in the project root and return combined stdout/stderr and the exit code. \
         Prefer dedicated tools (Read, Grep, Glob, mise tasks) when they fit."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "description": {"type": "string", "description": "Short description of what the command does"},
                "timeout": {"type": "integer", "description": "Timeout in milliseconds (max 600000)"}
            },
            "required": ["command"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn title(&self, input: &Value) -> String {
        input["description"]
            .as_str()
            .or_else(|| input["command"].as_str())
            .unwrap_or("Bash")
            .to_string()
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let command = str_arg(&input, "command")?;
        let timeout = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        let r = run_shell(command, &ctx.cwd, Duration::from_millis(timeout), ctx).await?;
        Ok(r)
    }
}

/// Make a child unable to block on interactive input: no stdin, no controlling
/// TTY, and credential helpers told never to prompt.
fn harden(cmd: &mut tokio::process::Command) {
    cmd.stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .env("SSH_ASKPASS", "true")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat");
    #[cfg(unix)]
    unsafe {
        // New session: no controlling TTY (so /dev/tty prompts fail fast) and a
        // process group that can be killed as a whole.
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Kill the whole process tree rooted at `child`.
async fn kill_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // setsid made the child a group leader: pgid == pid.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
}

/// Run `sh -c command`, honoring cancellation and a timeout.
pub async fn run_shell(
    command: &str,
    cwd: &std::path::Path,
    timeout: Duration,
    ctx: &ToolContext,
) -> anyhow::Result<ToolOutput> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    harden(&mut cmd);
    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let read = async {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let _ = tokio::join!(stdout.read_to_end(&mut o), stderr.read_to_end(&mut e));
        (o, e)
    };
    let result = tokio::select! {
        r = async { let out = read.await; (out, child.wait().await) } => r,
        _ = tokio::time::sleep(timeout) => {
            kill_tree(&mut child).await;
            return Ok(ToolOutput::error(format!("command timed out after {}ms", timeout.as_millis())));
        }
        _ = ctx.cancel.cancelled() => {
            kill_tree(&mut child).await;
            return Ok(ToolOutput::error("command cancelled"));
        }
    };
    let ((o, e), status) = result;
    let status = status?;
    let mut text = String::from_utf8_lossy(&o).into_owned();
    if !e.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&e));
    }
    let text = truncate_middle(&text, MAX_OUTPUT);
    let code = status.code().unwrap_or(-1);
    if status.success() {
        Ok(ToolOutput::ok(if text.is_empty() {
            "(no output)".to_string()
        } else {
            text
        }))
    } else {
        Ok(ToolOutput::error(format!("{text}\n[exit code {code}]")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ctx;

    #[tokio::test]
    async fn runs_and_reports_exit() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let ok = Bash.call(json!({"command": "echo hi"}), &c).await.unwrap();
        assert_eq!(ok.content.trim(), "hi");
        let bad = Bash
            .call(json!({"command": "echo no >&2; exit 3"}), &c)
            .await
            .unwrap();
        assert!(bad.is_error);
        assert!(bad.content.contains("no") && bad.content.contains("exit code 3"));
    }

    #[tokio::test]
    async fn stdin_is_closed() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let t = std::time::Instant::now();
        let r = Bash
            .call(json!({"command": "read x; echo got=[$x]"}), &c)
            .await
            .unwrap();
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(r.content.contains("got=[]") || r.is_error);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn git_prompt_against_auth_remote_does_not_wait() {
        // A local HTTP server that always answers 401 makes git ask for credentials.
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let _ = s
                        .write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                });
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let t = std::time::Instant::now();
        let r = Bash
            .call(
                json!({"command": format!("export NO_PROXY=127.0.0.1 no_proxy=127.0.0.1; git -c credential.helper= ls-remote http://127.0.0.1:{port}/r.git 2>&1; echo done"), "timeout": 20000}),
                &c,
            )
            .await
            .unwrap();
        assert!(t.elapsed() < Duration::from_secs(15), "hung: {}", r.content);
        assert!(r.content.contains("done"), "{}", r.content);
        assert!(
            !r.content.to_lowercase().contains("username for"),
            "{}",
            r.content
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn no_controlling_tty() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let r = Bash
            .call(
                json!({"command": "(echo x > /dev/tty) 2>/dev/null && echo HASTTY || echo notty"}),
                &c,
            )
            .await
            .unwrap();
        assert!(r.content.contains("notty"), "{}", r.content);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_and_cancel_kill_the_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("alive");
        let cmd = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let c = ctx(dir.path());
        let r = Bash
            .call(json!({"command": cmd, "timeout": 200}), &c)
            .await
            .unwrap();
        assert!(r.content.contains("timed out"));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "background child survived the timeout");

        let c2 = ctx(dir.path());
        let cancel = c2.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        });
        let r = Bash.call(json!({"command": cmd}), &c2).await.unwrap();
        assert!(r.content.contains("cancelled"));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "background child survived cancel");
    }
}
