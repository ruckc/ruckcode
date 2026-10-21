//! ruckcode desktop app: a Tauri shell that hosts an ACP agent and relays its JSON-RPC lines to
//! the SvelteKit UI (the ACP client lives in TypeScript, see `app/`).

mod host;
mod modes;
mod setup;
mod transcripts;
mod wsl;

use host::{AgentEvent, ConnectParams, HostKind, Hosts};
use ruckcode_core::setup_api::{InitInput, ModelsAuth, PlanInput};
use serde_json::Value;
use setup::Target;
use tauri::ipc::Channel;
use tauri::State;
use zeroize::Zeroizing;

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
    // Release builds set RUCKCODE_VERSION (same as `rc`), so the reported version matches the
    // bundle version even though Cargo.toml is never bumped.
    match option_env!("RUCKCODE_VERSION") {
        Some(v) if !v.is_empty() => v,
        _ => env!("CARGO_PKG_VERSION"),
    }
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

// ---- setup and keys -------------------------------------------------------------------------
//
// `project` is a local path or (with `distro`) a Linux path inside that WSL2 distro, where the same
// features then run through `rc ... --json`. Secrets are accepted only by `list_models`
// (`auth.secret`), `setup_preview`/`setup_apply` (`plan.newKey.secret`) and `keys_add`; they are
// zeroized when dropped and are never logged or returned.

#[tauri::command]
fn setup_presets() -> Result<Value, String> {
    setup::presets()
}

#[tauri::command]
async fn setup_state(project: String, distro: Option<String>) -> Result<Value, String> {
    setup::state(&Target::from_distro(distro), &project).await
}

#[tauri::command]
async fn list_models(
    project: String,
    distro: Option<String>,
    kind: String,
    base_url: String,
    auth: Option<ModelsAuth>,
) -> Result<Value, String> {
    setup::list_models(
        &Target::from_distro(distro),
        &project,
        &kind,
        &base_url,
        auth,
    )
    .await
}

#[tauri::command]
async fn setup_preview(
    project: String,
    distro: Option<String>,
    plan: PlanInput,
) -> Result<Value, String> {
    setup::setup_plan(&Target::from_distro(distro), &project, plan, true).await
}

#[tauri::command]
async fn setup_apply(
    project: String,
    distro: Option<String>,
    plan: PlanInput,
) -> Result<Value, String> {
    setup::setup_plan(&Target::from_distro(distro), &project, plan, false).await
}

#[tauri::command]
async fn keys_list(project: String, distro: Option<String>) -> Result<Value, String> {
    setup::keys_list(&Target::from_distro(distro), &project).await
}

#[tauri::command]
async fn keys_add(
    project: String,
    distro: Option<String>,
    name: String,
    secret: String,
    description: Option<String>,
) -> Result<Value, String> {
    setup::keys_add(
        &Target::from_distro(distro),
        &project,
        &name,
        Zeroizing::new(secret),
        description.as_deref(),
    )
    .await
}

#[tauri::command]
async fn keys_remove(
    project: String,
    distro: Option<String>,
    name: String,
) -> Result<Value, String> {
    setup::keys_remove(&Target::from_distro(distro), &project, &name).await
}

#[tauri::command]
async fn keys_use(
    project: String,
    distro: Option<String>,
    name: String,
    profile: Option<String>,
    project_scope: bool,
) -> Result<Value, String> {
    setup::keys_use(
        &Target::from_distro(distro),
        &project,
        &name,
        profile.as_deref(),
        project_scope,
    )
    .await
}

#[tauri::command]
async fn init_modes(
    project: String,
    distro: Option<String>,
    opts: Option<InitInput>,
) -> Result<Value, String> {
    setup::init_modes(
        &Target::from_distro(distro),
        &project,
        opts.unwrap_or_default(),
    )
    .await
}

// ---- transcript browser -----------------------------------------------------------------------
//
// Read-only. `project` is a local path or (with `distro`) a Linux path inside that WSL2 distro,
// where `rc sessions ... --json` produces the same JSON as the local core.

#[tauri::command]
async fn transcripts_list(
    project: String,
    distro: Option<String>,
    query: Option<String>,
) -> Result<Value, String> {
    transcripts::list(&Target::from_distro(distro), &project, query.as_deref()).await
}

#[tauri::command]
async fn transcripts_read(
    project: String,
    distro: Option<String>,
    id: String,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Value, String> {
    transcripts::read(&Target::from_distro(distro), &project, &id, offset, limit).await
}

#[tauri::command]
async fn transcripts_search(
    project: String,
    distro: Option<String>,
    text: String,
    limit: Option<usize>,
) -> Result<Value, String> {
    transcripts::search(&Target::from_distro(distro), &project, &text, limit).await
}

// ---- mode/hook editor ---------------------------------------------------------------------------
//
// `doc` is the camelCase modes document of `ruckcode_core::modes_api`; saving keeps comments.

#[tauri::command]
async fn modes_get(project: String, distro: Option<String>) -> Result<Value, String> {
    modes::get(&Target::from_distro(distro), &project).await
}

#[tauri::command]
async fn modes_validate(
    project: String,
    distro: Option<String>,
    doc: Value,
) -> Result<Value, String> {
    modes::validate(&Target::from_distro(distro), &project, &doc).await
}

#[tauri::command]
async fn modes_preview(
    project: String,
    distro: Option<String>,
    doc: Value,
) -> Result<Value, String> {
    modes::preview(&Target::from_distro(distro), &project, &doc).await
}

#[tauri::command]
async fn modes_apply(project: String, distro: Option<String>, doc: Value) -> Result<Value, String> {
    modes::apply(&Target::from_distro(distro), &project, &doc).await
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
            wsl_to_windows_path,
            setup_presets,
            setup_state,
            list_models,
            setup_preview,
            setup_apply,
            keys_list,
            keys_add,
            keys_remove,
            keys_use,
            init_modes,
            transcripts_list,
            transcripts_read,
            transcripts_search,
            modes_get,
            modes_validate,
            modes_preview,
            modes_apply
        ])
        .run(tauri::generate_context!())
        .expect("error while running ruckcode app");
}
