//! Embeds the version reported by `rc --version` as `RC_VERSION`.
//!
//! Release builds set `RUCKCODE_VERSION` (e.g. `0.3.1`); local builds fall back
//! to `CARGO_PKG_VERSION-dev+<git short sha>` (or the bare package version when
//! git is unavailable).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=RUCKCODE_VERSION");
    println!("cargo:rerun-if-changed=../../.git/HEAD");

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
