//! End-to-end enforcement tests: they run real commands through the real launcher.
//! Tests that need a kernel feature (Landlock network rules, user namespaces, ...) detect it and
//! skip with a message instead of failing.

#![cfg(unix)]

use ruckcode_sandbox::{
    LauncherExe, NamespaceMode, NetworkMode, Prep, Sandbox, SandboxConfig, SandboxOptions,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

struct Env {
    _base: tempfile::TempDir,
    proj: PathBuf,
    outside: PathBuf,
    home: PathBuf,
}

/// Directories outside /tmp (which is writable by policy), next to the test binary.
fn env() -> Env {
    let exe = std::env::current_exe().unwrap();
    let base = tempfile::tempdir_in(exe.parent().unwrap()).unwrap();
    let b = base.path().canonicalize().unwrap();
    let (proj, outside, home) = (b.join("proj"), b.join("outside"), b.join("home"));
    for d in [&proj, &outside, &home, &home.join(".ssh")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(home.join(".ssh/id_rsa"), "TOPSECRET").unwrap();
    std::fs::write(home.join("notes.txt"), "hello-notes").unwrap();
    std::fs::write(proj.join("a.txt"), "project-file").unwrap();
    Env {
        _base: base,
        proj,
        outside,
        home,
    }
}

async fn sandbox(e: &Env) -> Arc<Sandbox> {
    Sandbox::new(SandboxOptions {
        project_root: e.proj.clone(),
        exe: LauncherExe::Path(PathBuf::from(env!("CARGO_BIN_EXE_rc-sandbox-exec"))),
        prompter: None,
        store: None,
        config_dir: None,
        home: Some(e.home.clone()),
        caps: None,
    })
    .await
}

struct Out {
    code: i32,
    text: String,
    notes: String,
}

async fn run(sb: &Sandbox, cfg: &SandboxConfig, cwd: &Path, script: &str) -> Out {
    let argv = vec!["sh".to_string(), "-c".to_string(), script.to_string()];
    let prep = sb.prepare(cfg, cwd, &argv, "t").expect("prepare");
    let Prep::Run(mut p) = prep else {
        panic!("sandbox unavailable on this platform")
    };
    p.command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = p.spawn().expect("spawn");
    let o = child.wait_with_output().await.unwrap();
    let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&o.stderr));
    let code = o.status.code().unwrap_or(-1);
    let notes = p.finish(&text, o.status.success());
    Out { code, text, notes }
}

fn cfg(ns: NamespaceMode, net: NetworkMode) -> SandboxConfig {
    SandboxConfig {
        namespaces: ns,
        network: net,
        ..Default::default()
    }
}

fn python() -> &'static str {
    // Prefer a real interpreter over version-manager shims (which read $HOME config).
    [
        "/usr/bin/python3",
        "/usr/local/bin/python3",
        "/opt/homebrew/bin/python3",
    ]
    .into_iter()
    .find(|p| Path::new(p).exists())
    .unwrap_or("python3")
}

