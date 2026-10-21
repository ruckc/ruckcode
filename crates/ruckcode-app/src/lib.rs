//! ruckcode desktop app: a Tauri shell that hosts an ACP agent and relays its JSON-RPC lines to
//! the SvelteKit UI (the ACP client lives in TypeScript, see `app/`).

mod host;
mod wsl;

use host::{AgentEvent, ConnectParams, HostKind, Hosts};
use tauri::ipc::Channel;
use tauri::State;

/// Start an agent for `cwd` and stream its events to `on_event`: every ACP line, then a final
/// `exit` event when the agent ends. Returns the connection id.
///
/// For the `wsl` host, `cwd` may be a Windows or Linux path and is translated for `distro`.
#[tauri::command]
async fn agent_connect(
    hosts: State<'_, Hosts>,
    cwd: String,
    host: Option<HostKind>,
    rc_path: Option<String>,
    distro: Option<String>,
    on_event: Channel<AgentEvent>,
) -> Result<u32, String> {
    let kind = host.unwrap_or_default();
    let cwd = match (kind, distro.as_deref()) {
        (HostKind::Wsl, Some(d)) => wsl::resolve_linux_path(d, &cwd).await?,
        _ => cwd,
    };
    hosts.connect(
        &ConnectParams {
            kind,
            cwd,
            rc_path,
            distro,
        },
        move |event| on_event.send(event).is_ok(),
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

/// `windows`, `macos` or `linux`: lets the UI show the WSL2 host only where it can work.
#[tauri::command]
fn platform() -> &'static str {
    std::env::consts::OS
}

/// The app's own version, which `rc` inside WSL should match.
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
async fn wsl_list_distros() -> Result<Vec<wsl::Distro>, String> {
    wsl::list_distros().await
}

#[tauri::command]
async fn wsl_check_rc(distro: String) -> Result<wsl::RcStatus, String> {
    wsl::check_rc(&distro).await
}

/// Install the matching `rc` release into the distro; progress lines go to `on_progress`.
#[tauri::command]
async fn wsl_install_rc(
    distro: String,
    on_progress: Channel<String>,
) -> Result<wsl::RcStatus, String> {
    wsl::install_rc(&distro, move |l| {
        let _ = on_progress.send(l);
    })
    .await
}

/// Linux path for a Windows or Linux path as seen from `distro` (errors for other distros).
#[tauri::command]
async fn wsl_translate_path(distro: String, path: String) -> Result<String, String> {
    wsl::resolve_linux_path(&distro, &path).await
}

/// Windows path under which a Linux path of `distro` is visible (folder picker start).
#[tauri::command]
fn wsl_to_windows_path(distro: String, path: String) -> Result<String, String> {
    wsl::linux_to_windows(&path, &distro)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Hosts::default())
        .invoke_handler(tauri::generate_handler![
            agent_connect,
            agent_send,
            agent_disconnect,
            platform,
            app_version,
            wsl_list_distros,
            wsl_check_rc,
            wsl_install_rc,
            wsl_translate_path,
            wsl_to_windows_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running ruckcode app");
}
