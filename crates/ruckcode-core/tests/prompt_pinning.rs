//! Pins the full system prompt, byte for byte, across the scenarios that shape it: profile
//! (default, lean, tool allowlists), web on/off, every template mode, project instruction files,
//! a client append and a working directory outside a repository.
//!
//! The prompt is captured from the first model request of a real session. The working directory,
//! date and platform are replaced by placeholders. Regenerate the golden file with
//! `UPDATE_GOLDEN=1 cargo test -p ruckcode-core --test prompt_pinning` and review the diff.

use async_trait::async_trait;
use ruck_harness_llm::mock::MockProvider;
use ruck_harness_tools::{Answer, Question, UserBridge};
use ruckcode_core::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent,
};
use std::sync::Arc;

struct Quiet;

#[async_trait]
impl UserBridge for Quiet {
    async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
        anyhow::bail!("no user")
    }
}

#[async_trait]
impl Client for Quiet {
    async fn request_permission(&self, _: &str, _: PermissionRequest) -> PermissionDecision {
        PermissionDecision::AllowOnce
    }
    fn send_event(&self, _: &str, _: SessionEvent) {}
}

const PLAIN: &str = "default_profile = \"p\"\n[profiles.p]\nprovider = \"mock\"\nmodel = \"m\"\n";
const LEAN: &str =
    "default_profile = \"p\"\n[profiles.p]\nprovider = \"mock\"\nmodel = \"m\"\nlean_tools = true\n";
const FETCH_ONLY: &str = "default_profile = \"p\"\n[profiles.p]\nprovider = \"mock\"\nmodel = \"m\"\ntools = [\"Read\", \"WebFetch\"]\n";
const LEAN_SEARCH: &str = "default_profile = \"p\"\n[profiles.p]\nprovider = \"mock\"\nmodel = \"m\"\nlean_tools = true\ntools = [\"WebSearch\"]\n";
const NO_WEB: &str = "[web]\nenabled = false\n";

struct Scenario {
    name: String,
    config: String,
    modes: Option<&'static str>,
    mode: Option<String>,
    repo: bool,
    files: Vec<(&'static str, &'static str)>,
    append: Option<&'static str>,
}

impl Scenario {
    fn new(name: &str, config: &str) -> Self {
        Self {
            name: name.into(),
            config: config.into(),
            modes: None,
            mode: None,
            repo: false,
            files: vec![],
            append: None,
        }
    }
}

async fn capture(s: &Scenario) -> String {
    let proj = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
    std::fs::write(
        user.path().join("config.toml"),
        "[sandbox]\nenabled = false\n",
    )
    .unwrap();
    std::fs::write(proj.path().join(".ruckcode/config.toml"), &s.config).unwrap();
    if let Some(m) = s.modes {
        std::fs::write(proj.path().join(".ruckcode/modes.toml"), m).unwrap();
    }
    for (f, text) in &s.files {
        let p = proj.path().join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    if s.repo {
        for args in [
            vec!["init", "-q"],
            vec!["symbolic-ref", "HEAD", "refs/heads/pinned-branch"],
            vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            let st = std::process::Command::new("git")
                .args(&args)
                .current_dir(proj.path())
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        }
    }
    let mock = Arc::new(MockProvider::default());
    let agent = Agent::ruckcode(AgentOptions {
        provider_override: Some(mock.clone()),
        user_config_dir: Some(user.path().to_path_buf()),
        ..Default::default()
    });
    let info = agent
        .new_session(proj.path().to_path_buf(), s.mode.clone())
        .await
        .unwrap();
    if let Some(a) = s.append {
        agent
            .set_client_instructions(&info.id, Some(a.into()))
            .await
            .unwrap();
    }
    agent
        .prompt(&info.id, "hi".into(), Arc::new(Quiet))
        .await
        .unwrap();
    let system = mock.requests.lock().unwrap()[0].system.clone();
    let cwd = proj.path().to_string_lossy().into_owned();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    system
        .replace(&cwd, "<CWD>")
        .replace(&format!("Today's date: {today}"), "Today's date: <DATE>")
        .replace(
            &format!("Platform: {}", std::env::consts::OS),
            "Platform: <OS>",
        )
}

fn mode_ids(text: &str) -> Vec<String> {
    let m = ruckcode_core::ModeSet::from_toml_str(text).unwrap();
    m.ids().into_iter().map(String::from).collect()
}

fn scenarios() -> Vec<Scenario> {
    let full = include_str!("../templates/modes.toml");
    let minimal = include_str!("../templates/modes-minimal.toml");
    let mut v = vec![
        Scenario::new("default profile, outside a repository", PLAIN),
        Scenario::new("no profile configured", ""),
        Scenario::new("lean profile", LEAN),
        Scenario::new("tool allowlist with only WebFetch", FETCH_ONLY),
        Scenario::new("lean profile opting in to WebSearch", LEAN_SEARCH),
        Scenario::new("web disabled", &format!("{PLAIN}{NO_WEB}")),
    ];
    let mut s = Scenario::new("inside a repository", PLAIN);
    s.repo = true;
    v.push(s);
    let mut s = Scenario::new("all three instruction files", PLAIN);
    s.files = vec![
        ("AGENTS.md", "  Agents rule one.\n\nAgents rule two.\n\n"),
        ("CLAUDE.md", "Claude rule.\n"),
        (".ruckcode/INSTRUCTIONS.md", "Ruckcode rule.\n"),
    ];
    v.push(s);
    let mut s = Scenario::new("only CLAUDE.md, lean, in a repository", LEAN);
    s.repo = true;
    s.files = vec![("CLAUDE.md", "Claude only.\n")];
    v.push(s);
    let mut s = Scenario::new("client append", PLAIN);
    s.append = Some("  Always answer in French.\nKeep it short.  ");
    v.push(s);
    for id in mode_ids(full) {
        let mut s = Scenario::new(&format!("template full, mode {id}"), PLAIN);
        s.modes = Some(full);
        s.mode = Some(id);
        v.push(s);
    }
    for id in mode_ids(minimal) {
        let mut s = Scenario::new(&format!("template minimal, mode {id}"), LEAN);
        s.modes = Some(minimal);
        s.mode = Some(id);
        s.append = Some("client text");
        v.push(s);
    }
    v
}

#[tokio::test]
async fn system_prompts_are_byte_identical() {
    let mut out = String::new();
    for s in scenarios() {
        let text = capture(&s).await;
        out.push_str(&format!("===== {} =====\n{text}\n<<<END>>>\n", s.name));
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/prompts.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(path, &out).unwrap();
        return;
    }
    let want = std::fs::read_to_string(path).expect("golden file; run with UPDATE_GOLDEN=1");
    assert!(
        out == want,
        "system prompt drifted; diff the output against {path}"
    );
}
