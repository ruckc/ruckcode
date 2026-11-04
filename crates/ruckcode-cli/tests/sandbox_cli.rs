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

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_in(dir: &Path, args: &[&str]) {
    let o = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// The project committed to a local bare "remote" whose policy sets `network = "off"`.
fn project_with_remote(e: &Env) -> PathBuf {
    let remote = e.outside.join("remote.git");
    git_in(
        &e.outside,
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    git_in(&e.proj, &["init", "-q", "-b", "main"]);
    std::fs::write(
        e.proj.join(".ruckcode/config.toml"),
        format!("{CFG}[sandbox]\nnetwork = \"off\"\n"),
    )
    .unwrap();
    git_in(&e.proj, &["add", "."]);
    git_in(&e.proj, &["commit", "-q", "-m", "init"]);
    git_in(
        &e.proj,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git_in(&e.proj, &["push", "-q", "origin", "main"]);
    remote
}

fn status_json(e: &Env) -> serde_json::Value {
    let o = rc(e, &["sandbox", "status", "--json"]);
    serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", text(&o)))
}

#[test]
fn status_shows_the_verified_project_policy_and_drift() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let e = env();
    let remote = project_with_remote(&e);
    let v = status_json(&e);
    assert_eq!(v["project_policy"]["status"]["state"], "active", "{v}");
    assert_eq!(v["config"]["network"], "off");
    assert_eq!(
        v["project_policy"]["source"]["url"],
        remote.to_str().unwrap()
    );
    assert_eq!(v["project_policy"]["source"]["branch"], "main");
    assert!(v["project_policy"]["drift"].is_null());
    // Loosening the working tree does not take effect and is reported.
    std::fs::write(
        e.proj.join(".ruckcode/config.toml"),
        format!("{CFG}[sandbox]\nnetwork = \"full\"\n"),
    )
    .unwrap();
    let v = status_json(&e);
    assert_eq!(v["config"]["network"], "off");
    assert!(v["project_policy"]["drift"]
        .as_str()
        .unwrap()
        .contains("until merged"));
    let t = text(&rc(&e, &["sandbox", "status"]));
    assert!(t.contains("project policy: active; source:"), "{t}");
    assert!(t.contains("warning:") && t.contains("until merged"), "{t}");
}

#[test]
fn changed_origin_is_ignored_until_the_user_runs_trust_remote() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let e = env();
    let _ = project_with_remote(&e);
    assert_eq!(status_json(&e)["config"]["network"], "off");
    let other = e.outside.join("other.git");
    git_in(
        &e.outside,
        &["init", "-q", "--bare", "-b", "main", "other.git"],
    );
    git_in(
        &e.proj,
        &["remote", "set-url", "origin", other.to_str().unwrap()],
    );
    let v = status_json(&e);
    assert_eq!(
        v["project_policy"]["status"]["state"], "pin_mismatch",
        "{v}"
    );
    assert_eq!(v["config"]["network"], "allowlist");
    let t = text(&rc(&e, &["sandbox", "status"]));
    assert!(t.contains("IGNORED") && t.contains("trust-remote"), "{t}");
    // Needs an explicit yes when there is no terminal.
    let o = rc(&e, &["sandbox", "trust-remote"]);
    assert!(!o.status.success(), "{}", text(&o));
    let o = rc(&e, &["sandbox", "trust-remote", "--yes"]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        status_json(&e)["project_policy"]["pinned_url"],
        other.to_str().unwrap()
    );
}

#[test]
fn the_model_cannot_re_pin_the_remote() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let e = env();
    if !enforcing(&e) {
        return;
    }
    let remote = project_with_remote(&e);
    let other = e.outside.join("other.git");
    git_in(
        &e.outside,
        &["init", "-q", "--bare", "-b", "main", "other.git"],
    );
    // The model changes origin (blocked, or reverted when only Landlock is available) and then
    // tries the user's command from inside the sandbox.
    let out = bash(
        &e,
        &format!(
            "git remote set-url origin {} ; {} sandbox trust-remote --yes",
            other.display(),
            env!("CARGO_BIN_EXE_rc")
        ),
    );
    let v = status_json(&e);
    assert_eq!(
        v["project_policy"]["pinned_url"],
        remote.to_str().unwrap(),
        "{out}"
    );
    assert_eq!(
        v["project_policy"]["status"]["state"], "active",
        "{out} {v}"
    );
    assert_eq!(v["config"]["network"], "off");
}

#[test]
fn write_tool_refuses_git_config_hooks_and_info() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let e = env();
    if !enforcing(&e) {
        return;
    }
    git_in(&e.proj, &["init", "-q", "-b", "main"]);
    let cfg = std::fs::read_to_string(e.proj.join(".git/config")).unwrap();
    for target in [".git/config", ".git/hooks/post-commit", ".git/info/exclude"] {
        let out = text(&rc(
            &e,
            &["-p", &format!("mock:write {target} evil"), "--yolo"],
        ));
        assert!(
            out.contains("protected git state"),
            "{target} must be refused: {out}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(e.proj.join(".git/config")).unwrap(),
        cfg
    );
    assert!(!e.proj.join(".git/hooks/post-commit").exists());
    // Ordinary files are still writable.
    let out = text(&rc(&e, &["-p", "mock:write notes.txt hi", "--yolo"]));
    assert!(e.proj.join("notes.txt").exists(), "{out}");
}
