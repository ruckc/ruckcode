//! Tests use a local bare repository as the "remote" and a clone in a temp dir.

use super::*;
use crate::modes::ModeSet;
use ruckcode_sandbox::NetworkMode;

fn git_in(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .output()
        .expect("git");
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

struct Fx {
    _base: tempfile::TempDir,
    remote: PathBuf,
    clone: PathBuf,
    data: PathBuf,
}

const POLICY: &str = "[sandbox]\nnetwork = \"off\"\nextra_write_paths = [\"/data\"]\n";

/// A bare remote whose default branch `trunk` has a committed policy, plus a clone of it.
fn fixture(config: &str, modes: Option<&str>) -> Fx {
    let base = tempfile::tempdir().unwrap();
    let b = base.path().canonicalize().unwrap();
    let remote = b.join("remote.git");
    let seed = b.join("seed");
    let clone = b.join("clone");
    let data = b.join("data");
    std::fs::create_dir_all(&seed).unwrap();
    git_in(&b, &["init", "-q", "--bare", "-b", "trunk", "remote.git"]);
    git_in(&b, &["init", "-q", "-b", "trunk", "seed"]);
    std::fs::create_dir_all(seed.join(".ruckcode")).unwrap();
    std::fs::write(seed.join(".ruckcode/config.toml"), config).unwrap();
    if let Some(m) = modes {
        std::fs::write(seed.join(".ruckcode/modes.toml"), m).unwrap();
    }
    git_in(&seed, &["add", "."]);
    git_in(&seed, &["commit", "-q", "-m", "policy"]);
    git_in(&seed, &["push", "-q", remote.to_str().unwrap(), "trunk"]);
    git_in(&b, &["clone", "-q", remote.to_str().unwrap(), "clone"]);
    Fx {
        _base: base,
        remote,
        clone,
        data,
    }
}

fn resolve_fx(f: &Fx) -> Resolution {
    resolve(&f.clone, &f.data, ProjectPolicy::Remote)
}

fn effective(res: &Resolution) -> SandboxConfig {
    let mut c = SandboxConfig::default();
    let mut m = ModeSet::implicit();
    apply(res, &mut c, &mut m);
    c
}

#[test]
fn committed_policy_is_honoured() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(POLICY, None);
    let res = resolve_fx(&f);
    assert_eq!(res.status, Status::Active, "{:?}", res.notices);
    let v = res.verified.as_ref().unwrap();
    assert_eq!(v.source.branch, "trunk");
    assert!(!v.source.from_cache);
    assert_eq!(v.source.url, f.remote.to_str().unwrap());
    assert!(res.drift.is_none(), "{:?}", res.drift);
    let c = effective(&res);
    assert_eq!(c.network, NetworkMode::Off);
    assert_eq!(c.extra_write_paths, ["/data"]);
    assert!(c.enabled, "enabled is never taken from a project");
    let n = res.notices.join("\n");
    assert!(
        n.contains("will be taken from") && n.contains("trunk"),
        "{n}"
    );
    // The notice is one-time.
    let again = resolve_fx(&f);
    assert!(
        !again
            .notices
            .iter()
            .any(|n| n.contains("will be taken from")),
        "{:?}",
        again.notices
    );
    // The pin lives in the data dir, outside the repo.
    assert!(pinned_url(&f.clone, &f.data).is_some());
}

#[test]
fn project_cannot_set_enabled_or_project_policy() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(
        "[sandbox]\nenabled = false\nproject_policy = \"off\"\nnetwork = \"off\"\n",
        None,
    );
    let c = effective(&resolve_fx(&f));
    assert!(c.enabled);
    assert_eq!(c.project_policy, ProjectPolicy::Remote);
    assert_eq!(c.network, NetworkMode::Off);
}

#[test]
fn working_tree_edit_is_ignored_with_a_drift_warning() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(POLICY, None);
    resolve_fx(&f);
    std::fs::write(
        f.clone.join(".ruckcode/config.toml"),
        "[sandbox]\nnetwork = \"full\"\nextra_write_paths = [\"/\"]\n",
    )
    .unwrap();
    let res = resolve_fx(&f);
    let c = effective(&res);
    assert_eq!(c.network, NetworkMode::Off);
    assert_eq!(c.extra_write_paths, ["/data"]);
    let d = res.drift.expect("drift warning");
    assert!(d.contains("do not take effect until merged"), "{d}");
    assert!(d.contains("trunk"), "{d}");
    assert!(res.notices.iter().any(|n| n.contains("until merged")));
    // The normal config loader no longer reads [sandbox] from the working tree either.
    let cfg = crate::Config::load(&f.clone, None).unwrap();
    assert_eq!(cfg.sandbox.network, NetworkMode::Allowlist);
}

