//! The launcher passes its policy (and executable) to the child on fixed descriptor numbers. A
//! process that already has ~200 descriptors open hands those very numbers to the new pipe, so
//! the parent-side setup must not assume the source differs from the target. This runs in its own
//! test binary because it deliberately fills the descriptor table.

#![cfg(target_os = "linux")]

use ruck_harness_sandbox::{
    LauncherExe, NamespaceMode, NetworkMode, Prep, Sandbox, SandboxConfig, SandboxOptions,
};
use std::path::PathBuf;
use std::process::Stdio;

/// Hold descriptors until `target` is the lowest free descriptor number, and prove it by probing.
fn fill_fds_below(target: i32) -> Vec<std::fs::File> {
    use std::os::fd::AsRawFd;
    let mut held = Vec::new();
    loop {
        let f = std::fs::File::open("/dev/null").unwrap();
        let fd = f.as_raw_fd();
        assert!(
            fd <= target,
            "descriptor {target} was already in use (got {fd})"
        );
        if fd == target {
            // Closing it again leaves `target` as the lowest free number.
            drop(f);
            break;
        }
        held.push(f);
    }
    let probe = std::fs::File::open("/dev/null").unwrap();
    assert_eq!(
        probe.as_raw_fd(),
        target,
        "precondition: next fd is {target}"
    );
    held
}

#[tokio::test]
async fn launcher_works_when_pipe_lands_on_the_policy_fd() {
    let base = tempfile::tempdir().unwrap();
    let proj = base.path().canonicalize().unwrap().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let sb = Sandbox::new(SandboxOptions {
        project_root: proj.clone(),
        exe: LauncherExe::Path(PathBuf::from(env!("CARGO_BIN_EXE_rc-sandbox-exec"))),
        prompter: None,
        store: None,
        config_dir: None,
        home: Some(base.path().to_path_buf()),
        caps: None,
    })
    .await;
    let cfg = SandboxConfig {
        namespaces: NamespaceMode::Off,
        network: NetworkMode::Full,
        ..Default::default()
    };
    // prepare() allocates a pipe on the lowest free numbers: leave 198 (then 199) as the next
    // free descriptor so the read end lands exactly on a launcher descriptor.
    for next_free in [198, 199] {
        let held = fill_fds_below(next_free);
        let argv = vec!["sh".to_string(), "-c".to_string(), "echo fd-ok".to_string()];
        let Prep::Run(mut p) = sb.prepare(&cfg, &proj, &argv, "t").expect("prepare") else {
            panic!("sandbox unavailable")
        };
        p.command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let o = p.spawn().expect("spawn").wait_with_output().await.unwrap();
        let text =
            String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr);
        assert!(
            o.status.success() && text.contains("fd-ok"),
            "next_free={next_free}: {text}"
        );
        drop(held);
    }
}
