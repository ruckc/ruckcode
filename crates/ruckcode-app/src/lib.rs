//! ruckcode desktop app: a Tauri shell that hosts an ACP agent and relays its JSON-RPC lines to
//! the SvelteKit UI (the ACP client lives in TypeScript, see `app/`).

mod host;

use std::path::PathBuf;

use host::{HostKind, Hosts};
use tauri::ipc::Channel;
use tauri::State;

/// Start an agent for `cwd` and stream its lines to `on_line`. Returns the connection id.
#[tauri::command]
async fn agent_connect(
    hosts: State<'_, Hosts>,
    cwd: String,
    host: Option<HostKind>,
    rc_path: Option<String>,
    on_line: Channel<String>,
) -> Result<u32, String> {
    hosts.connect(
        host.unwrap_or_default(),
        &PathBuf::from(cwd),
        rc_path.map(PathBuf::from),
        move |line| on_line.send(line).is_ok(),
    )
}

/// Deliver one ACP JSON-RPC line (no trailing newline) to the agent.
#[tauri::command]
fn agent_send(hosts: State<'_, Hosts>, id: u32, line: String) -> Result<(), String> {
    hosts.send(id, line)
}

#[tauri::command]
fn agent_disconnect(hosts: State<'_, Hosts>, id: u32) {
    hosts.disconnect(id);
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Hosts::default())
        .invoke_handler(tauri::generate_handler![
            agent_connect,
            agent_send,
            agent_disconnect
        ])
        .run(tauri::generate_context!())
        .expect("error while running ruckcode app");
}
