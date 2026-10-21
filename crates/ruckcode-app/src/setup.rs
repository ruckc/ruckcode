//! Setup and keys commands. On the local host they call `ruckcode_core::setup_api` directly; on
//! WSL2 they run the equivalent `rc` subcommands inside the distro (JSON on stdout, secrets on
//! stdin only, never on a command line). The functions that build command lines and stdin are pure
//! and tested on any OS.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use ruckcode_core::setup_api::{self, InitInput, ModelsAuth, PlanInput};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use zeroize::Zeroizing;

use crate::wsl;

/// Where a request runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Local,
    /// Inside this WSL2 distro (the project is a Linux path).
    Wsl(String),
}

impl Target {
    pub fn from_distro(distro: Option<String>) -> Self {
        match distro.filter(|d| !d.is_empty()) {
            Some(d) => Target::Wsl(d),
            None => Target::Local,
        }
    }
}

fn json<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

// ---- command lines for `rc` inside WSL (pure) ---------------------------------------------

/// `sh -lc` script that runs `rc` with `args` (all quoted) with `~/.local/bin` on the PATH.
pub fn rc_script(args: &[String]) -> String {
    let quoted: Vec<String> = args.iter().map(|a| wsl::sh_quote(a)).collect();
    format!("{}; exec rc {}", wsl::PATH_PREFIX, quoted.join(" "))
}

/// Argument vector for `wsl.exe` that runs `rc <args>` in `distro`, starting in `linux_cwd`.
pub fn wsl_rc_args(distro: &str, linux_cwd: &str, args: &[String]) -> Vec<String> {
    wsl::exec_args(distro, Some(linux_cwd), &rc_script(args))
}

fn s(a: &[&str]) -> Vec<String> {
    a.iter().map(|x| x.to_string()).collect()
}

/// `rc` arguments and stdin for each operation. Stdin is only ever used for secrets or the plan.
pub struct RcCall {
    pub args: Vec<String>,
    pub stdin: Option<Zeroizing<String>>,
}

pub fn state_call() -> RcCall {
    RcCall {
        args: s(&["setup", "--json-state"]),
        stdin: None,
    }
}

pub fn setup_call(plan: &PlanInput, preview: bool) -> Result<RcCall, String> {
    let mut args = s(&["setup", "--plan-stdin", "--json"]);
    if preview {
        args.push("--preview".into());
    }
    let text = serde_json::to_string(plan).map_err(|e| e.to_string())?;
    Ok(RcCall {
        args,
        stdin: Some(Zeroizing::new(text)),
    })
}

pub fn models_call(kind: &str, base_url: &str, auth: Option<&ModelsAuth>) -> RcCall {
    let mut args = s(&["models", "--provider", kind, "--json"]);
    if !base_url.trim().is_empty() {
        args.extend(s(&["--base-url", base_url.trim()]));
    }
    let mut stdin = None;
    match auth {
        None => {}
        Some(ModelsAuth::KeyName(n)) => args.extend(s(&["--api-key-name", n])),
        Some(ModelsAuth::Secret(k)) => {
            args.push("--api-key-stdin".into());
            stdin = Some(Zeroizing::new(format!("{k}\n")));
        }
    }
    RcCall { args, stdin }
}

pub fn keys_list_call() -> RcCall {
    RcCall {
        args: s(&["keys", "list", "--json"]),
        stdin: None,
    }
}

pub fn keys_add_call(name: &str, secret: &str, description: Option<&str>) -> RcCall {
    let mut args = s(&["keys", "add", name, "--api-key-stdin", "--json"]);
    if let Some(d) = description.filter(|d| !d.is_empty()) {
        args.extend(s(&["--description", d]));
    }
    RcCall {
        args,
        stdin: Some(Zeroizing::new(format!("{secret}\n"))),
    }
}

pub fn keys_remove_call(name: &str) -> RcCall {
    RcCall {
        args: s(&["keys", "remove", name, "--json"]),
        stdin: None,
    }
}

pub fn keys_use_call(name: &str, profile: Option<&str>, project_scope: bool) -> RcCall {
    let mut args = s(&["keys", "use", name, "--json"]);
    if let Some(p) = profile.filter(|p| !p.is_empty()) {
        args.extend(s(&["--profile", p]));
    }
    if project_scope {
        args.push("--project".into());
    }
    RcCall { args, stdin: None }
}

