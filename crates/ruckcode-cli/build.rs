//! The version reported by `rc --version` is `ruckcode_agent::identity::VERSION` (the agent's
//! build.rs embeds it). This script embeds `RC_TARGET` (the compile-time target triple, used to pick the right archive in
//! `rc self-update`) and `RC_UPDATER_PUBKEY` from the `RUCKCODE_UPDATER_PUBKEY` environment
//! variable (the minisign public key of `tauri signer generate`; empty when not configured, in
//! which case `rc self-update` refuses to install anything).

fn main() {
    println!("cargo:rerun-if-env-changed=RUCKCODE_UPDATER_PUBKEY");

    let pubkey = std::env::var("RUCKCODE_UPDATER_PUBKEY").unwrap_or_default();
    // A multi-line value would break the `cargo:` directive; the key is single-line base64.
    let pubkey = pubkey.trim().replace(['\r', '\n'], "");
    println!("cargo:rustc-env=RC_UPDATER_PUBKEY={pubkey}");
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=RC_TARGET={target}");
    // Integration tests use the same triple to build a matching fake manifest.
    println!("cargo:rustc-env=TARGET_TRIPLE_FOR_TESTS={target}");
}