#[test]
fn staged_and_local_commits_are_ignored() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(POLICY, None);
    std::fs::write(
        f.clone.join(".ruckcode/config.toml"),
        "[sandbox]\nnetwork = \"full\"\n",
    )
    .unwrap();
    git_in(&f.clone, &["add", "."]);
    // staged
    let c = effective(&resolve_fx(&f));
    assert_eq!(c.network, NetworkMode::Off);
    // committed locally
    git_in(&f.clone, &["commit", "-q", "-m", "loosen"]);
    let res = resolve_fx(&f);
    assert_eq!(effective(&res).network, NetworkMode::Off);
    assert!(res.drift.is_some());
}

#[test]
fn forged_remote_tracking_ref_is_ignored() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(POLICY, None);
    std::fs::write(
        f.clone.join(".ruckcode/config.toml"),
        "[sandbox]\nnetwork = \"full\"\n",
    )
    .unwrap();
    git_in(&f.clone, &["commit", "-q", "-am", "evil"]);
    let evil = git_in(&f.clone, &["rev-parse", "HEAD"]);
    git_in(
        &f.clone,
        &["update-ref", "refs/remotes/origin/trunk", &evil],
    );
    git_in(&f.clone, &["update-ref", "refs/remotes/origin/HEAD", &evil]);
    let res = resolve_fx(&f);
    assert_eq!(effective(&res).network, NetworkMode::Off);
    assert_ne!(res.verified.unwrap().source.sha, evil);
}

#[test]
fn changed_origin_url_disables_project_policy_until_trusted() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(POLICY, None);
    resolve_fx(&f);
    // An attacker's remote serving a loose policy.
    let b = f.remote.parent().unwrap();
    let evil = b.join("evil.git");
    let seed = b.join("evilseed");
    std::fs::create_dir_all(seed.join(".ruckcode")).unwrap();
    git_in(b, &["init", "-q", "--bare", "-b", "trunk", "evil.git"]);
    git_in(b, &["init", "-q", "-b", "trunk", "evilseed"]);
    std::fs::write(
        seed.join(".ruckcode/config.toml"),
        "[sandbox]\nnetwork = \"full\"\n",
    )
    .unwrap();
    git_in(&seed, &["add", "."]);
    git_in(&seed, &["commit", "-q", "-m", "x"]);
    git_in(&seed, &["push", "-q", evil.to_str().unwrap(), "trunk"]);
    git_in(
        &f.clone,
        &["remote", "set-url", "origin", evil.to_str().unwrap()],
    );
    let res = resolve_fx(&f);
    assert!(
        matches!(res.status, Status::PinMismatch { .. }),
        "{:?}",
        res.status
    );
    assert!(res.verified.is_none());
    assert_eq!(effective(&res).network, NetworkMode::Allowlist);
    assert!(res.notices.join("\n").contains("rc sandbox trust-remote"));
    // Explicit re-pin by the user.
    let url = trust_remote(&f.clone, &f.data).unwrap();
    assert_eq!(url, evil.to_str().unwrap());
    let res = resolve_fx(&f);
    assert_eq!(res.status, Status::Active, "{:?}", res.notices);
    assert_eq!(effective(&res).network, NetworkMode::Full);
}

#[test]
fn offline_uses_the_cached_verified_copy() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(POLICY, None);
    let first = resolve_fx(&f);
    let sha = first.verified.unwrap().source.sha;
    std::fs::rename(&f.remote, f.remote.with_extension("gone")).unwrap();
    let res = resolve_fx(&f);
    assert_eq!(res.status, Status::Active);
    let v = res.verified.as_ref().unwrap();
    assert!(v.source.from_cache);
    assert_eq!(v.source.sha, sha);
    assert_eq!(effective(&res).network, NetworkMode::Off);
    let n = res.notices.join("\n");
    assert!(n.contains("Using project sandbox policy from"), "{n}");
}

#[test]
fn offline_without_a_cache_means_no_project_policy() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(POLICY, None);
    std::fs::rename(&f.remote, f.remote.with_extension("gone")).unwrap();
    let res = resolve_fx(&f);
    assert!(
        matches!(res.status, Status::Unavailable { .. }),
        "{:?}",
        res.status
    );
    assert!(res.verified.is_none());
    assert_eq!(effective(&res).network, NetworkMode::Allowlist);
    // The committed-looking working tree copy is ignored, and the user is told.
    assert!(res.drift.is_some());
}

#[test]
fn no_remote_or_no_repo_means_no_project_policy() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let b = tempfile::tempdir().unwrap();
    let data = b.path().join("data");
    let proj = b.path().join("proj");
    std::fs::create_dir_all(proj.join(".ruckcode")).unwrap();
    std::fs::write(proj.join(".ruckcode/config.toml"), POLICY).unwrap();
    let res = resolve(&proj, &data, ProjectPolicy::Remote);
    assert_eq!(res.status, Status::NotGit);
    assert_eq!(effective(&res).network, NetworkMode::Allowlist);
    git_in(&proj, &["init", "-q"]);
    let res = resolve(&proj, &data, ProjectPolicy::Remote);
    assert_eq!(res.status, Status::NoRemote);
    assert!(res.verified.is_none());
    assert_eq!(effective(&res).network, NetworkMode::Allowlist);
    assert!(res.drift.is_some());
    assert!(trust_remote(&proj, &data).is_err());
}