pub fn init_call(opts: &InitInput) -> RcCall {
    let mut args = s(&["init", "--json"]);
    if opts.minimal {
        args.push("--minimal".into());
    }
    if opts.force {
        args.push("--force".into());
    }
    if !opts.modes.is_empty() {
        args.extend(s(&["--modes", &opts.modes.join(",")]));
    }
    RcCall { args, stdin: None }
}

// ---- running rc in WSL ----------------------------------------------------------------------

/// The JSON document on stdout: the last line that is an object or array and parses (login
/// shells may print banners before `rc` runs).
pub fn parse_json_output(stdout: &str) -> Result<Value, String> {
    let mut candidate = false;
    for line in stdout.lines().rev() {
        let line = line.trim();
        if line.starts_with('{') || line.starts_with('[') {
            candidate = true;
            if let Ok(v) = serde_json::from_str(line) {
                return Ok(v);
            }
        }
    }
    Err(if candidate {
        "rc printed invalid JSON".to_string()
    } else {
        format!("rc printed no JSON: {}", stdout.trim())
    })
}

/// The message of a failed `rc`: its `rc: ...` line on stderr when there is one.
pub fn failure_message(stderr: &str, code: Option<i32>) -> String {
    let line = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line = line.strip_prefix("rc: ").unwrap_or(line);
    if line.is_empty() {
        format!(
            "rc exited with code {}",
            code.map_or("?".into(), |c| c.to_string())
        )
    } else {
        line.to_string()
    }
}

pub async fn run_rc(
    program: &str,
    distro: &str,
    cwd: &str,
    call: RcCall,
    timeout: Duration,
) -> Result<Value, String> {
    let mut cmd = wsl::wsl_command(program);
    cmd.args(wsl_rc_args(distro, cwd, &call.args))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run `{program}`: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(text) = &call.stdin {
            // A failed write shows up as the command's own error below.
            let _ = stdin.write_all(text.as_bytes()).await;
        }
        drop(stdin);
    }
    drop(call);
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("rc timed out after {}s", timeout.as_secs()))?
        .map_err(|e| format!("failed to run rc: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        return Err(failure_message(&stderr, out.status.code()));
    }
    parse_json_output(&stdout)
}

fn wsl_program() -> Result<&'static str, String> {
    if cfg!(windows) {
        Ok("wsl.exe")
    } else {
        Err("WSL2 is only available on Windows".into())
    }
}

pub(crate) async fn via_wsl(
    distro: &str,
    project: &str,
    call: RcCall,
    secs: u64,
) -> Result<Value, String> {
    run_rc(
        wsl_program()?,
        distro,
        project,
        call,
        Duration::from_secs(secs),
    )
    .await
}

// ---- operations -------------------------------------------------------------------------------

pub fn presets() -> Result<Value, String> {
    json(setup_api::suggestions())
}

pub async fn state(t: &Target, project: &str) -> Result<Value, String> {
    match t {
        Target::Local => state_local(project, None),
        Target::Wsl(d) => via_wsl(d, project, state_call(), 30).await,
    }
}

pub fn state_local(project: &str, user_dir: Option<&Path>) -> Result<Value, String> {
    json(setup_api::state(Path::new(project), user_dir))
}

pub async fn list_models(
    t: &Target,
    project: &str,
    kind: &str,
    base_url: &str,
    auth: Option<ModelsAuth>,
) -> Result<Value, String> {
    match t {
        Target::Local => {
            let models = setup_api::list_models(kind, base_url, auth, None)
                .await
                .map_err(err)?;
            Ok(serde_json::json!({ "models": models }))
        }
        Target::Wsl(d) => {
            let call = models_call(kind, base_url, auth.as_ref());
            drop(auth);
            via_wsl(d, project, call, 45).await
        }
    }
}

pub async fn setup_plan(
    t: &Target,
    project: &str,
    plan: PlanInput,
    preview: bool,
) -> Result<Value, String> {
    match t {
        Target::Local => setup_plan_local(project, plan, preview, None),
        Target::Wsl(d) => {
            let call = setup_call(&plan, preview)?;
            drop(plan);
            via_wsl(d, project, call, 60).await
        }
    }
}

