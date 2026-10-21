//! Agent hosts: the backend side of the webview's single ACP transport.
//!
//! The webview speaks newline-delimited ACP JSON-RPC. Lines from the agent are pushed to it on a
//! Tauri `Channel`; lines to the agent come in through `agent_send`. The backend never inspects
//! them, so the host kind (embedded agent or `rc acp` child process) is invisible to the UI.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ruckcode_core::{Agent, AgentOptions};
use serde::{Deserialize, Serialize};
use tauri::async_runtime::JoinHandle;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::wsl;

/// Which agent the connection talks to.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HostKind {
    /// `ruckcode_core::Agent` embedded in this process behind `ruckcode_acp`.
    #[default]
    Local,
    /// `rc acp` as a child process.
    Subprocess,
    /// `rc acp` inside a WSL2 distro, started through `wsl.exe`.
    Wsl,
}

/// What the webview receives on its channel: agent lines, then one final `exit`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum AgentEvent {
    Line {
        line: String,
    },
    /// The agent ended (process exit, task finished or panicked). Nothing follows.
    Exit {
        code: Option<i32>,
        message: String,
    },
}

/// Parameters of a connection.
#[derive(Debug, Clone, Default)]
pub struct ConnectParams {
    pub kind: HostKind,
    /// Working directory: a local path, or (for WSL) an already translated Linux path.
    pub cwd: String,
    pub rc_path: Option<String>,
    pub distro: Option<String>,
}

struct Connection {
    to_agent: UnboundedSender<String>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Aborting drops the child handle in the subprocess case (`kill_on_drop`).
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Live connections by id.
#[derive(Default)]
pub struct Hosts {
    next: AtomicU32,
    conns: Mutex<HashMap<u32, Connection>>,
}

impl Hosts {
    /// Open a connection. `forward` is called for every event in order; the last one is `Exit`
    /// (unless the connection is closed with [`Hosts::disconnect`] first).
    pub fn connect(
        &self,
        params: &ConnectParams,
        forward: impl Fn(AgentEvent) -> bool + Send + 'static,
    ) -> Result<u32, String> {
        let spawned = match params.kind {
            HostKind::Local => {
                check_dir(&params.cwd)?;
                let agent = Arc::new(Agent::new(AgentOptions::default()));
                let (tx, rx) = ruckcode_acp::spawn_in_process_raw(agent);
                Spawned {
                    to_agent: tx,
                    from_agent: rx,
                    tasks: Vec::new(),
                    exit: None,
                }
            }
            HostKind::Subprocess => {
                check_dir(&params.cwd)?;
                let rc = params.rc_path.clone().unwrap_or_else(|| "rc".into());
                let mut cmd = tokio::process::Command::new(&rc);
                cmd.arg("acp").current_dir(&params.cwd);
                wsl::hide_window(&mut cmd);
                spawn_command(cmd, format!("{rc} acp"))?
            }
            HostKind::Wsl => {
                let distro = params.distro.as_deref().ok_or("no WSL distro selected")?;
                if !cfg!(windows) {
                    return Err("WSL2 is only available on Windows".into());
                }
                spawn_wsl("wsl.exe", distro, &params.cwd, params.rc_path.as_deref())?
            }
        };
        Ok(self.register(spawned, forward))
    }

    fn register(
        &self,
        mut s: Spawned,
        forward: impl Fn(AgentEvent) -> bool + Send + 'static,
    ) -> u32 {
        let exit = s.exit.take();
        s.tasks.push(tauri::async_runtime::spawn(relay(
            s.from_agent,
            exit,
            forward,
        )));
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.conns.lock().unwrap().insert(
            id,
            Connection {
                to_agent: s.to_agent,
                tasks: s.tasks,
            },
        );
        id
    }

    pub fn send(&self, id: u32, line: String) -> Result<(), String> {
        let conns = self.conns.lock().unwrap();
        let conn = conns
            .get(&id)
            .ok_or_else(|| format!("no such connection: {id}"))?;
        conn.to_agent
            .send(line)
            .map_err(|_| "agent connection closed".to_string())
    }

