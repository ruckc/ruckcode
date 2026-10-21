//! Transcript browser commands. On the local host they call `ruckcode_core::transcript_api`
//! directly (on a blocking thread, since transcripts can be large); on WSL2 they run
//! `rc sessions ... --json` inside the distro, so both produce the same JSON.

use std::path::PathBuf;

use ruckcode_core::transcript_api::{self, ReadOptions};
use serde_json::Value;

use crate::setup::{via_wsl, RcCall, Target};

fn json<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn call(args: &[&str]) -> RcCall {
    RcCall {
        args: args.iter().map(|a| a.to_string()).collect(),
        stdin: None,
    }
}

pub fn list_call(query: Option<&str>) -> RcCall {
    let mut c = call(&["sessions", "--json"]);
    if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
        c.args.extend(["--query".into(), q.into()]);
    }
    c
}

pub fn read_call(id: &str, offset: Option<usize>, limit: Option<usize>) -> RcCall {
    let mut c = call(&["sessions", "show", id, "--json"]);
    if let Some(o) = offset {
        c.args.extend(["--offset".into(), o.to_string()]);
    }
    if let Some(l) = limit {
        c.args.extend(["--limit".into(), l.to_string()]);
    }
    c
}

pub fn search_call(text: &str, limit: Option<usize>) -> RcCall {
    let mut c = call(&["sessions", "search", text, "--json"]);
    if let Some(l) = limit {
        c.args.extend(["--limit".into(), l.to_string()]);
    }
    c
}

/// Run blocking transcript reading off the async runtime.
async fn local<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

pub async fn list(t: &Target, project: &str, query: Option<&str>) -> Result<Value, String> {
    match t {
        Target::Local => {
            let (p, q) = (PathBuf::from(project), query.map(String::from));
            json(local(move || transcript_api::list(&p, q.as_deref())).await?)
        }
        Target::Wsl(d) => via_wsl(d, project, list_call(query), 60).await,
    }
}

pub async fn read(
    t: &Target,
    project: &str,
    id: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Value, String> {
    match t {
        Target::Local => {
            let (p, id) = (PathBuf::from(project), id.to_string());
            json(local(move || transcript_api::read(&p, &id, ReadOptions { offset, limit })).await?)
        }
        Target::Wsl(d) => via_wsl(d, project, read_call(id, offset, limit), 60).await,
    }
}

pub async fn search(
    t: &Target,
    project: &str,
    text: &str,
    limit: Option<usize>,
) -> Result<Value, String> {
    match t {
        Target::Local => {
            let (p, text) = (PathBuf::from(project), text.to_string());
            json(local(move || transcript_api::search(&p, &text, limit)).await?)
        }
        Target::Wsl(d) => via_wsl(d, project, search_call(text, limit), 90).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_calls() {
        assert_eq!(list_call(None).args, ["sessions", "--json"]);
        assert_eq!(list_call(Some("  ")).args, ["sessions", "--json"]);
        assert_eq!(
            list_call(Some(" a b ")).args,
            ["sessions", "--json", "--query", "a b"]
        );
        assert_eq!(
            read_call("s1", None, None).args,
            ["sessions", "show", "s1", "--json"]
        );
        assert_eq!(
            read_call("s1", Some(10), Some(5)).args,
            ["sessions", "show", "s1", "--json", "--offset", "10", "--limit", "5"]
        );
        assert_eq!(
            search_call("x", Some(7)).args,
            ["sessions", "search", "x", "--json", "--limit", "7"]
        );
        assert!(search_call("x", None).stdin.is_none());
    }

    fn project(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rc-app-tr-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".ruckcode/sessions")).unwrap();
        let start = r#"{"ts":"2026-03-01T10:00:00.000Z","type":"session_start","id":"20260301-100000-aaaaaa","cwd":"/x","created_at":"2026-03-01T10:00:00.000Z","ruckcode_version":"0.1.0","profile":"p","model":"m","mode":"plan"}"#;
        let user = r#"{"ts":"2026-03-01T10:00:01.000Z","type":"user_message","source":"user","message":{"role":"user","content":[{"type":"text","text":"Find the Needle"}]}}"#;
        std::fs::write(
            d.join(".ruckcode/sessions/20260301-100000-aaaaaa.jsonl"),
            format!("{start}\n{user}\nbroken\n"),
        )
        .unwrap();
        d
    }

    #[tokio::test]
    async fn local_list_read_search() {
        let d = project("local");
        let p = d.to_str().unwrap();
        let l = list(&Target::Local, p, None).await.unwrap();
        assert_eq!(l[0]["firstPrompt"], "Find the Needle");
        assert_eq!(l[0]["skippedLines"], 1);
        assert!(list(&Target::Local, p, Some("zzz"))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        let r = read(
            &Target::Local,
            p,
            "20260301-100000-aaaaaa",
            Some(1),
            Some(10),
        )
        .await
        .unwrap();
        assert_eq!(r["total"], 2);
        assert_eq!(r["records"][0]["type"], "user_message");
        let h = search(&Target::Local, p, "needle", None).await.unwrap();
        assert_eq!(h[0]["recordIndex"], 1);
        // Ids that try to leave the sessions directory are errors.
        let e = read(&Target::Local, p, "../../etc/passwd", None, None)
            .await
            .unwrap_err();
        assert!(e.contains("invalid session id"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Stand-in `wsl.exe` running a fake `rc` that answers with a JSON array echoing its args.
    #[cfg(unix)]
    #[tokio::test]
    async fn wsl_calls_return_arrays_through_a_stand_in() {
        use crate::setup::run_rc;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;
        let dir = project("wsl");
        let home = dir.join("home");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        let rc = home.join(".local/bin/rc");
        std::fs::write(
            &rc,
            "#!/bin/sh\nif [ \"$2\" = fail ]; then echo 'rc: session not found: x' >&2; exit 1; fi\necho 'motd banner'\nprintf '[{\"args\":\"%s\",\"cwd\":\"%s\"}]\\n' \"$*\" \"$(pwd)\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o755)).unwrap();
        let wsl = dir.join("wsl.exe");
        std::fs::write(
            &wsl,
            format!("#!/bin/sh\n[ \"$1\" = -d ] && [ \"$2\" = Ubuntu ] && [ \"$3\" = --cd ] && [ \"$5\" = --exec ] && [ \"$6\" = sh ] && [ \"$7\" = -lc ] || {{ echo bad args >&2; exit 90; }}\ncd \"$4\" || exit 91\nexport HOME={}\nexec sh -c \"$8\"\n", home.display()),
        )
        .unwrap();
        std::fs::set_permissions(&wsl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let prog = wsl.to_str().unwrap();
        let cwd = dir.to_str().unwrap();
        let v = run_rc(
            prog,
            "Ubuntu",
            cwd,
            search_call("it's a test", Some(3)),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(v[0]["args"], "sessions search it's a test --json --limit 3");
        assert_eq!(v[0]["cwd"], cwd);
        let e = run_rc(
            prog,
            "Ubuntu",
            cwd,
            RcCall {
                args: vec!["sessions".into(), "fail".into()],
                stdin: None,
            },
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert_eq!(e, "session not found: x");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