pub fn setup_plan_local(
    project: &str,
    plan: PlanInput,
    preview: bool,
    user_dir: Option<&Path>,
) -> Result<Value, String> {
    let project = Path::new(project);
    if preview {
        json(setup_api::preview(plan, user_dir, project).map_err(err)?)
    } else {
        json(setup_api::apply(plan, user_dir, project).map_err(err)?)
    }
}

pub async fn keys_list(t: &Target, project: &str) -> Result<Value, String> {
    match t {
        Target::Local => json(setup_api::keys_list(None, Path::new(project)).map_err(err)?),
        Target::Wsl(d) => via_wsl(d, project, keys_list_call(), 30).await,
    }
}

pub async fn keys_add(
    t: &Target,
    project: &str,
    name: &str,
    secret: Zeroizing<String>,
    description: Option<&str>,
) -> Result<Value, String> {
    match t {
        Target::Local => json(setup_api::keys_add(None, name, &secret, description).map_err(err)?),
        Target::Wsl(d) => {
            let call = keys_add_call(name, &secret, description);
            drop(secret);
            via_wsl(d, project, call, 30).await
        }
    }
}

pub async fn keys_remove(t: &Target, project: &str, name: &str) -> Result<Value, String> {
    match t {
        Target::Local => json(setup_api::keys_remove(None, Path::new(project), name).map_err(err)?),
        Target::Wsl(d) => via_wsl(d, project, keys_remove_call(name), 30).await,
    }
}

pub async fn keys_use(
    t: &Target,
    project: &str,
    name: &str,
    profile: Option<&str>,
    project_scope: bool,
) -> Result<Value, String> {
    match t {
        Target::Local => json(
            setup_api::keys_use(None, Path::new(project), name, profile, project_scope)
                .map_err(err)?,
        ),
        Target::Wsl(d) => {
            via_wsl(d, project, keys_use_call(name, profile, project_scope), 30).await
        }
    }
}

pub async fn init_modes(t: &Target, project: &str, opts: InitInput) -> Result<Value, String> {
    match t {
        Target::Local => json(
            setup_api::init_modes(Path::new(project), &opts)
                .await
                .map_err(err)?,
        ),
        Target::Wsl(d) => via_wsl(d, project, init_call(&opts), 120).await,
    }
}

/// `rc models --probe`: detect the API flavor of `base_url` with the key (never stored).
pub fn probe_call(base_url: &str, force: Option<&str>, auth: Option<&ModelsAuth>) -> RcCall {
    let mut args = s(&["models", "--probe", "--json", "--base-url", base_url.trim()]);
    if let Some(k) = force.filter(|k| !k.is_empty()) {
        args.extend(s(&["--provider", k]));
    }
    let mut stdin = None;
    match auth {
        None => {}
        Some(ModelsAuth::KeyName(n)) => args.extend(s(&["--api-key-name", n])),
        Some(ModelsAuth::Secret(k)) => {
            args.push("--api-key-stdin".into());
            stdin = Some(Zeroizing::new(format!("{k}\n")));
        }
    }
    RcCall { args, stdin }
}

