use async_trait::async_trait;
use ruckcode_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
    ToolStatus,
};
use ruckcode_core::transcript::{PermissionOutcome, Record, TranscriptReader, UserSource};
use ruckcode_llm::mock::MockProvider;
use ruckcode_llm::{
    ContentBlock, Message, Provider, Request, Response, Role, StopReason as LlmStop, StreamEvent,
    Usage,
};
use ruckcode_tools::{Answer, Question, UserBridge};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
    decisions: Mutex<VecDeque<PermissionDecision>>,
    requests: Mutex<Vec<PermissionRequest>>,
}

impl TestClient {
    fn with(decisions: impl IntoIterator<Item = PermissionDecision>) -> Arc<Self> {
        let c = Self::default();
        c.decisions.lock().unwrap().extend(decisions);
        Arc::new(c)
    }
    fn events(&self) -> Vec<SessionEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait]
impl UserBridge for TestClient {
    async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
        anyhow::bail!("no user")
    }
}

#[async_trait]
impl Client for TestClient {
    async fn request_permission(&self, _: &str, req: PermissionRequest) -> PermissionDecision {
        self.requests.lock().unwrap().push(req);
        self.decisions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PermissionDecision::AllowOnce)
    }
    fn send_event(&self, _: &str, event: SessionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

struct Env {
    proj: TempDir,
    _user: TempDir,
    user_path: PathBuf,
    mock: Arc<MockProvider>,
}

impl Env {
    fn new(modes: Option<&str>, config: Option<&str>) -> Self {
        let proj = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        if let Some(m) = modes {
            std::fs::write(proj.path().join(".ruckcode/modes.toml"), m).unwrap();
        }
        if let Some(c) = config {
            std::fs::write(proj.path().join(".ruckcode/config.toml"), c).unwrap();
        }
        Self {
            user_path: user.path().to_path_buf(),
            proj,
            _user: user,
            mock: Arc::new(MockProvider::default()),
        }
    }

    fn agent(&self) -> Agent {
        self.agent_with(self.mock.clone())
    }

    fn agent_with(&self, p: Arc<dyn Provider>) -> Agent {
        Agent::new(AgentOptions {
            provider_override: Some(p),
            user_config_dir: Some(self.user_path.clone()),
            ..Default::default()
        })
    }

    fn cwd(&self) -> PathBuf {
        self.proj.path().to_path_buf()
    }

    fn records(&self, id: &str) -> Vec<Record> {
        TranscriptReader::load(&ruckcode_core::transcript::session_path(&self.cwd(), id))
            .unwrap()
            .records
            .into_iter()
            .map(|r| r.record)
            .collect()
    }
}

fn tool_use(id: &str, name: &str, input: Value) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }],
    }
}

fn last_user_text(r: &Request) -> String {
    let m = r.messages.last().unwrap();
    assert_eq!(m.role, Role::User);
    m.content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::ToolResult { content, .. } => content.clone(),
            _ => String::new(),
        })
        .collect()
}

fn tool_names(r: &Request) -> Vec<String> {
    r.tools.iter().map(|t| t.name.clone()).collect()
}

const MODES: &str = r#"
default_mode = "a"
[modes.a]
description = "mode a"
tools = ["Read", "Write"]
next = ["b"]
[modes.b]
description = "mode b"
tools = ["Read", "Grep"]
"#;

