use tauri_build::{AppManifest, Attributes};

fn main() {
    // Generates `allow-<command>` permissions for our commands so capabilities can grant
    // exactly these and nothing else.
    tauri_build::try_build(
        Attributes::new().app_manifest(AppManifest::new().commands(&[
            "agent_connect",
            "agent_send",
            "agent_disconnect",
        ])),
    )
    .expect("failed to run tauri-build");
}
