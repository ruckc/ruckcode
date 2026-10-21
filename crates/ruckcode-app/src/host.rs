//! Agent hosts: the backend side of the webview's single ACP transport.
//!
//! The webview speaks newline-delimited ACP JSON-RPC. Lines from the agent are pushed to it on a
//! Tauri `Channel`; lines to the agent come in through `agent_send`. The backend never inspects
//! them, so the host kind (embedded agent or `rc acp` child process) is invisible to the UI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use ruckcode_core::{Agent, AgentOptions};
use serde::Deserialize;
use tauri::async_runtime::JoinHandle;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Which agent the connection talks to.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HostKind {
    /// `ruckcode_core::Agent` embedded in this process behind `ruckcode_acp`.
    #[default]
    Local,
    /// `rc acp` as a child process.
    Subprocess,
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
    /// Open a connection. `forward` is called for every line the agent emits, in order.
    pub fn connect(
        &self,
        kind: HostKind,
        cwd: &Path,
        rc_path: Option<PathBuf>,
        forward: impl Fn(String) -> bool + Send + 'static,
    ) -> Result<u32, String> {
        if !cwd.is_dir() {
            return Err(format!("not a directory: {}", cwd.display()));
        }
        let (to_agent, from_agent, mut tasks) = match kind {
            HostKind::Local => {
                let agent = Arc::new(Agent::new(AgentOptions::default()));
                let (tx, rx) = ruckcode_acp::spawn_in_process_raw(agent);
                (tx, rx, Vec::new())
            }
            HostKind::Subprocess => spawn_subprocess(cwd, rc_path)?,
        };
        tasks.push(tauri::async_runtime::spawn(relay(from_agent, forward)));
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.conns
            .lock()
            .unwrap()
            .insert(id, Connection { to_agent, tasks });
        Ok(id)
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

async fn relay(mut rx: UnboundedReceiver<String>, forward: impl Fn(String) -> bool) {
    while let Some(line) = rx.recv().await {
        if !forward(line) {
            break; // webview went away
        }
    }
}

type Spawned = (
    UnboundedSender<String>,
    UnboundedReceiver<String>,
    Vec<JoinHandle<()>>,
);

fn spawn_subprocess(cwd: &Path, rc_path: Option<PathBuf>) -> Result<Spawned, String> {
    let rc = rc_path.unwrap_or_else(|| PathBuf::from("rc"));
    let mut child = tokio::process::Command::new(&rc)
        .arg("acp")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to start `{} acp`: {e}", rc.display()))?;
    let mut stdin = child.stdin.take().ok_or("child has no stdin")?;
    let stdout = child.stdout.take().ok_or("child has no stdout")?;

    let (to_agent, mut from_host) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (to_host, from_agent) = tokio::sync::mpsc::unbounded_channel::<String>();

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
    // Owns the child so aborting this task kills it.
    let waiter = tauri::async_runtime::spawn(async move {
        let _ = child.wait().await;
    });
    Ok((to_agent, from_agent, vec![writer, reader, waiter]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn rejects_missing_directory() {
        let hosts = Hosts::default();
        let err = hosts
            .connect(
                HostKind::Local,
                Path::new("/definitely/not/here"),
                None,
                |_| true,
            )
            .unwrap_err();
        assert!(err.contains("not a directory"));
    }

    #[test]
    fn local_host_relays_initialize() {
        let dir = std::env::temp_dir();
        let hosts = Hosts::default();
        let (tx, rx) = mpsc::channel();
        // The embedded agent spawns tokio tasks, so connect from inside the async runtime.
        let id = tauri::async_runtime::block_on(async {
            hosts.connect(HostKind::Local, &dir, None, move |l| tx.send(l).is_ok())
        })
        .unwrap();
        hosts
            .send(
                id,
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#
                    .into(),
            )
            .unwrap();
        let line = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["protocolVersion"], 1);
        hosts.disconnect(id);
        assert!(hosts.send(id, "{}".into()).is_err());
    }
}