fn have_python() -> bool {
    std::process::Command::new(python())
        .arg("-c")
        .arg("1")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn skip(why: &str) {
    eprintln!("SKIPPED: {why}");
}

/// Layers must actually enforce on this host, otherwise tests that assert denial are skipped.
fn enforcing(sb: &Sandbox, c: &SandboxConfig) -> bool {
    let l = sb.plan(c).layers;
    let ok = l.landlock || l.namespaces || l.seatbelt;
    if !ok {
        skip("no filesystem confinement layer available on this host");
    }
    ok
}

async fn fs_checks(ns: NamespaceMode) {
    let e = env();
    let sb = sandbox(&e).await;
    let c = cfg(ns, NetworkMode::Full);
    if !enforcing(&sb, &c) {
        return;
    }
    if ns == NamespaceMode::Required && !sb.plan(&c).layers.namespaces {
        skip("user namespaces unavailable here");
        return;
    }
    let (p, o, h) = (e.proj.display(), e.outside.display(), e.home.display());
    // inside the project: allowed; outside: denied
    let r = run(
        &sb,
        &c,
        &e.proj,
        &format!("echo x > {p}/new.txt && cat {p}/a.txt"),
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.text);
    assert!(r.text.contains("project-file"));
    assert!(e.proj.join("new.txt").exists());
    let r = run(&sb, &c, &e.proj, &format!("echo x > {o}/evil.txt")).await;
    assert_ne!(r.code, 0, "write outside the project must fail");
    assert!(!e.outside.join("evil.txt").exists());
    assert!(
        r.notes.contains("[sandbox]") && r.notes.contains("extra_write_paths"),
        "notes={} text={} code={}",
        r.notes,
        r.text,
        r.code
    );
    // secrets unreadable, other home files readable
    let r = run(&sb, &c, &e.proj, &format!("cat {h}/.ssh/id_rsa")).await;
    assert!(!r.text.contains("TOPSECRET"), "{}", r.text);
    assert_ne!(r.code, 0);
    let r = run(&sb, &c, &e.proj, &format!("cat {h}/notes.txt")).await;
    assert!(r.text.contains("hello-notes"), "{}", r.text);
    // children inherit the restrictions
    let r = run(
        &sb,
        &c,
        &e.proj,
        &format!("sh -c 'sh -c \"echo x > {o}/deep.txt\"'"),
    )
    .await;
    assert_ne!(r.code, 0);
    assert!(!e.outside.join("deep.txt").exists());
    // exit codes propagate
    assert_eq!(run(&sb, &c, &e.proj, "exit 7").await.code, 7);
}

#[tokio::test]
async fn filesystem_confinement_auto() {
    fs_checks(NamespaceMode::Auto).await;
}

#[tokio::test]
async fn filesystem_confinement_landlock_only() {
    let e = env();
    let sb = sandbox(&e).await;
    if sb.capabilities().platform == "linux" && sb.capabilities().landlock_abi.is_none() {
        skip("Landlock unavailable on this kernel");
        return;
    }
    fs_checks(NamespaceMode::Off).await;
}

#[tokio::test]
async fn filesystem_confinement_namespaces() {
    let e = env();
    let sb = sandbox(&e).await;
    if !sb.capabilities().namespaces {
        skip(&format!(
            "user namespaces unavailable: {}",
            sb.capabilities()
                .namespaces_reason
                .clone()
                .unwrap_or_default()
        ));
        return;
    }
    fs_checks(NamespaceMode::Required).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn udp_sockets_are_denied() {
    if !have_python() {
        return skip("python3 not installed");
    }
    let e = env();
    let sb = sandbox(&e).await;
    let c = cfg(NamespaceMode::Off, NetworkMode::Allowlist);
    if !sb.capabilities().seccomp {
        return skip("seccomp unavailable");
    }
    let r = run(
        &sb,
        &c,
        &e.proj,
        &format!("{} -c 'import socket; socket.socket(socket.AF_INET, socket.SOCK_DGRAM)' 2>&1; echo rc=$?", python()),
    )
    .await;
    assert!(
        r.text.contains("PermissionError") || r.text.contains("rc=1"),
        "{}",
        r.text
    );
    assert!(!r.text.contains("rc=0"), "{}", r.text);
    // ptrace is denied too
    let r = run(&sb, &c, &e.proj, &format!("{} -c 'import ctypes,sys; l=ctypes.CDLL(None); sys.exit(0 if l.ptrace(0,0,0,0)!=-1 else 3)'", python())).await;
    assert_eq!(r.code, 3, "{}", r.text);
}

/// A local HTTP server answering "pong"; returns its port.
async fn test_server() -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut b = [0u8; 4096];
                let _ = c.read(&mut b).await;
                let _ = c
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\npong",
                    )
                    .await;
            });
        }
    });
    port
}

const VIA_PROXY: &str = r#"import os,sys,urllib.request as u,urllib.error as ue
h=os.environ['HTTP_PROXY']
os.environ.pop('NO_PROXY',None); os.environ.pop('no_proxy',None)
o=u.build_opener(u.ProxyHandler({'http':h}))
try:
    print(o.open(sys.argv[1],timeout=10).read().decode())
except ue.HTTPError as e:
    print('HTTP',e.code,e.read().decode())
"#;
const DIRECT: &str = r#"import socket,sys
s=socket.socket()
s.settimeout(5)
try:
    s.connect(('127.0.0.1',int(sys.argv[1])))
    print('CONNECTED')
except Exception as e:
    print('BLOCKED',e)
