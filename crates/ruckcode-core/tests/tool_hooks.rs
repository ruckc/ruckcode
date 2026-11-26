//! Pins the behaviour that the `Tool` hooks (`plan`, `auto_allow`, `lean_class`) must keep:
//! plan replay from old transcripts, which tools skip the permission prompt, and which tools a
//! lean profile offers.

use async_trait::async_trait;
use ruck_harness_llm::mock::MockProvider;
use ruck_harness_llm::{ContentBlock, Message, Provider, StopReason as LlmStop, Usage};
use ruckcode_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent,
};
use ruckcode_core::transcript::{
    assess_unfinished, Metrics, Record, TranscriptReader, TranscriptWriter, UserSource,
};
use ruckcode_tools::{Answer, Question, UserBridge};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
    requests: Mutex<Vec<PermissionRequest>>,
}

#[async_trait]
impl UserBridge for TestClient {
    async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
        Ok(vec![])
    }
}

#[async_trait]
impl Client for TestClient {
    async fn request_permission(&self, _: &str, req: PermissionRequest) -> PermissionDecision {
        self.requests.lock().unwrap().push(req);
        PermissionDecision::AllowOnce
    }
    fn send_event(&self, _: &str, event: SessionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

struct Env {
    proj: TempDir,
    user: TempDir,
}

impl Env {
    fn new(config: Option<&str>) -> Self {
        let proj = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        if let Some(c) = config {
            std::fs::write(proj.path().join(".ruckcode/config.toml"), c).unwrap();
        }
        Self { proj, user }
    }
    fn cwd(&self) -> PathBuf {
        self.proj.path().to_path_buf()
    }
    fn agent(&self, p: Arc<dyn Provider>) -> Agent {
        Agent::new(AgentOptions {
            provider_override: Some(p),
            user_config_dir: Some(self.user.path().to_path_buf()),
            ..Default::default()
        })
    }
}

fn tool_use(id: &str, name: &str, input: Value) -> Message {
    Message {
        role: ruck_harness_llm::Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }],
    }
}

fn assistant_record(message: Message, request_id: u64) -> Record {
    Record::AssistantMessage {
        message,
        request_id,
        model: "m".into(),
        stop_reason: LlmStop::ToolUse,
        usage: Usage::default(),
        cost_usd: None,
        metrics: Metrics {
            duration_ms: 1,
            time_to_first_token_ms: None,
            output_tokens_per_sec: None,
        },
    }
}

fn plan(items: &[(&str, &str)]) -> Value {
    json!({"todos": items.iter().map(|(c, s)| json!({"content": c, "status": s})).collect::<Vec<_>>()})
}

/// An old transcript: several plan updates, a repeat (no-op), a malformed update, completion.
fn old_plan_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("p1", plan(&[("a", "pending"), ("b", "pending")])),
        ("p2", plan(&[("a", "in_progress"), ("b", "pending")])),
        ("p3", plan(&[("a", "in_progress"), ("b", "pending")])),
        ("p4", json!({"todos": "not a list"})),
        ("p5", plan(&[("a", "completed"), ("b", "in_progress")])),
        ("p6", plan(&[("a", "completed"), ("b", "completed")])),
    ]
}

type PlanShape = Vec<(String, String)>;

fn expected_plan_events() -> Vec<PlanShape> {
    let p = |v: &[(&str, &str)]| -> PlanShape {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    };
    vec![
        p(&[("a", "pending"), ("b", "pending")]),
        p(&[("a", "in_progress"), ("b", "pending")]),
        p(&[("a", "in_progress"), ("b", "pending")]),
        // p4 is malformed: no plan event.
        p(&[("a", "completed"), ("b", "in_progress")]),
        p(&[("a", "completed"), ("b", "completed")]),
    ]
}