/// Probe an endpoint with the entered key: inside the WSL distro for a WSL project, so the
/// network path and the distro's own saved keys match where the agent will run.
pub async fn probe(
    t: &Target,
    project: &str,
    base_url: &str,
    force: Option<&str>,
    auth: Option<ModelsAuth>,
) -> Result<Value, String> {
    match t {
        Target::Local => {
            let r = setup_api::probe(base_url, auth, None, force)
                .await
                .map_err(err)?;
            json(r)
        }
        Target::Wsl(d) => {
            let call = probe_call(base_url, force, auth.as_ref());
            drop(auth);
            via_wsl(d, project, call, 60).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_quotes_and_prefixes_path() {
        let a = rc_script(&s(&["keys", "add", "my key", "--description", "it's"]));
        assert_eq!(
            a,
            "PATH=$HOME/.local/bin:$PATH; exec rc keys add 'my key' --description 'it'\\''s'"
        );
        let v = wsl_rc_args(
            "Ubuntu-24.04",
            "/home/me/p q",
            &s(&["setup", "--json-state"]),
        );
        assert_eq!(
            &v[..6],
            ["-d", "Ubuntu-24.04", "--cd", "/home/me/p q", "--exec", "sh"]
        );
        assert_eq!(v[6], "-lc");
        assert_eq!(
            wsl::unwrap_script_for_tests(&v[7]),
            "PATH=$HOME/.local/bin:$PATH; exec rc setup --json-state"
        );
        assert!(!v.iter().any(|a| a == "--"));
    }

    fn plan(secret: bool) -> PlanInput {
        serde_json::from_value(serde_json::json!({
            "profileName": "p", "provider": "openai", "baseUrl": "http://localhost:11434/v1", "model": "m", "models": ["m"],
            "newKey": secret.then(|| serde_json::json!({"name":"k","secret":"sk-TOPSECRET"})),
        }))
        .unwrap()
    }

    #[test]
    fn secrets_go_to_stdin_never_argv() {
        let c = models_call(
            "openai",
            " http://h:1/v1 ",
            Some(&ModelsAuth::Secret("sk-TOPSECRET".into())),
        );
        assert_eq!(
            c.args,
            [
                "models",
                "--provider",
                "openai",
                "--json",
                "--base-url",
                "http://h:1/v1",
                "--api-key-stdin"
            ]
        );
        assert_eq!(
            c.stdin.as_deref().map(String::as_str),
            Some("sk-TOPSECRET\n")
        );
        let c = models_call("anthropic", "", None);
        assert_eq!(c.args, ["models", "--provider", "anthropic", "--json"]);
        assert!(c.stdin.is_none());
        let c = probe_call(
            " http://h:1/v1 ",
            None,
            Some(&ModelsAuth::Secret("sk-X".into())),
        );
        assert_eq!(
            c.args,
            [
                "models",
                "--probe",
                "--json",
                "--base-url",
                "http://h:1/v1",
                "--api-key-stdin"
            ]
        );
        assert_eq!(c.stdin.as_deref().map(String::as_str), Some("sk-X\n"));
        assert!(!c.args.join(" ").contains("sk-X"));
        let c = probe_call(
            "http://h",
            Some("anthropic"),
            Some(&ModelsAuth::KeyName("k".into())),
        );
        assert!(c
            .args
            .ends_with(&s(&["--provider", "anthropic", "--api-key-name", "k"])));
        let c = models_call("anthropic", "", Some(&ModelsAuth::KeyName("k".into())));
        assert!(c.args.ends_with(&s(&["--api-key-name", "k"])));
        assert!(models_call("openai", "", None).stdin.is_none());

        let c = keys_add_call("work", "sk-TOPSECRET", Some("desc"));
        assert_eq!(
            c.args,
            [
                "keys",
                "add",
                "work",
                "--api-key-stdin",
                "--json",
                "--description",
                "desc"
            ]
        );
        assert_eq!(
            c.stdin.as_deref().map(String::as_str),
            Some("sk-TOPSECRET\n")
        );
        assert!(!keys_add_call("w", "x", Some(""))
            .args
            .contains(&"--description".to_string()));

        for preview in [false, true] {
            let c = setup_call(&plan(true), preview).unwrap();
            assert!(
                c.args.iter().all(|a| !a.contains("TOPSECRET")),
                "{:?}",
                c.args
            );
            assert_eq!(c.args.contains(&"--preview".to_string()), preview);
            assert!(c.stdin.as_ref().unwrap().contains("sk-TOPSECRET"));
            let back: Value = serde_json::from_str(c.stdin.as_ref().unwrap()).unwrap();
            assert_eq!(back["newKey"]["name"], "k");
            assert_eq!(back["profileName"], "p");
        }
    }

    #[test]
    fn other_calls() {
        assert_eq!(state_call().args, ["setup", "--json-state"]);
        assert_eq!(keys_list_call().args, ["keys", "list", "--json"]);
        assert_eq!(
            keys_remove_call("a").args,
            ["keys", "remove", "a", "--json"]
        );
        assert_eq!(
            keys_use_call("a", Some("p"), true).args,
            ["keys", "use", "a", "--json", "--profile", "p", "--project"]
        );
        assert_eq!(
            keys_use_call("a", None, false).args,
            ["keys", "use", "a", "--json"]
        );
        let c = init_call(&InitInput {
            minimal: true,
            modes: vec!["a".into(), "b".into()],
            force: true,
        });
        assert_eq!(
            c.args,
            ["init", "--json", "--minimal", "--force", "--modes", "a,b"]
        );
    }

    #[test]
    fn output_parsing() {
        let v = parse_json_output("Welcome to Ubuntu\n{\"a\":1}\n").unwrap();
        assert_eq!(v["a"], 1);
        let v = parse_json_output("[sudo] hi\n[{\"a\":1},{\"a\":2}]\n").unwrap();
        assert_eq!(v[1]["a"], 2);
        assert_eq!(parse_json_output("[]\n").unwrap(), serde_json::json!([]));
        assert!(parse_json_output("nothing\n").is_err());
        assert!(parse_json_output("{bad\n").is_err());
        assert_eq!(
            failure_message("warn\nrc: HTTP 401: bad key\n", Some(1)),
            "HTTP 401: bad key"
        );
        assert_eq!(failure_message("", Some(3)), "rc exited with code 3");
    }

    // Local commands against a temp config dir.
    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rc-app-setup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn local_state_preview_apply() {
        let user = tmp("user");
        let proj = tmp("proj");
        let p = proj.to_str().unwrap();
        let st = state_local(p, Some(&user)).unwrap();
        assert_eq!(st["needsOnboarding"], true);
        let pv = setup_plan_local(p, plan(true), true, Some(&user)).unwrap();
        assert!(!pv["diff"].as_str().unwrap().contains("TOPSECRET"));
        assert!(!user.join("config.toml").exists());
        let r = setup_plan_local(p, plan(true), false, Some(&user)).unwrap();
        assert!(r["credentialsPath"].is_string());
        let st = state_local(p, Some(&user)).unwrap();
        assert_eq!(st["profiles"][0]["name"], "p");
        assert!(!st.to_string().contains("TOPSECRET"));
        assert!(setup_plan_local(
            p,
            serde_json::from_str(r#"{"profileName":"x y","provider":"openai","model":"m"}"#)
                .unwrap(),
            false,
            Some(&user)
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&proj);
    }

    #[test]
    fn target_from_distro() {
        assert_eq!(Target::from_distro(None), Target::Local);
        assert_eq!(Target::from_distro(Some(String::new())), Target::Local);
        assert_eq!(
            Target::from_distro(Some("U".into())),
            Target::Wsl("U".into())
        );
    }

    /// Stand-in `wsl.exe`: checks the argv shape, runs the script in a fake HOME with a fake `rc`
    /// that echoes its stdin length and args, proving secrets reach `rc` only on stdin.
    #[cfg(unix)]
    #[tokio::test]
    async fn run_rc_end_to_end_with_stand_in() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("wslexe");
        let home = dir.join("home");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        let rc = home.join(".local/bin/rc");
        std::fs::write(
            &rc,
            "#!/bin/sh\nif [ \"$1\" = fail ]; then echo 'rc: boom happened' >&2; exit 1; fi\nIN=$(cat)\nprintf '{\"args\":\"%s\",\"stdin\":\"%s\",\"cwd\":\"%s\"}\\n' \"$*\" \"$IN\" \"$(pwd)\"\n",
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
        let v = run_rc(
            prog,
            "Ubuntu",
            dir.to_str().unwrap(),
            models_call(
                "openai",
                "http://x",
                Some(&ModelsAuth::Secret("sk-TOPSECRET".into())),
            ),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(v["stdin"], "sk-TOPSECRET");
        assert!(!v["args"].as_str().unwrap().contains("TOPSECRET"));
        assert!(v["args"]
            .as_str()
            .unwrap()
            .starts_with("models --provider openai"));
        assert_eq!(v["cwd"], dir.to_str().unwrap());
        let e = run_rc(
            prog,
            "Ubuntu",
            dir.to_str().unwrap(),
            RcCall {
                args: s(&["fail"]),
                stdin: None,
            },
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert_eq!(e, "boom happened");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