"#;

async fn network_checks(ns: NamespaceMode) {
    if !have_python() {
        return skip("python3 not installed");
    }
    let e = env();
    let sb = sandbox(&e).await;
    let mut c = cfg(ns, NetworkMode::Allowlist);
    c.allowed_domains = vec!["127.0.0.1".into()];
    let plan = sb.plan(&c);
    if !(plan.layers.netns || plan.layers.landlock_net || plan.layers.seatbelt) {
        return skip("no network isolation available (needs Landlock ABI 4 / Linux 6.7+, namespaces or Seatbelt)");
    }
    let port = test_server().await;
    let py = e.proj.join("t.py");
    std::fs::write(e.proj.join("direct.py"), DIRECT).unwrap();
    std::fs::write(&py, VIA_PROXY).unwrap();
    // direct connection to a non-proxy port is blocked
    let r = run(&sb, &c, &e.proj, &format!("{} direct.py {port}", python())).await;
    assert!(
        r.text.contains("BLOCKED"),
        "direct TCP must fail: {}",
        r.text
    );
    // through the proxy: allowed host works
    let r = run(
        &sb,
        &c,
        &e.proj,
        &format!("{} t.py http://127.0.0.1:{port}/", python()),
    )
    .await;
    assert!(r.text.contains("pong"), "{}", r.text);
    // not on the allowlist: refused, and the result explains why
    let r = run(
        &sb,
        &c,
        &e.proj,
        &format!("{} t.py http://not-allowed.invalid/", python()),
    )
    .await;
    assert!(
        r.text.contains("HTTP 403") && r.text.contains("not allowed"),
        "{}",
        r.text
    );
    assert!(r.notes.contains("not-allowed.invalid"), "{}", r.notes);
}

#[tokio::test]
async fn network_through_proxy_auto() {
    network_checks(NamespaceMode::Auto).await;
}

#[tokio::test]
async fn network_through_proxy_landlock() {
    let e = env();
    let sb = sandbox(&e).await;
    if !sb.capabilities().landlock_net() && cfg!(target_os = "linux") {
        return skip(&format!(
            "Landlock network rules unavailable (ABI {:?}; needs 4 / Linux 6.7+)",
            sb.capabilities().landlock_abi
        ));
    }
    network_checks(NamespaceMode::Off).await;
}

/// Regression: every namespace probe used one directory per pid, so concurrent probes in a
/// process deleted each other's directory ("cannot enter .../proj: No such file or directory")
/// and reported namespaces as unavailable. Probes must be independent.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_namespace_probes_all_agree() {
    let exe = LauncherExe::Path(PathBuf::from(env!("CARGO_BIN_EXE_rc-sandbox-exec")));
    let reference = ruckcode_sandbox::caps::detect_uncached(&exe);
    if !reference.namespaces {
        return skip("user namespaces unavailable");
    }
    for _round in 0..4 {
        let probes: Vec<_> = (0..16)
            .map(|_| {
                let exe = exe.clone();
                tokio::task::spawn_blocking(move || ruckcode_sandbox::caps::detect_uncached(&exe))
            })
            .collect();
        for p in probes {
            let c = p.await.unwrap();
            assert!(
                c.namespaces,
                "a concurrent probe failed: {:?}",
                c.namespaces_reason
            );
        }
    }
}