    pub fn disconnect(&self, id: u32) {
        self.conns.lock().unwrap().remove(&id);
    }
}

fn check_dir(cwd: &str) -> Result<(), String> {
    if Path::new(cwd).is_dir() {
        Ok(())
    } else {
        Err(format!("not a directory: {cwd}"))
    }
}

/// How a child process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub message: String,
}

/// Human-readable exit message, including the tail of stderr when there is one.
pub fn exit_message(label: &str, code: Option<i32>, stderr_tail: &str) -> String {
    let how = match code {
        Some(0) => format!("`{label}` exited"),
        Some(c) => format!("`{label}` exited with code {c}"),
        None => format!("`{label}` was terminated"),
    };
    let tail = stderr_tail.trim();
    if tail.is_empty() {
        how
    } else {
        format!("{how}: {tail}")
    }
}

async fn relay(
    mut rx: UnboundedReceiver<String>,
    exit: Option<oneshot::Receiver<ExitInfo>>,
    forward: impl Fn(AgentEvent) -> bool,
) {
    while let Some(line) = rx.recv().await {
        if !forward(AgentEvent::Line { line }) {
            return; // webview went away
        }
    }
    // The agent's output ended: report how. A child that closed stdout but lives on is killed
    // when the connection is dropped, so only wait briefly for its status.
    let info = match exit {
        Some(rx) => match tokio::time::timeout(Duration::from_secs(3), rx).await {
            Ok(Ok(info)) => info,
            _ => ExitInfo {
                code: None,
                message: "The agent closed its output.".into(),
            },
        },
        None => ExitInfo {
            code: None,
            message: "The embedded agent stopped.".into(),
        },
    };
    forward(AgentEvent::Exit {
        code: info.code,
        message: info.message,
    });
}

struct Spawned {
    to_agent: UnboundedSender<String>,
    from_agent: UnboundedReceiver<String>,
    tasks: Vec<JoinHandle<()>>,
    exit: Option<oneshot::Receiver<ExitInfo>>,
}

const STDERR_LINES: usize = 20;
const STDERR_LINE_CHARS: usize = 400;

/// Spawn `wsl.exe -d <distro> --cd <cwd> -- sh -lc 'PATH=...; exec rc acp'` and relay its stdio.
/// `program` is a parameter so tests can substitute a stand-in for `wsl.exe`.
fn spawn_wsl(
    program: &str,
    distro: &str,
    linux_cwd: &str,
    rc: Option<&str>,
) -> Result<Spawned, String> {
    let mut cmd = wsl::wsl_command(program);
    cmd.args(wsl::agent_args(distro, linux_cwd, rc));
    spawn_command(cmd, format!("rc acp ({distro})"))
}

