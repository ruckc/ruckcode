//! The sandbox through the real `rc` binary (self re-exec launcher), driven by the mock model.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CFG: &str =
    "default_profile = \"demo\"\n[profiles.demo]\nprovider = \"mock\"\nmodel = \"mock\"\n";

struct Env {
    _base: tempfile::TempDir,
    proj: PathBuf,
    outside: PathBuf,
    home: PathBuf,
    cfg: PathBuf,
}

fn env() -> Env {
    // Next to the test binary: not under /tmp, which sandboxed commands may write.
    let exe = std::env::current_exe().unwrap();
    let base = tempfile::tempdir_in(exe.parent().unwrap()).unwrap();
    let b = base.path().canonicalize().unwrap();
    let (proj, outside, home, cfg) = (
        b.join("proj"),
        b.join("outside"),
        b.join("home"),
        b.join("cfg"),
    );
    for d in [&proj, &outside, &home, &cfg, &proj.join(".ruckcode")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(proj.join(".ruckcode/config.toml"), CFG).unwrap();
    Env {
        _base: base,
        proj,
        outside,
        home,
        cfg,
    }
}

fn rc(e: &Env, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rc"))
        .args(args)
        .current_dir(&e.proj)
        .env("HOME", &e.home)
        .env("XDG_CONFIG_HOME", e.home.join("config"))
        .env("XDG_DATA_HOME", e.home.join("data"))
        .env("RUCKCODE_CONFIG_DIR", &e.cfg)
        .env_remove("RUCKCODE_PROFILE")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

fn enforcing(e: &Env) -> bool {
    let o = rc(e, &["sandbox", "status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let l = &v["plan"]["layers"];
    let ok = l["landlock"] == true || l["namespaces"] == true || l["seatbelt"] == true;
    if !ok {
        eprintln!("SKIPPED: no sandbox layer available here: {v}");
    }
    ok
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn bash(e: &Env, cmd: &str) -> String {
    text(&rc(e, &["-p", &format!("mock:bash {cmd}"), "--yolo"]))
}

#[test]
fn status_reports_layers() {
    let e = env();
    let o = rc(&e, &["sandbox", "status"]);
    assert!(o.status.success(), "{}", text(&o));
    let t = text(&o);
    assert!(
        t.contains("active layers:") && t.contains("platform:"),
        "{t}"
    );
}

#[test]
fn bash_is_confined_through_the_launcher() {
    let e = env();
    if !enforcing(&e) {
        return;
    }
    let out = bash(&e, &format!("echo evil > {}/x.txt", e.outside.display()));
    assert!(!e.outside.join("x.txt").exists(), "{out}");
    assert!(
        out.contains("[sandbox]"),
        "blocked output must explain itself: {out}"
    );
    let out = bash(&e, "echo fine > inner.txt");
    assert!(e.proj.join("inner.txt").exists(), "{out}");
}

#[test]
fn ruckcode_config_cannot_be_changed_by_tools() {
    let e = env();
    if !enforcing(&e) {
        return;
    }
    let cfg_path = e.proj.join(".ruckcode/config.toml");
    let _ = bash(
        &e,
        "printf '[sandbox]\\nenabled = false\\n' > .ruckcode/config.toml",
    );
    assert_eq!(std::fs::read_to_string(&cfg_path).unwrap(), CFG);
    let out = text(&rc(
        &e,
        &["-p", "mock:write .ruckcode/modes.toml evil", "--yolo"],
    ));
    assert!(!e.proj.join(".ruckcode/modes.toml").exists(), "{out}");
    assert!(out.contains("ruckcode's own configuration"), "{out}");
}

#[test]
fn only_the_user_config_can_turn_the_sandbox_off() {
    let e = env();
    if !enforcing(&e) {
        return;
    }
    let target = format!("echo x > {}/off.txt", e.outside.display());
    // A project config cannot disable it.
    std::fs::write(
        e.proj.join(".ruckcode/config.toml"),
        format!("{CFG}[sandbox]\nenabled = false\n"),
    )
    .unwrap();
    let _ = bash(&e, &target);
    assert!(!e.outside.join("off.txt").exists());
    // The user config can.
    std::fs::write(e.cfg.join("config.toml"), "[sandbox]\nenabled = false\n").unwrap();
    let _ = bash(&e, &target);
    assert!(e.outside.join("off.txt").exists());
}

#[allow(dead_code)]
fn _p(_: &Path) {}