/// Regression: many Sandboxes created and used at once (parallel tests, parallel tool calls)
/// must each get working namespaces and their own private stage directory.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sandboxes_prepare_and_run() {
    let e = env();
    if !sandbox(&e).await.capabilities().namespaces {
        return skip("user namespaces unavailable");
    }
    let tasks: Vec<_> = (0..12)
        .map(|i| {
            let (proj, home) = (e.proj.clone(), e.home.clone());
            tokio::spawn(async move {
                let e2 = Env {
                    _base: tempfile::tempdir().unwrap(),
                    proj,
                    outside: PathBuf::new(),
                    home,
                };
                let sb = sandbox(&e2).await;
                let c = cfg(NamespaceMode::Required, NetworkMode::Allowlist);
                let r = run(&sb, &c, &e2.proj, &format!("echo ok-{i}")).await;
                assert!(r.text.contains(&format!("ok-{i}")), "{}", r.text);
                sb
            })
        })
        .collect();
    let mut stages = std::collections::HashSet::new();
    for t in tasks {
        let sb = t.await.unwrap();
        assert!(
            stages.insert(
                sb.policy_for(
                    &cfg(NamespaceMode::Required, NetworkMode::Allowlist),
                    &e.proj,
                    &sb.plan(&cfg(NamespaceMode::Required, NetworkMode::Allowlist)),
                )
                .stage_dir
            ),
            "two sandboxes shared a stage dir"
        );
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn network_through_proxy_in_netns() {
    let e = env();
    let sb = sandbox(&e).await;
    if !sb.capabilities().namespaces {
        return skip("user namespaces unavailable");
    }
    network_checks(NamespaceMode::Required).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn namespace_masks_env_and_isolates_pids() {
    let e = env();
    let sb = sandbox(&e).await;
    if !sb.capabilities().namespaces {
        return skip(&format!(
            "user namespaces unavailable: {}",
            sb.capabilities()
                .namespaces_reason
                .clone()
                .unwrap_or_default()
        ));
    }
    let c = cfg(NamespaceMode::Required, NetworkMode::Off);
    std::fs::write(e.proj.join(".env"), "API_KEY=supersecret").unwrap();
    std::fs::write(e.proj.join(".env.example"), "API_KEY=").unwrap();
    let r = run(
        &sb,
        &c,
        &e.proj,
        "cat .env; echo ---; cat a.txt; cat .env.example",
    )
    .await;
    assert!(
        !r.text.contains("supersecret"),
        "masked .env leaked: {}",
        r.text
    );
    assert!(
        r.text.contains("project-file") && r.text.contains("API_KEY="),
        "{}",
        r.text
    );
    // PID isolation: only sandbox processes are visible, not the test process.
    let me = std::process::id().to_string();
    let r = run(&sb, &c, &e.proj, "ls /proc | grep -E '^[0-9]+$'").await;
    let pids: Vec<&str> = r.text.lines().collect();
    assert!(!pids.contains(&me.as_str()), "host pid visible: {pids:?}");
    assert!(pids.len() < 10, "{pids:?}");
    assert!(pids.contains(&"1"));
    // ruckcode's project config is read-only inside the sandbox
    std::fs::create_dir_all(e.proj.join(".ruckcode")).unwrap();
    std::fs::write(e.proj.join(".ruckcode/modes.toml"), "orig").unwrap();
    let r = run(&sb, &c, &e.proj, "echo evil > .ruckcode/modes.toml").await;
    assert_ne!(r.code, 0, "{}", r.text);
    assert_eq!(
        std::fs::read_to_string(e.proj.join(".ruckcode/modes.toml")).unwrap(),
        "orig"
    );
    // no host network
    let r = run(&sb, &c, &e.proj, "cat /proc/net/dev | grep -c ':'").await;
    assert!(r.text.trim().parse::<u32>().unwrap_or(9) <= 1, "{}", r.text);
}

#[cfg(unix)]
#[tokio::test]
async fn config_changes_by_commands_are_reverted() {
    let e = env();
    let sb = sandbox(&e).await;
    let c = cfg(NamespaceMode::Off, NetworkMode::Full);
    if !enforcing(&sb, &c) {
        return;
    }
    std::fs::create_dir_all(e.proj.join(".ruckcode")).unwrap();
    std::fs::write(e.proj.join(".ruckcode/config.toml"), "orig").unwrap();
    let r = run(&sb, &c, &e.proj, "echo '[sandbox]' > .ruckcode/config.toml").await;
    assert_eq!(
        std::fs::read_to_string(e.proj.join(".ruckcode/config.toml")).unwrap(),
        "orig"
    );
    let _ = r;
}

#[cfg(unix)]
#[tokio::test]
async fn orphans_die_with_the_launcher() {
    use std::os::unix::process::CommandExt;
    let e = env();
    let sb = sandbox(&e).await;
    let c = cfg(NamespaceMode::Auto, NetworkMode::Full);
    if !enforcing(&sb, &c) {
        return;
    }
    let marker = e.proj.join("alive");
    let script = format!("(sleep 1.5; touch {}) & sleep 30", marker.display());
    let argv = vec!["sh".to_string(), "-c".to_string(), script];
    let Prep::Run(mut p) = sb.prepare(&c, &e.proj, &argv, "t").unwrap() else {
        panic!()
    };
    unsafe {
        p.command.as_std_mut().pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    p.command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = p.spawn().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    // What the Bash tool does on timeout/cancel.
    unsafe { libc::kill(-(child.id().unwrap() as i32), libc::SIGKILL) };
    let _ = child.wait().await;
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert!(!marker.exists(), "orphan survived the kill");
}

#[cfg(unix)]
#[tokio::test]
async fn background_children_are_reaped_when_the_command_ends() {
    let e = env();
    let sb = sandbox(&e).await;
    let c = cfg(NamespaceMode::Required, NetworkMode::Full);
    if !sb.capabilities().namespaces || !cfg!(target_os = "linux") {
        return skip("user namespaces unavailable");
    }
    let marker = e.proj.join("bg");
    let r = run(
        &sb,
        &c,
        &e.proj,
        &format!(
            "(sleep 1; touch {}) > /dev/null 2>&1 & echo started",
            marker.display()
        ),
    )
    .await;
    assert!(r.text.contains("started"), "{}", r.text);
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
    assert!(!marker.exists(), "background process outlived its sandbox");
}

#[tokio::test]
async fn strict_refuses_without_enforcement() {
    let e = env();
    let sb = Sandbox::new(SandboxOptions {
        project_root: e.proj.clone(),
        exe: LauncherExe::Path(PathBuf::from(env!("CARGO_BIN_EXE_rc-sandbox-exec"))),
        prompter: None,
        store: None,
        config_dir: None,
        home: Some(e.home.clone()),
        caps: Some(ruckcode_sandbox::Capabilities {
            platform: "linux".into(),
            landlock_abi: None,
            seccomp: false,
            namespaces: false,
            namespaces_reason: Some("test".into()),
            seatbelt: false,
            launcher_error: None,
        }),
    })
    .await;
    let strict = SandboxConfig {
        strict: true,
        ..Default::default()
    };
    let argv = vec!["true".to_string()];
    match sb.prepare(&strict, &e.proj, &argv, "t") {
        Err(m) => assert!(m.contains("sandbox.strict") && m.contains("not run"), "{m}"),
        Ok(_) => panic!("strict mode must refuse"),
    }
    // Non-strict degrades with a one-time warning.
    let lax = SandboxConfig::default();
    match sb.prepare(&lax, &e.proj, &argv, "t") {
        Ok(Prep::Run(p)) => assert!(p.notice.is_some()),
        _ => panic!("expected degraded run"),
    }
    match sb.prepare(&lax, &e.proj, &argv, "t") {
        Ok(Prep::Run(p)) => assert!(p.notice.is_none(), "warn once per session"),
        _ => panic!(),
    }
}

fn have_git() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_repo(dir: &Path) {
    let st = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(dir)
        .status()
        .unwrap();
    assert!(st.success());
    std::fs::write(
        dir.join(".git/config"),
        "[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = /nowhere/repo.git\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join(".git/refs/remotes/origin")).unwrap();
    std::fs::write(
        dir.join(".git/refs/remotes/origin/main"),
        "0000000000000000000000000000000000000001\n",
    )
    .unwrap();
}

const GIT_ID: &str = "GIT_CONFIG_GLOBAL=/dev/null XDG_CONFIG_HOME=/tmp/nocfg git -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false";

/// Kernel-enforced read-only `.git` paths: namespaces on Linux, Seatbelt on macOS.
#[tokio::test]
async fn kernel_makes_git_config_hooks_and_remote_refs_read_only() {
    let e = env();
    let sb = sandbox(&e).await;
    let c = cfg(NamespaceMode::Auto, NetworkMode::Off);
    let l = sb.plan(&c).layers;
    if !(l.namespaces || l.seatbelt) {
        return skip(&format!(
            "neither user namespaces nor Seatbelt available: {}",
            sb.capabilities()
                .namespaces_reason
                .clone()
                .unwrap_or_default()
        ));
    }
    if !have_git() {
        return skip("git not installed");
    }
    git_repo(&e.proj);
    let orig = std::fs::read_to_string(e.proj.join(".git/config")).unwrap();
    for (name, script) in [
        ("hook", "echo evil > .git/hooks/post-commit"),
        ("config", "git config core.fsmonitor 'touch /tmp/pwned'"),
        ("config-append", "echo '[core]' >> .git/config"),
        (
            "remote ref",
            "echo 1111111111111111111111111111111111111111 > .git/refs/remotes/origin/main",
        ),
        ("info", "echo x > .git/info/exclude"),
    ] {
        let r = run(&sb, &c, &e.proj, script).await;
        assert_ne!(r.code, 0, "{name} must fail: {}", r.text);
    }
    assert_eq!(
        std::fs::read_to_string(e.proj.join(".git/config")).unwrap(),
        orig
    );
    assert!(!e.proj.join(".git/hooks/post-commit").exists());
    // Normal git keeps working: commit, branch, checkout, stash.
    let r = run(
        &sb,
        &c,
        &e.proj,
        &format!(
            "{GIT_ID} add a.txt && {GIT_ID} commit -q -m one && {GIT_ID} branch topic && \
             {GIT_ID} checkout -q topic && echo more >> a.txt && {GIT_ID} stash -q && \
             {GIT_ID} checkout -q main && {GIT_ID} log --oneline"
        ),
    )
    .await;
    assert_eq!(r.code, 0, "git inside the sandbox: {}", r.text);
    assert!(r.text.contains("one"), "{}", r.text);
    assert!(e.proj.join(".git/refs/heads/topic").exists());
}

#[tokio::test]
async fn landlock_only_restores_git_config_hooks_and_remote_refs() {
    let e = env();
    let sb = sandbox(&e).await;
    let c = cfg(NamespaceMode::Off, NetworkMode::Full);
    if !enforcing(&sb, &c) {
        return;
    }
    if !have_git() {
        return skip("git not installed");
    }
    git_repo(&e.proj);
    std::fs::write(e.proj.join(".git/hooks/pre-commit.sample"), "orig-hook").unwrap();
    let orig = std::fs::read_to_string(e.proj.join(".git/config")).unwrap();
    let r = run(
        &sb,
        &c,
        &e.proj,
        "echo evil > .git/hooks/post-commit; echo '[core]' >> .git/config; \
         rm .git/hooks/pre-commit.sample; \
         echo 1111111111111111111111111111111111111111 > .git/refs/remotes/origin/main",
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(e.proj.join(".git/config")).unwrap(),
        orig
    );
    assert!(!e.proj.join(".git/hooks/post-commit").exists());
    assert_eq!(
        std::fs::read_to_string(e.proj.join(".git/hooks/pre-commit.sample")).unwrap(),
        "orig-hook"
    );
    assert!(
        std::fs::read_to_string(e.proj.join(".git/refs/remotes/origin/main"))
            .unwrap()
            .starts_with("0000000")
    );
    if !sb.plan(&c).layers.seatbelt {
        assert!(
            r.notes.contains(".git/config") && r.notes.contains("reverted"),
            "{}",
            r.notes
        );
    }
    // Local work is not reverted.
    let r = run(
        &sb,
        &c,
        &e.proj,
        &format!("{GIT_ID} add a.txt && {GIT_ID} commit -q -m one && {GIT_ID} branch topic"),
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.text);
    assert!(e.proj.join(".git/refs/heads/topic").exists());
    assert!(!r.notes.contains("reverted"), "{}", r.notes);
}

/// The model cannot pin or re-pin the policy remote: the pin lives in the user data directory,
/// which is not writable by sandboxed commands.
#[tokio::test]
async fn user_data_dir_is_not_writable_by_commands() {
    let e = env();
    let sb = sandbox(&e).await;
    for ns in [NamespaceMode::Off, NamespaceMode::Auto] {
        let c = cfg(ns, NetworkMode::Full);
        if !enforcing(&sb, &c) {
            return;
        }
        let data = e.home.join(".local/share/ruckcode");
        std::fs::create_dir_all(data.join("project-policy")).unwrap();
        let r = run(
            &sb,
            &c,
            &e.proj,
            &format!("echo '{{}}' > {}/project-policy/pin.json", data.display()),
        )
        .await;
        assert_ne!(r.code, 0, "{}", r.text);
        assert!(!data.join("project-policy/pin.json").exists());
    }
}
