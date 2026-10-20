use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ruckcode_acp::schema::*;
use ruckcode_acp::{spawn_in_process, Connection, Handler, RpcError};
use ruckcode_core::{Agent, AgentOptions};
use ruckcode_llm::mock::MockProvider;
use ruckcode_llm::{ContentBlock as LlmBlock, Message, Provider, Role};
use serde_json::{json, Value};
use tempfile::TempDir;

#[derive(Default)]
struct TestClient {
    updates: Mutex<Vec<SessionNotification>>,
    permission_reqs: Mutex<Vec<RequestPermissionRequest>>,
    /// Option ids to select, in order; `None` entry or empty queue means cancelled.
    selections: Mutex<VecDeque<Option<String>>>,
    ask_reqs: Mutex<Vec<Value>>,
    ask_reply: Mutex<Option<Value>>,
}

impl TestClient {
    fn updates(&self) -> Vec<SessionUpdate> {
        self.updates
            .lock()
            .unwrap()
            .iter()
            .map(|n| n.update.clone())
            .collect()
    }
}

#[async_trait]
impl Handler for TestClient {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            SESSION_REQUEST_PERMISSION => {
                self.permission_reqs
                    .lock()
                    .unwrap()
                    .push(serde_json::from_value(params).unwrap());
                let sel = self.selections.lock().unwrap().pop_front().flatten();
                Ok(match sel {
                    Some(id) => json!({"outcome":{"outcome":"selected","optionId":id}}),
                    None => json!({"outcome":{"outcome":"cancelled"}}),
                })
            }
            ASK_USER_QUESTION => {
                self.ask_reqs.lock().unwrap().push(params);
                Ok(self.ask_reply.lock().unwrap().clone().unwrap())
            }
            m => Err(RpcError::method_not_found(m)),
        }
    }
    async fn notification(&self, method: &str, params: Value) {
        if method == SESSION_UPDATE {
            self.updates
                .lock()
                .unwrap()
                .push(serde_json::from_value(params).unwrap());
        }
    }
}

struct Env {
    proj: TempDir,
    user: TempDir,
    client: Arc<TestClient>,
}

impl Env {
    fn new() -> Self {
        Self {
            proj: tempfile::tempdir().unwrap(),
            user: tempfile::tempdir().unwrap(),
            client: Arc::new(TestClient::default()),
        }
    }
    fn cwd(&self) -> PathBuf {
        self.proj.path().to_path_buf()
    }
    fn agent(&self, p: Arc<dyn Provider>) -> Arc<Agent> {
        Arc::new(Agent::new(AgentOptions {
            provider_override: Some(p),
            user_config_dir: Some(self.user.path().to_path_buf()),
            ..Default::default()
        }))
    }
    fn connect(&self, p: Arc<dyn Provider>) -> Connection {
        spawn_in_process(self.agent(p), self.client.clone())
    }
}

fn tool_use(id: &str, name: &str, input: Value) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![LlmBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }],
    }
}

fn mock(msgs: impl IntoIterator<Item = Message>) -> Arc<MockProvider> {
    Arc::new(MockProvider::new(msgs))
}

async fn init(c: &Connection, ask: bool) -> Value {
    let caps = if ask {
        json!({"_meta":{"ruckcode":{"askUserQuestion":true}}})
    } else {
        json!({})
    };
    c.request(
        INITIALIZE,
        json!({"protocolVersion":1,"clientCapabilities":caps}),
    )
    .await
    .unwrap()
}

async fn new_session(c: &Connection, env: &Env) -> String {
    let r = c
        .request(SESSION_NEW, json!({"cwd":env.cwd(),"mcpServers":[]}))
        .await
        .unwrap();
    r["sessionId"].as_str().unwrap().to_string()
}

async fn prompt(c: &Connection, sid: &str, text: &str) -> Value {
    c.request(
        SESSION_PROMPT,
        json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]}),
    )
    .await
    .unwrap()
}

/// Notifications sent before a response are always handled before the request
/// future resolves, so no polling is needed.
async fn wait_for(mut f: impl FnMut() -> bool) {
    assert!(f(), "condition not met");
}