fn plan_events(events: &[SessionEvent]) -> Vec<PlanShape> {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Plan(items) => Some(
                items
                    .iter()
                    .map(|i| {
                        (
                            i.content.clone(),
                            serde_json::to_value(i.status)
                                .unwrap()
                                .as_str()
                                .unwrap()
                                .to_string(),
                        )
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn resuming_an_old_transcript_replays_the_plan_updates() {
    let env = Env::new(None);
    let agent = env.agent(Arc::new(MockProvider::default()));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let path = ruckcode_core::transcript::session_path(&env.cwd(), &info.id);
    let mut w = TranscriptWriter::open(&path).unwrap();
    w.append(&Record::UserMessage {
        message: Message::user_text("plan it"),
        source: UserSource::User,
    })
    .unwrap();
    for (n, (id, input)) in old_plan_calls().into_iter().enumerate() {
        w.append(&assistant_record(
            tool_use(id, "TodoWrite", input.clone()),
            n as u64 + 1,
        ))
        .unwrap();
        w.append(&Record::ToolCall {
            tool_use_id: id.into(),
            name: "TodoWrite".into(),
            input,
            title: "Update plan".into(),
            kind: "think".into(),
        })
        .unwrap();
    }
    drop(w);

    let agent2 = env.agent(Arc::new(MockProvider::default()));
    let client = Arc::new(TestClient::default());
    agent2
        .load_session(&info.id, env.cwd(), client.clone())
        .await
        .unwrap();
    assert_eq!(
        plan_events(&client.events.lock().unwrap()),
        expected_plan_events()
    );

    // The unfinished-work scan sees the plan as of the last valid update.
    let records = TranscriptReader::load(&path).unwrap().records;
    let all = assess_unfinished(&records);
    assert!(all.open_tasks.is_empty() && all.in_progress.is_none());
    let p5 = records
        .iter()
        .position(
            |r| matches!(&r.record, Record::ToolCall { tool_use_id, .. } if tool_use_id == "p5"),
        )
        .unwrap();
    let before_last = assess_unfinished(&records[..=p5]);
    assert_eq!(before_last.open_tasks, vec!["b".to_string()]);
    assert_eq!(before_last.in_progress.as_deref(), Some("b"));
}

#[tokio::test]
async fn only_the_plan_and_ask_tools_skip_the_permission_prompt() {
    let env = Env::new(None);
    let q = json!({"questions": [{"question": "q?", "header": "h", "options": [
        {"label": "a", "description": "d"}, {"label": "b", "description": "d"}]}]});
    let mock = Arc::new(MockProvider::new([
        tool_use("t1", "TodoWrite", plan(&[("a", "pending")])),
        tool_use("t2", "AskUserQuestion", q),
        tool_use("t3", "Write", json!({"file_path": "x.txt", "content": "x"})),
        tool_use("t4", "Read", json!({"file_path": "x.txt"})),
        Message::assistant_text("done"),
    ]));
    let agent = env.agent(mock);
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    let asked: Vec<String> = client
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.tool_call_id.clone())
        .collect();
    assert_eq!(asked, vec!["t3".to_string()], "only Write prompts");
}

const LEAN: &str = r#"
default_profile = "lean"
[profiles.lean]
provider = "mock"
model = "m"
lean_tools = true
"#;

const PLAIN: &str = r#"
default_profile = "plain"
[profiles.plain]
provider = "mock"
model = "m"
"#;

const LEAN_WEB: &str = r#"
default_profile = "lean"
[profiles.lean]
provider = "mock"
model = "m"
lean_tools = true
tools = ["Read", "WebSearch", "TodoWrite"]
"#;

async fn offered(config: &str) -> Vec<String> {
    let env = Env::new(Some(config));
    let mock = Arc::new(MockProvider::new([Message::assistant_text("hi")]));
    let agent = env.agent(mock.clone());
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    agent
        .prompt(&info.id, "x".into(), Arc::new(TestClient::default()))
        .await
        .unwrap();
    let reqs = mock.requests.lock().unwrap();
    let mut names: Vec<String> = reqs[0].tools.iter().map(|t| t.name.clone()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn lean_profiles_drop_and_opt_in_the_same_tools() {
    let all = offered(PLAIN).await;
    for t in ["TodoWrite", "AskUserQuestion", "WebFetch", "WebSearch"] {
        assert!(all.iter().any(|n| n == t), "{t} in {all:?}");
    }
    let lean = offered(LEAN).await;
    for t in ["TodoWrite", "AskUserQuestion", "WebFetch", "WebSearch"] {
        assert!(!lean.iter().any(|n| n == t), "{t} in {lean:?}");
    }
    assert!(lean.iter().any(|n| n == "Bash") && lean.iter().any(|n| n == "Edit"));
    // The allowlist opts the web tool back in; the dropped tools stay dropped even when listed.
    let lean_web = offered(LEAN_WEB).await;
    assert!(lean_web.iter().any(|n| n == "WebSearch"));
    assert!(!lean_web.iter().any(|n| n == "WebFetch"));
    assert!(!lean_web.iter().any(|n| n == "TodoWrite"));
}
