//! Mode/hook editor commands. On the local host they call `ruckcode_core::modes_api` directly; on
//! WSL2 they run `rc modes ... --json` inside the distro (the document goes in on stdin), so both
//! produce the same JSON.

use std::path::PathBuf;

use ruckcode_core::modes_api::{self, ModesDoc};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::setup::{via_wsl, RcCall, Target};

fn json<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn call(args: &[&str], doc: Option<&Value>) -> RcCall {
    RcCall {
        args: args.iter().map(|a| a.to_string()).collect(),
        stdin: doc.map(|d| Zeroizing::new(d.to_string())),
    }
}

pub fn get_call() -> RcCall {
    call(&["modes", "get", "--json"], None)
}

pub fn validate_call(doc: &Value) -> RcCall {
    call(&["modes", "validate", "--doc-stdin", "--json"], Some(doc))
}

pub fn set_call(doc: &Value, preview: bool) -> RcCall {
    let mut c = call(&["modes", "set", "--doc-stdin", "--json"], Some(doc));
    if preview {
        c.args.push("--preview".into());
    }
    c
}

fn parse_doc(doc: &Value) -> Result<ModesDoc, String> {
    serde_json::from_value(doc.clone()).map_err(|e| format!("invalid modes document: {e}"))
}

async fn local<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

pub async fn get(t: &Target, project: &str) -> Result<Value, String> {
    match t {
        Target::Local => json(modes_api::get(&PathBuf::from(project), None).await),
        Target::Wsl(d) => via_wsl(d, project, get_call(), 60).await,
    }
}

pub async fn validate(t: &Target, project: &str, doc: &Value) -> Result<Value, String> {
    match t {
        Target::Local => {
            let (p, d) = (PathBuf::from(project), parse_doc(doc)?);
            json(local(move || Ok(modes_api::validate(&p, None, &d))).await?)
        }
        Target::Wsl(d) => via_wsl(d, project, validate_call(doc), 30).await,
    }
}

pub async fn preview(t: &Target, project: &str, doc: &Value) -> Result<Value, String> {
    match t {
        Target::Local => {
            let (p, d) = (PathBuf::from(project), parse_doc(doc)?);
            json(local(move || modes_api::preview(&p, None, &d)).await?)
        }
        Target::Wsl(d) => via_wsl(d, project, set_call(doc, true), 30).await,
    }
}

pub async fn apply(t: &Target, project: &str, doc: &Value) -> Result<Value, String> {
    match t {
        Target::Local => {
            let (p, d) = (PathBuf::from(project), parse_doc(doc)?);
            json(local(move || modes_api::apply(&p, None, &d)).await?)
        }
        Target::Wsl(d) => via_wsl(d, project, set_call(doc, false), 30).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rc-app-modes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn doc() -> Value {
        json!({"defaultMode": "a", "modes": [
            {"id": "a", "description": "x", "next": ["b"],
             "hooks": {"post_edit": [{"run": "fmt {files}", "glob": "*.rs"}]}},
            {"id": "b", "tools": ["Read"], "permissions": {"Bash": "ask"}}
        ]})
    }

    #[test]
    fn rc_calls() {
        assert_eq!(get_call().args, ["modes", "get", "--json"]);
        assert!(get_call().stdin.is_none());
        let c = validate_call(&doc());
        assert_eq!(c.args, ["modes", "validate", "--doc-stdin", "--json"]);
        assert!(c.stdin.as_ref().unwrap().contains("\"defaultMode\":\"a\""));
        assert_eq!(
            set_call(&doc(), true).args,
            ["modes", "set", "--doc-stdin", "--json", "--preview"]
        );
        assert_eq!(set_call(&doc(), false).args.len(), 4);
    }

    #[tokio::test]
    async fn local_get_validate_preview_apply() {
        let d = project("local");
        let p = d.to_str().unwrap();
        let v = get(&Target::Local, p).await.unwrap();
        assert_eq!(v["status"], "missing");
        assert!(v["catalog"]["builtinTools"].as_array().unwrap().len() >= 8);

        let issues = validate(&Target::Local, p, &json!({"defaultMode": "q", "modes": []}))
            .await
            .unwrap();
        assert_eq!(issues[0]["severity"], "error");
        assert!(validate(&Target::Local, p, &json!({"modes": 3}))
            .await
            .unwrap_err()
            .contains("invalid modes document"));

        let pv = preview(&Target::Local, p, &doc()).await.unwrap();
        assert!(pv["diff"].as_str().unwrap().contains("+[modes.a]"));
        assert!(!d.join(".ruckcode/modes.toml").exists());
        let r = apply(&Target::Local, p, &doc()).await.unwrap();
        assert_eq!(r["changed"], true);
        assert!(std::fs::read_to_string(d.join(".ruckcode/modes.toml"))
            .unwrap()
            .contains("[[modes.a.hooks.post_edit]]"));
        let v = get(&Target::Local, p).await.unwrap();
        assert_eq!(v["status"], "ready");
        assert_eq!(v["modes"][1]["permissions"]["Bash"], "ask");
        let e = apply(
            &Target::Local,
            p,
            &json!({"defaultMode": "zz", "modes": [{"id": "a"}]}),
        )
        .await
        .unwrap_err();
        assert!(e.contains("default mode"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Stand-in `wsl.exe` running a fake `rc` that records its stdin and arguments.
    #[cfg(unix)]
    #[tokio::test]
    async fn wsl_calls_pass_the_document_on_stdin() {
        use crate::setup::run_rc;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;
        let dir = project("wsl");
        let home = dir.join("home");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        let rc = home.join(".local/bin/rc");
        std::fs::write(
            &rc,
            "#!/bin/sh\nif [ \"$1 $2\" = \"modes bad\" ]; then echo 'rc: modes.toml would be invalid: nope' >&2; exit 1; fi\ncat > \"$HOME/stdin.txt\"\ns=$(tr -d '\\n' < \"$HOME/stdin.txt\")\n[ -z \"$s\" ] && s=null\nprintf '{\"args\":\"%s\",\"stdin\":%s}\\n' \"$*\" \"$s\"\n",
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
        let (prog, cwd) = (wsl.to_str().unwrap(), dir.to_str().unwrap());
        let v = run_rc(
            prog,
            "Ubuntu",
            cwd,
            set_call(&doc(), true),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(v["args"], "modes set --doc-stdin --json --preview");
        assert_eq!(
            v["stdin"]["modes"][0]["hooks"]["post_edit"][0]["glob"],
            "*.rs"
        );
        let v = run_rc(prog, "Ubuntu", cwd, get_call(), Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(v["args"], "modes get --json");
        let e = run_rc(
            prog,
            "Ubuntu",
            cwd,
            RcCall {
                args: vec!["modes".into(), "bad".into()],
                stdin: None,
            },
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert_eq!(e, "modes.toml would be invalid: nope");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
