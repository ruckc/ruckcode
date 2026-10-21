use tauri_build::{AppManifest, Attributes};

fn main() {
    // The updater public key is compiled in when set (CI also merges it into tauri.conf.json).
    println!("cargo:rerun-if-env-changed=RUCKCODE_UPDATER_PUBKEY");
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
            "modes_get",
            "modes_validate",
            "modes_preview",
            "modes_apply",
            "update_check",
            "update_download",
            "update_install",
            "update_restart",
        ])),
    )
    .expect("failed to run tauri-build");
}