#[tokio::test]
async fn simple_text_reply() {
    let env = Env::new(None, None);
    env.mock.push(Message::assistant_text("hello there"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    assert_eq!(info.current_mode, "default");
    let client = TestClient::with([]);
    let r = agent
        .prompt(&info.id, "hi".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);
    assert!(client
        .events()
        .contains(&SessionEvent::AgentText("hello there".into())));

    let recs = env.records(&info.id);
    let types: Vec<&str> = recs
        .iter()
        .map(|r| match r {
            Record::SessionStart(_) => "session_start",
            Record::UserMessage { .. } => "user_message",
            Record::AssistantMessage { .. } => "assistant_message",
            Record::TurnEnd { .. } => "turn_end",
            _ => "other",
        })
        .collect();
    assert_eq!(
        types,
        [
            "session_start",
            "user_message",
            "assistant_message",
            "turn_end"
        ]
    );
    let Record::AssistantMessage {
        metrics,
        request_id,
        ..
    } = &recs[2]
    else {
        panic!()
    };
    assert_eq!(*request_id, 1);
    assert!(metrics.time_to_first_token_ms.is_some());
    let Record::TurnEnd {
        model_requests,
        stop_reason,
        ..
    } = &recs[3]
    else {
        panic!()
    };
    assert_eq!((*model_requests, stop_reason.as_str()), (1, "end_turn"));
    let raw = std::fs::read_to_string(ruckcode_core::transcript::session_path(
        &env.cwd(),
        &info.id,
    ))
    .unwrap();
    assert!(raw
        .lines()
        .all(|l| l.contains("\"ts\":\"") && l.contains("\"type\":\"")));
    let sys = &env.mock.requests.lock().unwrap()[0].system;
    assert!(sys.contains("Current mode: Default"));
}

#[tokio::test]
async fn write_tool_asks_and_creates_file() {
    let env = Env::new(None, None);
    env.mock.push(tool_use(
        "t1",
        "Write",
        json!({"file_path": "a.txt", "content": "hi\n"}),
    ));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::AllowOnce]);
    let r = agent
        .prompt(&info.id, "write".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);
    assert_eq!(
        std::fs::read_to_string(env.proj.path().join("a.txt")).unwrap(),
        "hi\n"
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);

    let ev = client.events();
    assert!(ev
        .iter()
        .any(|e| matches!(e, SessionEvent::ToolCall { id, .. } if id == "t1")));
    let done = ev
        .iter()
        .find_map(|e| match e {
            SessionEvent::ToolCallUpdate {
                status: ToolStatus::Completed,
                edits,
                ..
            } => Some(edits),
            _ => None,
        })
        .unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].new_text, "hi\n");

    let recs = env.records(&info.id);
    let result = recs
        .iter()
        .find_map(|r| match r {
            Record::ToolResult {
                permission, edits, ..
            } => Some((permission, edits)),
            _ => None,
        })
        .unwrap();
    assert_eq!(*result.0, PermissionOutcome::AskApproved);
    assert!(result.1[0].diff.contains("+hi"));
    assert!(recs.iter().any(|r| matches!(
        r,
        Record::UserMessage {
            source: UserSource::ToolResults,
            ..
        }
    )));
    // second request carries the tool result back to the model
    assert_eq!(env.mock.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn allow_always_skips_second_prompt() {
    let env = Env::new(None, None);
    for i in 0..2 {
        env.mock.push(tool_use(
            &format!("t{i}"),
            "Write",
            json!({"file_path": format!("{i}.txt"), "content": "x"}),
        ));
    }
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::AllowAlways]);
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert!(env.proj.path().join("1.txt").exists());
}

#[tokio::test]
async fn deny_by_mode_policy_and_user_reject() {
    let modes = r#"
[modes.a]
tools = ["Write", "Bash"]
[modes.a.permissions]
Write = "deny"
"#;
    let env = Env::new(Some(modes), None);
    env.mock.push(tool_use(
        "t1",
        "Write",
        json!({"file_path": "no.txt", "content": "x"}),
    ));
    env.mock
        .push(tool_use("t2", "Bash", json!({"command": "touch nope"})));
    env.mock.push(Message::assistant_text("ok"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::Reject]);
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert!(!env.proj.path().join("no.txt").exists());
    assert!(!env.proj.path().join("nope").exists());
    // only Bash asked
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    let outcomes: Vec<(PermissionOutcome, String)> = env
        .records(&info.id)
        .into_iter()
        .filter_map(|r| match r {
            Record::ToolResult {
                permission,
                output,
                is_error: true,
                ..
            } => Some((permission, output)),
            _ => None,
        })
        .collect();
    assert_eq!(
        outcomes[0],
        (PermissionOutcome::Denied, "denied by policy".into())
    );
    assert_eq!(
        outcomes[1],
        (PermissionOutcome::Rejected, "user rejected".into())
    );
}

