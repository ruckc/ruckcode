// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The embedded agent sandboxes commands by re-executing this binary as
    // `<exe> __sandbox-exec ...`; that launcher must run before anything else.
    ruckcode_core::ruckcode_sandbox::launcher::maybe_run();
    ruckcode_app_lib::run();
}
