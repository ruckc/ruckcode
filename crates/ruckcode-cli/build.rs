//! Embeds the version reported by `rc --version` as `RC_VERSION`.
//!
//! Release builds set `RUCKCODE_VERSION` (e.g. `0.3.1`); local builds fall back
//! to `CARGO_PKG_VERSION-dev+<git short sha>` (or the bare package version when
//! git is unavailable).
//!
//! It also embeds `RC_TARGET` (the compile-time target triple, used to pick the right archive in
//! `rc self-update`) and `RC_UPDATER_PUBKEY` from the `RUCKCODE_UPDATER_PUBKEY` environment
//! variable (the minisign public key of `tauri signer generate`; empty when not configured, in
//! which case `rc self-update` refuses to install anything).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=RUCKCODE_VERSION");
    println!("cargo:rerun-if-env-changed=RUCKCODE_UPDATER_PUBKEY");
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    let pubkey = std::env::var("RUCKCODE_UPDATER_PUBKEY").unwrap_or_default();
    // A multi-line value would break the `cargo:` directive; the key is single-line base64.
    let pubkey = pubkey.trim().replace(['\r', '\n'], "");
    println!("cargo:rustc-env=RC_UPDATER_PUBKEY={pubkey}");
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=RC_TARGET={target}");
    // Integration tests use the same triple to build a matching fake manifest.
    println!("cargo:rustc-env=TARGET_TRIPLE_FOR_TESTS={target}");

    let explicit = std::env::var("RUCKCODE_VERSION")
        .ok()
        .map(|v| v.trim().trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty());

    let version = explicit.unwrap_or_else(|| {
        let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
        let sha = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        match sha {
            Some(sha) => format!("{pkg}-dev+{sha}"),
            None => pkg,
        }
    });
    println!("cargo:rustc-env=RC_VERSION={version}");
}