fn agent_text(u: &[SessionUpdate]) -> String {
    u.iter()
        .filter_map(|u| match u {
            SessionUpdate::AgentMessageChunk {
                content: ContentBlock::Text { text },
            } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn initialize_and_new_session() {
    let env = Env::new();
    let c = env.connect(mock([]));
    let r = init(&c, true).await;
    assert_eq!(r["protocolVersion"], 1);
    assert_eq!(r["agentCapabilities"]["loadSession"], true);
    assert_eq!(
        r["agentCapabilities"]["promptCapabilities"]["embeddedContext"],
        true
    );
    assert_eq!(r["agentCapabilities"]["promptCapabilities"]["image"], false);
    assert_eq!(r["authMethods"], json!([]));
    assert_eq!(r["agentInfo"]["name"], "ruckcode");
    let v = c
        .request(INITIALIZE, json!({"protocolVersion":99}))
        .await
        .unwrap();
    assert_eq!(v["protocolVersion"], 1);
    assert_eq!(c.request(AUTHENTICATE, json!({})).await.unwrap(), json!({}));

    let r = c
        .request(SESSION_NEW, json!({"cwd":env.cwd()}))
        .await
        .unwrap();
    assert!(!r["sessionId"].as_str().unwrap().is_empty());
    assert_eq!(r["modes"]["currentModeId"], "default");
    assert_eq!(r["modes"]["availableModes"][0]["id"], "default");

    assert_eq!(c.request("nope", json!({})).await.unwrap_err().code, -32601);
    assert_eq!(
        c.request("_ruckcode/nope", json!({}))
            .await
            .unwrap_err()
            .code,
        -32601
    );
}

#[tokio::test]
async fn text_prompt_streams() {
    let env = Env::new();
    let c = env.connect(mock([Message::assistant_text("hello there")]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    let r = prompt(&c, &sid, "hi").await;
    assert_eq!(r["stopReason"], "end_turn");
    wait_for(|| agent_text(&env.client.updates()) == "hello there").await;
    assert!(env
        .client
        .updates
        .lock()
        .unwrap()
        .iter()
        .all(|n| n.session_id == sid));

    let l = c
        .request(LIST_SESSIONS, json!({"cwd":env.cwd()}))
        .await
        .unwrap();
    assert_eq!(l["sessions"][0]["id"], sid);
    assert_eq!(l["sessions"][0]["firstPrompt"], "hi");
    assert_eq!(l["sessions"][0]["mode"], "default");
}

const LIST_SESSIONS: &str = "_ruckcode/list_sessions";

#[tokio::test]
async fn write_tool_requests_permission_and_reports_diff() {
    let env = Env::new();
    let file = env.cwd().join("out.txt");
    let c = env.connect(mock([
        tool_use(
            "t1",
            "Write",
            json!({"file_path": file, "content": "new body"}),
        ),
        Message::assistant_text("done"),
    ]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    env.client
        .selections
        .lock()
        .unwrap()
        .push_back(Some("allow_once".into()));
    let r = prompt(&c, &sid, "write it").await;
    assert_eq!(r["stopReason"], "end_turn");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new body");

    let reqs = env.client.permission_reqs.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].tool_call.tool_call_id, "t1");
    assert_eq!(reqs[0].tool_call.kind, Some(ToolKind::Edit));
    assert_eq!(reqs[0].tool_call.status, Some(ToolCallStatus::Pending));
    let kinds: Vec<_> = reqs[0].options.iter().map(|o| o.kind).collect();
    assert_eq!(kinds[0], PermissionOptionKind::AllowOnce);
    assert_eq!(*kinds.last().unwrap(), PermissionOptionKind::RejectOnce);

    wait_for(|| agent_text(&env.client.updates()) == "done").await;
    let u = env.client.updates();
    let call = u
        .iter()
        .position(|u| matches!(u, SessionUpdate::ToolCall(t) if t.tool_call_id == "t1"))
        .expect("tool_call");
    let upd = u
        .iter()
        .position(|u| {
            matches!(u, SessionUpdate::ToolCallUpdate(t) if t.tool_call_id == "t1"
            && t.status == Some(ToolCallStatus::Completed))
        })
        .expect("tool_call_update");
    assert!(call < upd);
    let SessionUpdate::ToolCall(tc) = &u[call] else {
        unreachable!()
    };
    assert_eq!(tc.status, ToolCallStatus::Pending);
    assert_eq!(tc.kind, ToolKind::Edit);
    assert!(tc.raw_input.is_some());
    let SessionUpdate::ToolCallUpdate(tu) = &u[upd] else {
        unreachable!()
    };
    assert!(tu.content.as_ref().unwrap().iter().any(|c| matches!(c,
        ToolCallContent::Diff { path, old_text: None, new_text } if *path == file && new_text == "new body")));
}

#[tokio::test]
async fn write_rejected_and_cancelled() {
    let env = Env::new();
    let file = env.cwd().join("x.txt");
    let c = env.connect(mock([
        tool_use("t1", "Write", json!({"file_path": file, "content": "a"})),
        Message::assistant_text("ok"),
    ]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    env.client
        .selections
        .lock()
        .unwrap()
        .push_back(Some("reject_once".into()));
    let r = prompt(&c, &sid, "w").await;
    assert_eq!(r["stopReason"], "end_turn");
    assert!(!file.exists());
}

#[tokio::test]
async fn ask_user_question_via_extension() {
    let env = Env::new();
    let q = json!({"questions":[{"question":"Which DB?","header":"DB","multiSelect":false,
        "options":[{"label":"sqlite","description":""},{"label":"postgres","description":""}]}]});
    let c = env.connect(mock([
        tool_use("t1", "AskUserQuestion", q),
        Message::assistant_text("thanks"),
    ]));
    init(&c, true).await;
    *env.client.ask_reply.lock().unwrap() = Some(json!({
        "answers":[{"question":"Which DB?","selected":["sqlite"]}],"cancelled":false}));
    let sid = new_session(&c, &env).await;
    let r = prompt(&c, &sid, "go").await;
    assert_eq!(r["stopReason"], "end_turn");
    let asks = env.client.ask_reqs.lock().unwrap().clone();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0]["sessionId"], sid.as_str());
    assert_eq!(asks[0]["questions"][0]["question"], "Which DB?");
    assert_eq!(asks[0]["questions"][0]["options"][1]["label"], "postgres");
    assert!(env.client.permission_reqs.lock().unwrap().is_empty());
    wait_for(|| {
        env.client.updates().iter().any(|u| {
            matches!(u,
            SessionUpdate::ToolCallUpdate(t) if t.status == Some(ToolCallStatus::Completed)
            && format!("{:?}", t.content).contains("sqlite"))
        })
    })
    .await;
}

#[tokio::test]
async fn ask_user_question_fallback_and_cancel() {
    let env = Env::new();
    let q = json!({"questions":[{"question":"Which DB?","options":[{"label":"sqlite"},{"label":"postgres"}]}]});
    let c = env.connect(mock([
        tool_use("t1", "AskUserQuestion", q.clone()),
        Message::assistant_text("one"),
        tool_use("t2", "AskUserQuestion", q),
        Message::assistant_text("two"),
    ]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    env.client
        .selections
        .lock()
        .unwrap()
        .extend([Some("opt-1".to_string()), None]);
    prompt(&c, &sid, "go").await;
    let reqs = env.client.permission_reqs.lock().unwrap().clone();
    assert_eq!(reqs[0].tool_call.title.as_deref(), Some("Which DB?"));
    assert_eq!(reqs[0].tool_call.kind, Some(ToolKind::Other));
    let names: Vec<_> = reqs[0].options.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["sqlite", "postgres"]);
    assert_eq!(reqs[0].options[1].option_id, "opt-1");
    wait_for(|| {
        env.client.updates().iter().any(|u| {
            matches!(u,
            SessionUpdate::ToolCallUpdate(t) if format!("{:?}", t.content).contains("postgres"))
        })
    })
    .await;
    // Second call is cancelled -> tool error, turn still completes.
    let r = prompt(&c, &sid, "again").await;
    assert_eq!(r["stopReason"], "end_turn");
    wait_for(|| {
        env.client.updates().iter().any(|u| {
            matches!(u,
            SessionUpdate::ToolCallUpdate(t) if t.status == Some(ToolCallStatus::Failed))
        })
    })
    .await;
}

#[tokio::test]
async fn set_mode_emits_update() {
    let env = Env::new();
    std::fs::create_dir_all(env.cwd().join(".ruckcode")).unwrap();
    std::fs::write(
        env.cwd().join(".ruckcode/modes.toml"),
        "default_mode = \"a\"\n[modes.a]\ndescription = \"A\"\ntools = [\"Read\"]\n[modes.b]\ndescription = \"B\"\ntools = [\"Read\"]\n",
    )
    .unwrap();
    let c = env.connect(mock([]));
    init(&c, false).await;
    let r = c
        .request(SESSION_NEW, json!({"cwd":env.cwd()}))
        .await
        .unwrap();
    assert_eq!(r["modes"]["currentModeId"], "a");
    assert_eq!(r["modes"]["availableModes"].as_array().unwrap().len(), 2);
    let sid = r["sessionId"].as_str().unwrap();
    c.request(SESSION_SET_MODE, json!({"sessionId":sid,"modeId":"b"}))
        .await
        .unwrap();
    wait_for(|| {
        env.client.updates().iter().any(|u| {
            matches!(u, SessionUpdate::CurrentModeUpdate { current_mode_id } if current_mode_id == "b")
        })
    })
    .await;
    let e = c
        .request(SESSION_SET_MODE, json!({"sessionId":sid,"modeId":"zzz"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, -32603);
}

#[tokio::test]
async fn load_session_replays() {
    let env = Env::new();
    let c = env.connect(mock([Message::assistant_text("first answer")]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    prompt(&c, &sid, "question one").await;
    wait_for(|| agent_text(&env.client.updates()) == "first answer").await;

    // New agent instance, same project dir, fresh client.
    let env2 = Env {
        proj: env.proj,
        user: tempfile::tempdir().unwrap(),
        client: Arc::new(TestClient::default()),
    };
    let c2 = env2.connect(mock([]));
    init(&c2, false).await;
    let r = c2
        .request(
            SESSION_LOAD,
            json!({"sessionId":sid,"cwd":env2.cwd(),"mcpServers":[]}),
        )
        .await
        .unwrap();
    assert_eq!(r["modes"]["currentModeId"], "default");
    wait_for(|| agent_text(&env2.client.updates()) == "first answer").await;
    let u = env2.client.updates();
    let user = u
        .iter()
        .position(|u| matches!(u, SessionUpdate::UserMessageChunk { content: ContentBlock::Text { text } } if text == "question one"))
        .expect("user chunk");
    let agent = u
        .iter()
        .position(|u| matches!(u, SessionUpdate::AgentMessageChunk { .. }))
        .unwrap();
    assert!(user < agent);
}

struct Slow;
#[async_trait]
impl Provider for Slow {
    fn name(&self) -> &str {
        "slow"
    }
    async fn complete(
        &self,
        _: ruckcode_llm::Request,
        _: tokio::sync::mpsc::UnboundedSender<ruckcode_llm::StreamEvent>,
    ) -> anyhow::Result<ruckcode_llm::Response> {
        tokio::time::sleep(Duration::from_secs(30)).await;
        unreachable!()
    }
}

#[tokio::test]
async fn cancel_notification() {
    let env = Env::new();
    let c = env.connect(Arc::new(Slow));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    let c2 = c.clone();
    let sid2 = sid.clone();
    let task = tokio::spawn(async move { prompt(&c2, &sid2, "go").await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.notify(SESSION_CANCEL, json!({"sessionId":sid})).unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopReason"], "cancelled");
}