#[tokio::test]
async fn post_edit_fix_turn() {
    let modes = r#"
[modes.a]
tools = ["Write"]
[modes.a.permissions]
Write = "allow"
[[modes.a.hooks.post_edit]]
run = "test -f ok.txt"
on_failure = "fix_turn"
"#;
    let env = Env::new(Some(modes), None);
    env.mock.push(tool_use(
        "t1",
        "Write",
        json!({"file_path": "a.txt", "content": "x"}),
    ));
    env.mock.push(Message::assistant_text("first attempt done"));
    env.mock.push(tool_use(
        "t2",
        "Write",
        json!({"file_path": "ok.txt", "content": "y"}),
    ));
    env.mock.push(Message::assistant_text("all good"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([]);
    let r = agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);

    let reqs = env.mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    let fix = last_user_text(&reqs[2]);
    assert!(
        fix.contains("hook commands failed") && fix.contains("test -f ok.txt"),
        "{fix}"
    );
    drop(reqs);
    assert!(client
        .events()
        .iter()
        .any(|e| matches!(e, SessionEvent::Notice(n) if n.contains("starting fix turn 1/2"))));

    let recs = env.records(&info.id);
    let hooks: Vec<bool> = recs
        .iter()
        .filter_map(|r| match r {
            Record::HookRun { event, success, .. } if event == "post_edit" => Some(*success),
            _ => None,
        })
        .collect();
    assert_eq!(hooks, [false, true]);
    assert!(recs.iter().any(|r| matches!(
        r,
        Record::UserMessage {
            source: UserSource::FixTurn,
            ..
        }
    )));
}

#[tokio::test]
async fn switch_mode_with_approval_and_auto_advance() {
    let env = Env::new(Some(MODES), None);
    env.mock.push(tool_use(
        "t1",
        "SwitchMode",
        json!({"mode_id": "b", "reason": "done here"}),
    ));
    env.mock.push(Message::assistant_text("now in b"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::AllowOnce]);
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();

    let req = client.requests.lock().unwrap()[0].clone();
    assert_eq!(req.title, "Switch to B mode: done here");
    assert!(!req.allow_always);
    assert!(client
        .events()
        .contains(&SessionEvent::ModeChanged("b".into())));
    assert_eq!(agent.session_modes(&info.id).unwrap().current_mode, "b");
    {
        let reqs = env.mock.requests.lock().unwrap();
        assert_eq!(tool_names(&reqs[0]), ["Read", "Write", "SwitchMode"]);
        assert_eq!(tool_names(&reqs[1]), ["Read", "Grep", "SwitchMode"]);
        assert!(reqs[1].system.contains("Current mode: B"));
    }
    assert!(env.records(&info.id).iter().any(|r| matches!(r,
        Record::ModeChange { from, to, .. } if from == "a" && to == "b")));

    // auto-advance skips confirmation
    let auto = MODES.replace(
        "next = [\"b\"]",
        "next = [\"b\"]\nauto_advance_to = [\"b\"]",
    );
    let env = Env::new(Some(&auto), None);
    env.mock.push(tool_use(
        "t1",
        "SwitchMode",
        json!({"mode_id": "b", "reason": "r"}),
    ));
    env.mock.push(Message::assistant_text("ok"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::Reject]);
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert!(client.requests.lock().unwrap().is_empty());
    assert_eq!(agent.session_modes(&info.id).unwrap().current_mode, "b");

    // user-initiated switch back
    agent.set_mode(&info.id, "a", client.clone()).await.unwrap();
    assert_eq!(agent.session_modes(&info.id).unwrap().current_mode, "a");
    assert!(agent.set_mode(&info.id, "zzz", client).await.is_err());
    assert!(env.records(&info.id).iter().any(|r| matches!(r,
        Record::ModeChange { to, initiated_by: ruckcode_core::transcript::Initiator::User, .. } if to == "a")));
}

#[tokio::test]
async fn switch_mode_rejected_stays() {
    let env = Env::new(Some(MODES), None);
    env.mock.push(tool_use(
        "t1",
        "SwitchMode",
        json!({"mode_id": "b", "reason": "r"}),
    ));
    env.mock.push(Message::assistant_text("ok"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::Reject]);
    agent.prompt(&info.id, "go".into(), client).await.unwrap();
    assert_eq!(agent.session_modes(&info.id).unwrap().current_mode, "a");
}

#[tokio::test]
async fn resume_restores_history_and_repairs_dangling_tool_use() {
    let env = Env::new(None, None);
    let thinking = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                thinking: "pondering".into(),
                signature: Some("sig1".into()),
            },
            ContentBlock::Text {
                text: "answer".into(),
            },
        ],
    };
    env.mock.push(thinking.clone());
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([]);
    agent
        .prompt(&info.id, "first".into(), client)
        .await
        .unwrap();

    // Simulate a crash after a tool_use was recorded.
    let path = ruckcode_core::transcript::session_path(&env.cwd(), &info.id);
    let mut w = ruckcode_core::transcript::TranscriptWriter::open(&path).unwrap();
    w.append(&Record::UserMessage {
        message: Message::user_text("second"),
        source: UserSource::User,
    })
    .unwrap();
    w.append(&Record::AssistantMessage {
        message: tool_use("dangling", "Read", json!({"file_path": "x"})),
        request_id: 2,
        model: "m".into(),
        stop_reason: LlmStop::ToolUse,
        usage: Usage::default(),
        metrics: ruckcode_core::transcript::Metrics {
            duration_ms: 1,
            time_to_first_token_ms: None,
            output_tokens_per_sec: None,
        },
    })
    .unwrap();
    drop(w);

    let env2_mock = Arc::new(MockProvider::new([Message::assistant_text("resumed")]));
    let agent2 = env.agent_with(env2_mock.clone());
    let client2 = TestClient::with([]);
    let info2 = agent2
        .load_session(&info.id, env.cwd(), client2.clone())
        .await
        .unwrap();
    assert_eq!(info2.id, info.id);
    let ev = client2.events();
    assert!(ev.contains(&SessionEvent::UserText("first".into())));
    assert!(ev.contains(&SessionEvent::AgentThought("pondering".into())));
    assert!(ev.contains(&SessionEvent::AgentText("answer".into())));

    agent2
        .prompt(&info.id, "third".into(), client2)
        .await
        .unwrap();
    let reqs = env2_mock.requests.lock().unwrap();
    let msgs = &reqs[0].messages;
    assert_eq!(msgs[1], thinking);
    let ContentBlock::ToolResult {
        tool_use_id,
        is_error,
        content,
    } = &msgs[4].content[0]
    else {
        panic!("{:?}", msgs[4])
    };
    assert_eq!((tool_use_id.as_str(), *is_error), ("dangling", true));
    assert!(content.contains("interrupted"));
    assert_eq!(msgs.last().unwrap().text(), "third");
    drop(reqs);

    let recs = env.records(&info.id);
    assert!(recs.iter().any(|r| matches!(r, Record::SessionResumed {})));
    let listed = agent2.list_sessions(&env.cwd()).unwrap();
    assert_eq!(listed[0].first_prompt, "first");
}

