//! `rc self-update` end to end against a local HTTP server: the real binary replaces a copy of
//! itself. Signature checking is covered by unit tests in `src/update.rs` (a test build has no
//! embedded public key, which is itself tested here).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

fn serve(files: Vec<(String, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split(' ').nth(1))
                .unwrap_or("/")
                .to_string();
            let (status, body) = match files.iter().find(|(p, _)| *p == path) {
                Some((_, b)) => ("200 OK", b.clone()),
                None => ("404 Not Found", Vec::new()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&body);
        }
    });
    base
}

fn tar_gz(name: &str, content: &[u8]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    let mut h = tar::Header::new_gnu();
    h.set_size(content.len() as u64);
    h.set_mode(0o755);
    h.set_cksum();
    b.append_data(&mut h, name, content).unwrap();
    b.into_inner().unwrap().finish().unwrap()
}

/// Run `cmd`, retrying while the freshly copied binary is "Text file busy": another test's
/// fork can briefly inherit our still-open write descriptor.
fn output(cmd: &mut Command) -> Output {
    for _ in 0..50 {
        match cmd.output() {
            Ok(o) => return o,
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(100))
            }
            Err(e) => panic!("{e}"),
        }
    }
    panic!("binary stayed busy");
}

fn rc(exe: &std::path::Path, base: &str, args: &[&str], home: &std::path::Path) -> Output {
    output(
        Command::new(exe)
            .args(args)
            .env("RUCKCODE_UPDATE_BASE_URL", base)
            .env("HOME", home)
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("RUCKCODE_CONFIG_DIR", home.join("cfg")),
    )
}

struct Env {
    dir: tempfile::TempDir,
    exe: std::path::PathBuf,
    base: String,
}

fn env(version: &str, target_ok: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join(if cfg!(windows) { "rc.exe" } else { "rc" });
    std::fs::copy(env!("CARGO_BIN_EXE_rc"), &exe).unwrap();
    let target = env!("TARGET_TRIPLE_FOR_TESTS");
    let archive = tar_gz("rc-9.9.9/rc", b"#!/bin/sh\necho replaced-by-update\n");
    let sha: String = Sha256::digest(&archive)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let key = if target_ok { target } else { "other-target" };
    let manifest = serde_json::json!({
        "version": version,
        "notes": "notes",
        "pub_date": "2026-01-01T00:00:00Z",
        "targets": { key: {
            "url": "download/v9.9.9/rc.tar.gz", "sha256": sha, "signature": "unused" } }
    });
    let base = serve(vec![
        (
            "/rc/latest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        ("/download/v9.9.9/rc.tar.gz".into(), archive),
    ]);
    Env { dir, exe, base }
}

#[test]
fn check_and_version_only() {
    let e = env("9.9.9", true);
    let o = rc(&e.exe, &e.base, &["self-update", "--check"], e.dir.path());
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success() && out.contains("9.9.9 is available"),
        "{out}"
    );
    let o = rc(
        &e.exe,
        &e.base,
        &["self-update", "--version-only"],
        e.dir.path(),
    );
    assert_eq!(String::from_utf8_lossy(&o.stdout), "9.9.9\n");
}

#[test]
fn unsigned_build_refuses_without_the_insecure_flag() {
    let e = env("9.9.9", true);
    let before = std::fs::read(&e.exe).unwrap();
    let o = rc(&e.exe, &e.base, &["self-update", "--yes"], e.dir.path());
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("no update signing key"), "{err}");
    assert_eq!(std::fs::read(&e.exe).unwrap(), before, "binary untouched");
}

#[cfg(unix)]
#[test]
fn replaces_the_running_binary() {
    let e = env("9.9.9", true);
    let o = rc(
        &e.exe,
        &e.base,
        &["self-update", "--yes", "--insecure-skip-signature"],
        e.dir.path(),
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(err.contains("WARNING"), "loud warning: {err}");
    let after = output(&mut Command::new(&e.exe));
    assert_eq!(
        String::from_utf8_lossy(&after.stdout).trim(),
        "replaced-by-update"
    );
    // Only the binary remains (no temp files).
    let names: Vec<_> = std::fs::read_dir(e.dir.path())
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.contains(".tmp")), "{names:?}");
}

#[test]
fn target_missing_from_manifest_is_reported() {
    let e = env("9.9.9", false);
    let o = rc(
        &e.exe,
        &e.base,
        &["self-update", "--yes", "--insecure-skip-signature"],
        e.dir.path(),
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("has no build for"));
}

#[test]
fn already_current_does_nothing() {
    let e = env("0.0.1", true);
    let o = rc(&e.exe, &e.base, &["self-update", "--yes"], e.dir.path());
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("up to date"));
}

#[test]
fn no_hint_on_stderr_when_not_a_terminal_or_disabled() {
    // `rc acp`/headless never print the hint without a TTY; here stderr is a pipe.
    let e = env("9.9.9", true);
    let o = output(
        Command::new(&e.exe)
            .args(["--version"])
            .env("RUCKCODE_UPDATE_BASE_URL", &e.base)
            .env("RUCKCODE_NO_UPDATE_CHECK", "1"),
    );
    assert!(o.stderr.is_empty());
}