/// Spawn `cmd` as an ACP agent: stdin/stdout relayed as lines, stderr tail kept for the exit
/// message. The child is killed when the connection is dropped.
fn spawn_command(mut cmd: tokio::process::Command, label: String) -> Result<Spawned, String> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start `{label}`: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("child has no stdin")?;
    let stdout = child.stdout.take().ok_or("child has no stdout")?;
    let stderr = child.stderr.take().ok_or("child has no stderr")?;

    let (to_agent, mut from_host) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (to_host, from_agent) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (exit_tx, exit_rx) = oneshot::channel();
    let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();

    let writer = tauri::async_runtime::spawn(async move {
        while let Some(line) = from_host.recv().await {
            let mut buf = line.into_bytes();
            buf.push(b'\n');
            if stdin.write_all(&buf).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
    });
    let reader = tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() && to_host.send(line).is_err() {
                break;
            }
        }
    });
    let err_tail = tail.clone();
    let err_reader = tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line: String = line.chars().take(STDERR_LINE_CHARS).collect();
            let mut t = err_tail.lock().unwrap();
            if t.len() == STDERR_LINES {
                t.pop_front();
            }
            t.push_back(line);
        }
    });
    // Owns the child so aborting this task kills it.
    let waiter = tauri::async_runtime::spawn(async move {
        let status = child.wait().await;
        // Let stderr drain so its tail is complete.
        let _ = tokio::time::timeout(Duration::from_secs(1), err_reader).await;
        let tail = tail
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let code = status.ok().and_then(|s| s.code());
        let _ = exit_tx.send(ExitInfo {
            code,
            message: exit_message(&label, code, &tail),
        });
    });
    Ok(Spawned {
        to_agent,
        from_agent,
        tasks: vec![writer, reader, waiter],
        exit: Some(exit_rx),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn params(kind: HostKind, cwd: &str) -> ConnectParams {
        ConnectParams {
            kind,
            cwd: cwd.into(),
            ..Default::default()
        }
    }

    fn collect_until_exit(rx: &mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        loop {
            let e = rx.recv_timeout(Duration::from_secs(10)).expect("event");
            let done = matches!(e, AgentEvent::Exit { .. });
            out.push(e);
            if done {
                return out;
            }
        }
    }

    #[test]
    fn rejects_missing_directory() {
        let hosts = Hosts::default();
        let err = hosts
            .connect(&params(HostKind::Local, "/definitely/not/here"), |_| true)
            .unwrap_err();
        assert!(err.contains("not a directory"));
    }

    #[test]
    fn wsl_requires_a_distro() {
        let hosts = Hosts::default();
        let err = hosts
            .connect(&params(HostKind::Wsl, "/home/x"), |_| true)
            .unwrap_err();
        assert!(err.contains("distro"));
    }

    #[test]
    fn exit_messages() {
        assert_eq!(
            exit_message("rc acp", Some(3), "boom\n"),
            "`rc acp` exited with code 3: boom"
        );
        assert_eq!(exit_message("rc acp", Some(0), ""), "`rc acp` exited");
        assert_eq!(exit_message("rc acp", None, ""), "`rc acp` was terminated");
    }

    #[test]
    fn event_wire_format() {
        let line = serde_json::to_value(AgentEvent::Line { line: "x".into() }).unwrap();
        assert_eq!(line, serde_json::json!({"event": "line", "line": "x"}));
        let exit = serde_json::to_value(AgentEvent::Exit {
            code: Some(2),
            message: "m".into(),
        })
        .unwrap();
        assert_eq!(
            exit,
            serde_json::json!({"event": "exit", "code": 2, "message": "m"})
        );
    }

    #[test]
    fn local_host_relays_initialize() {
        let dir = std::env::temp_dir();
        let hosts = Hosts::default();
        let (tx, rx) = mpsc::channel();
        // The embedded agent spawns tokio tasks, so connect from inside the async runtime.
        let id = tauri::async_runtime::block_on(async {
            hosts.connect(&params(HostKind::Local, dir.to_str().unwrap()), move |e| {
                tx.send(e).is_ok()
            })
        })
        .unwrap();
        hosts
            .send(
                id,
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#
                    .into(),
            )
            .unwrap();
        let AgentEvent::Line { line } = rx.recv_timeout(Duration::from_secs(10)).unwrap() else {
            panic!("expected a line");
        };
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["protocolVersion"], 1);
        hosts.disconnect(id);
        assert!(hosts.send(id, "{}".into()).is_err());
    }

    #[cfg(unix)]
    fn sh_agent(script: &str) -> tokio::process::Command {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", script]);
        c
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_exit_reports_code_and_stderr_after_all_lines() {
        let hosts = Hosts::default();
        let (tx, rx) = mpsc::channel();
        tauri::async_runtime::block_on(async {
            let s = spawn_command(
                sh_agent(
                    r#"echo '{"a":1}'; echo '{"a":2}'; echo first >&2; echo oops >&2; exit 3"#,
                ),
                "rc acp".into(),
            )
            .unwrap();
            hosts.register(s, move |e| tx.send(e).is_ok());
        });
        let events = collect_until_exit(&rx);
        assert_eq!(
            events,
            vec![
                AgentEvent::Line {
                    line: r#"{"a":1}"#.into()
                },
                AgentEvent::Line {
                    line: r#"{"a":2}"#.into()
                },
                AgentEvent::Exit {
                    code: Some(3),
                    message: "`rc acp` exited with code 3: first\noops".into()
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_killed_by_signal_has_no_code() {
        let hosts = Hosts::default();
        let (tx, rx) = mpsc::channel();
        tauri::async_runtime::block_on(async {
            let s = spawn_command(sh_agent("kill -9 $$"), "rc acp".into()).unwrap();
            hosts.register(s, move |e| tx.send(e).is_ok());
        });
        let events = collect_until_exit(&rx);
        assert_eq!(
            events,
            vec![AgentEvent::Exit {
                code: None,
                message: "`rc acp` was terminated".into()
            }]
        );
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_round_trips_lines_and_disconnect_is_silent() {
        let hosts = Hosts::default();
        let (tx, rx) = mpsc::channel();
        let id = tauri::async_runtime::block_on(async {
            let s = spawn_command(sh_agent("cat"), "cat".into()).unwrap();
            hosts.register(s, move |e| tx.send(e).is_ok())
        });
        hosts.send(id, r#"{"hello":1}"#.into()).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            AgentEvent::Line {
                line: r#"{"hello":1}"#.into()
            }
        );
        hosts.disconnect(id);
        // An explicit disconnect is not an "agent exited" event.
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    }

    #[test]
    fn missing_binary_fails_to_spawn() {
        let r = tauri::async_runtime::block_on(async {
            spawn_command(
                tokio::process::Command::new("/definitely/not/a/binary"),
                "nope".into(),
            )
        });
        assert!(r.err().unwrap().contains("failed to start"));
    }

    /// Stand-in `wsl.exe` that understands `-d X --cd DIR -- sh -lc SCRIPT`: checks the argument
    /// shape, changes directory and runs the script with a fake `rc` on `PATH`.
    #[cfg(unix)]
    #[test]
    fn wsl_host_spawns_with_expected_args_and_reports_exit() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("rc-fake-wsl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, work) = (root.join("home"), root.join("work"));
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let rc = home.join(".local/bin/rc");
        std::fs::write(
            &rc,
            "#!/bin/sh\nread l\necho \"{\\\"cwd\\\":\\\"$(pwd)\\\",\\\"sub\\\":\\\"$1\\\"}\"\necho 'agent died' >&2\nexit 7\n",
        )
        .unwrap();
        std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o755)).unwrap();
        let wsl = root.join("wsl.exe");
        std::fs::write(
            &wsl,
            "#!/bin/sh\n[ \"$1\" = -d ] && [ \"$2\" = Ubuntu ] && [ \"$3\" = --cd ] && [ \"$5\" = --exec ] && [ \"$6\" = sh ] && [ \"$7\" = -lc ] || { echo \"bad args: $*\" >&2; exit 90; }\ncd \"$4\" || exit 91\nexport HOME=$FAKE_HOME\nexec sh -c \"$8\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&wsl, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("FAKE_HOME", &home);

        let hosts = Hosts::default();
        let (tx, rx) = mpsc::channel();
        let id = tauri::async_runtime::block_on(async {
            let s = spawn_wsl(
                wsl.to_str().unwrap(),
                "Ubuntu",
                work.to_str().unwrap(),
                None,
            )
            .unwrap();
            hosts.register(s, move |e| tx.send(e).is_ok())
        });
        hosts.send(id, r#"{"ping":1}"#.into()).unwrap();
        let events = collect_until_exit(&rx);
        let AgentEvent::Line { line } = &events[0] else {
            panic!("{events:?}")
        };
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["sub"], "acp");
        assert_eq!(
            std::fs::canonicalize(v["cwd"].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(&work).unwrap()
        );
        assert_eq!(
            events.last().unwrap(),
            &AgentEvent::Exit {
                code: Some(7),
                message: "`rc acp (Ubuntu)` exited with code 7: agent died".into()
            }
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
