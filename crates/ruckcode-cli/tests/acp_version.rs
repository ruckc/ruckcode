//! `rc --version` and the ACP `initialize` result must report the same version string.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

fn rc() -> Command {
    let home = std::env::temp_dir();
    let mut c = Command::new(env!("CARGO_BIN_EXE_rc"));
    c.env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("RUCKCODE_CONFIG_DIR", home.join("rc-acp-version-cfg"))
        .current_dir(&home);
    c
}

#[test]
fn acp_agent_info_version_matches_rc_version() {
    let out = rc().arg("--version").output().unwrap();
    assert!(out.status.success());
    let printed = String::from_utf8(out.stdout).unwrap();
    let version = printed.trim().strip_prefix("rc ").expect("`rc <version>`");
    assert!(!version.is_empty());

    let mut child = rc()
        .arg("acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":1}}}}"#
    )
    .unwrap();
    stdin.flush().unwrap();
    // Keep stdin open until the reply is read, so the server cannot exit before answering.
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();

    let reply: serde_json::Value = serde_json::from_str(&line).expect("initialize reply");
    assert_eq!(reply["result"]["agentInfo"]["name"], "ruckcode");
    assert_eq!(reply["result"]["agentInfo"]["version"], version);
    assert_eq!(version, ruckcode_agent::identity::VERSION);
}