#[test]
fn project_policy_off_ignores_everything() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(POLICY, None);
    let res = resolve(&f.clone, &f.data, ProjectPolicy::Off);
    assert_eq!(res.status, Status::Off);
    assert!(res.verified.is_none() && res.notices.is_empty());
    assert!(!f.data.exists(), "nothing is pinned when the policy is off");
}

#[cfg(unix)]
#[test]
fn malicious_git_config_does_not_run_during_the_fetch() {
    use std::os::unix::fs::PermissionsExt;
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture(POLICY, None);
    let marker = f.clone.parent().unwrap().join("PWNED");
    let script = f.clone.parent().unwrap().join("pwn.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch {}\nexit 0\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    // hooks directory that fires on everything
    let hooks = f.clone.parent().unwrap().join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for h in [
        "reference-transaction",
        "post-fetch",
        "pre-auto-gc",
        "post-checkout",
        "post-merge",
        "fsmonitor-watchman",
    ] {
        std::fs::copy(&script, hooks.join(h)).unwrap();
    }
    let cfg = f.clone.join(".git/config");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str(&format!(
        "[core]\n\tfsmonitor = {s}\n\thooksPath = {h}\n\tsshCommand = {s}\n\tpager = {s}\n\teditor = {s}\n\
         [credential]\n\thelper = !{s}\n\
         [uploadpack]\n\tpackObjectsHook = {s}\n\
         [protocol \"ext\"]\n\tallow = always\n\
         [url \"ext::{s} %S\"]\n\tinsteadOf = {r}\n\
         [include]\n\tpath = {s}\n",
        s = script.display(),
        h = hooks.display(),
        r = f.remote.display()
    ));
    std::fs::write(&cfg, text).unwrap();
    // `origin` itself is untouched (the pin would otherwise mismatch).
    let res = resolve_fx(&f);
    assert!(!marker.exists(), "repo-local config executed code");
    // Policy still came from the real remote, fetched by URL.
    assert_eq!(res.status, Status::Active, "{:?}", res.notices);
    assert_eq!(effective(&res).network, NetworkMode::Off);
    // And a second (cached/refetched) run is equally harmless.
    resolve_fx(&f);
    assert!(!marker.exists());
}

#[test]
fn per_mode_overrides_come_from_the_committed_modes_file() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let modes = "default_mode = \"build\"\n[modes.build]\ndescription = \"b\"\n[modes.build.sandbox]\nnetwork = \"off\"\n";
    let f = fixture("", Some(modes));
    // A local modes.toml tries to widen it.
    std::fs::write(
        f.clone.join(".ruckcode/modes.toml"),
        "default_mode = \"build\"\n[modes.build]\ndescription = \"b\"\n[modes.build.sandbox]\nnetwork = \"full\"\n",
    )
    .unwrap();
    let local = ModeSet::load(&f.clone, None).unwrap();
    assert!(
        local.get("build").unwrap().sandbox.is_none(),
        "working tree sandbox overrides are dropped by the loader"
    );
    let res = resolve_fx(&f);
    assert_eq!(res.status, Status::Active, "{:?}", res.notices);
    let mut set = local;
    let mut c = SandboxConfig::default();
    assert!(apply(&res, &mut c, &mut set).is_empty());
    let eff = c.with_override(set.get("build").unwrap().sandbox.as_ref());
    assert_eq!(eff.network, NetworkMode::Off);
    assert!(res.drift.is_some(), "local modes.toml differs");
}

#[test]
fn project_in_a_subdirectory_reads_its_own_files() {
    if !have_git() {
        return eprintln!("SKIPPED: git not installed");
    }
    let f = fixture("", None);
    let sub = f.clone.join("svc");
    std::fs::create_dir_all(sub.join(".ruckcode")).unwrap();
    std::fs::write(sub.join(".ruckcode/config.toml"), POLICY).unwrap();
    git_in(&f.clone, &["add", "."]);
    git_in(&f.clone, &["commit", "-q", "-m", "svc"]);
    git_in(&f.clone, &["push", "-q", "origin", "trunk"]);
    let res = resolve(&sub, &f.data, ProjectPolicy::Remote);
    assert_eq!(res.status, Status::Active, "{:?}", res.notices);
    assert_eq!(effective(&res).network, NetworkMode::Off);
}

#[test]
fn urls_and_refs_are_validated() {
    assert!(!safe_url("--upload-pack=x"));
    assert!(!safe_url("ext::sh -c evil"));
    assert!(safe_url("https://example.com/a.git"));
    assert!(safe_ref("main") && safe_ref("release/1.x"));
    assert!(!safe_ref("-x") && !safe_ref("a..b") && !safe_ref("a b"));
}