struct Slow;

#[async_trait]
impl Provider for Slow {
    fn name(&self) -> &str {
        "slow"
    }
    async fn complete(
        &self,
        _: Request,
        _: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response> {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        unreachable!()
    }
}

#[tokio::test]
async fn cancellation_during_model_request() {
    let env = Env::new(None, None);
    let agent = Arc::new(env.agent_with(Arc::new(Slow)));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let a = agent.clone();
    let id = info.id.clone();
    let task = tokio::spawn(async move { a.prompt(&id, "go".into(), TestClient::with([])).await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    agent.cancel(&info.id);
    let r = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(r, StopReason::Cancelled);
    assert!(env.records(&info.id).iter().any(|r| matches!(r,
        Record::TurnEnd { stop_reason, .. } if stop_reason == "cancelled")));
}

#[tokio::test]
async fn permission_cancel_aborts_with_valid_history() {
    let env = Env::new(None, None);
    let two = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::ToolUse {
                id: "a".into(),
                name: "Write".into(),
                input: json!({"file_path": "a", "content": "1"}),
            },
            ContentBlock::ToolUse {
                id: "b".into(),
                name: "Write".into(),
                input: json!({"file_path": "b", "content": "2"}),
            },
        ],
    };
    env.mock.push(two);
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::Cancelled]);
    let r = agent.prompt(&info.id, "go".into(), client).await.unwrap();
    assert_eq!(r, StopReason::Cancelled);
    let loaded = TranscriptReader::load(&ruckcode_core::transcript::session_path(
        &env.cwd(),
        &info.id,
    ))
    .unwrap();
    let last = loaded.messages.last().unwrap();
    assert_eq!(last.content.len(), 2);
    assert!(loaded.dangling_tool_uses().is_empty());
}

#[tokio::test]
async fn compaction_replaces_history() {
    let env = Env::new(None, Some("compaction_threshold = 0.0001"));
    env.mock.push(Message::assistant_text("r1"));
    env.mock.push(Message::assistant_text("r2"));
    env.mock.push(Message::assistant_text("THE SUMMARY"));
    env.mock.push(Message::assistant_text("r3"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    for p in ["p1", "p2", "p3"] {
        agent
            .prompt(&info.id, p.into(), TestClient::with([]))
            .await
            .unwrap();
    }
    let reqs = env.mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    assert!(reqs[2].tools.is_empty());
    let m = &reqs[3].messages;
    assert_eq!(m.len(), 2);
    assert!(m[0].text().contains("THE SUMMARY"));
    drop(reqs);
    let loaded = TranscriptReader::load(&ruckcode_core::transcript::session_path(
        &env.cwd(),
        &info.id,
    ))
    .unwrap();
    assert_eq!(loaded.messages.len(), 3);
    assert!(loaded.records.iter().any(|r| matches!(
        &r.record,
        Record::Compaction {
            replaced_messages: 4,
            ..
        }
    )));
}
