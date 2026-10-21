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
        ])),
    )
    .expect("failed to run tauri-build");
}
