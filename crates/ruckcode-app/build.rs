use tauri_build::{AppManifest, Attributes};

fn main() {
    // Generates `allow-<command>` permissions for our commands so capabilities can grant
    // exactly these and nothing else.
    tauri_build::try_build(
        Attributes::new().app_manifest(AppManifest::new().commands(&[
            "agent_connect",
            "agent_send",
            "agent_disconnect",
            "platform",
            "app_version",
            "wsl_list_distros",
            "wsl_check_rc",
            "wsl_install_rc",
            "wsl_translate_path",
            "wsl_to_windows_path",
            "setup_presets",
            "setup_state",
            "list_models",
            "setup_preview",
            "setup_apply",
            "keys_list",
            "keys_add",
            "keys_remove",
            "keys_use",
            "init_modes",
            "transcripts_list",
            "transcripts_read",
            "transcripts_search",
        ])),
    )
    .expect("failed to run tauri-build");
}
