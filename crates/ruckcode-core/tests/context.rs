//! Context-window management: mid-turn compaction, per-result caps, the hard ceiling and the
//! compact-and-retry after a `max_tokens` stop on a full context. Scripted provider, 8K window.

use async_trait::async_trait;
use ruckcode_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
};
use ruckcode_core::context::estimate_tokens;
use ruckcode_core::McpServerConfig;
use ruckcode_llm::mock::MockProvider;
use ruckcode_llm::{ContentBlock, Message, Request, Role, StopReason as LlmStop, Usage};
use ruckcode_tools::{Answer, Question, UserBridge};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
}

impl TestClient {
    fn notices(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Notice(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
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
    async fn request_permission(&self, _: &str, _: PermissionRequest) -> PermissionDecision {
        PermissionDecision::AllowOnce
    }
    fn send_event(&self, _: &str, event: SessionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

struct Env {
    proj: TempDir,
    user: TempDir,
    mock: Arc<MockProvider>,
}

const BASE: &str = r#"
default_profile = "p"
[profiles.p]
provider = "mock"
model = "m"
context_window = 8192
max_tokens = 1024
"#;

impl Env {
    fn new(top: &str, tables: &str) -> Self {
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(
            proj.path().join(".ruckcode/config.toml"),
            format!("{top}\n{BASE}\n{tables}"),
        )
        .unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("config.toml"),
            "[sandbox]\nenabled = false\n",
        )
        .unwrap();
        Self {
            proj,
            user,
            mock: Arc::new(MockProvider::default()),
        }
    }

    fn agent(&self) -> Agent {
        Agent::new(AgentOptions {
            provider_override: Some(self.mock.clone()),
            user_config_dir: Some(self.user.path().to_path_buf()),
            ..Default::default()
        })
    }

    fn cwd(&self) -> PathBuf {
        self.proj.path().to_path_buf()
    }

    fn requests(&self) -> Vec<Request> {
        self.mock.requests.lock().unwrap().clone()
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

/// A Bash call printing `n` lines of `tag`.
fn bash_lines(id: &str, tag: &str, n: usize) -> Message {
    tool_use(
        id,
        "Bash",
        json!({"command": format!("yes {tag} | head -n {n}")}),
    )
}

fn results(r: &Request) -> Vec<(String, String)> {
    r.messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some((tool_use_id.clone(), content.clone())),
            _ => None,
        })
        .collect()
}

fn assert_paired(r: &Request) {
    let uses: Vec<String> = r
        .messages
        .iter()
        .flat_map(|m| m.tool_uses().map(|(i, _, _)| i.to_string()))
        .collect();
    let res: Vec<String> = results(r).into_iter().map(|(i, _)| i).collect();
    assert_eq!(uses, res, "tool_use / tool_result pairing");
}

fn est(r: &Request) -> u64 {
    estimate_tokens(&r.system, &r.tools, &r.messages)
}

const MARKER: &str = "truncated by ruckcode; re-run with a narrower command or read a range";

#[tokio::test]
async fn mid_turn_results_trigger_compaction_before_the_next_request() {
    // Results are not capped (so they pile up); the threshold is crossed after the second one.
    let env = Env::new("", "[context]\nmax_tool_result_tokens = 0");
    env.mock.push(bash_lines("a", "AAAAAAAAAA", 700)); // ~7.7 KB
    env.mock.push(bash_lines("b", "BBBBBBBBBB", 700));
    env.mock.push(Message::assistant_text("THE SUMMARY"));
    env.mock.push(Message::assistant_text("all done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    agent
        .set_client_instructions(&info.id, Some("CLIENT-RULES-XYZ".into()))
        .await
        .unwrap();
    let client = Arc::new(TestClient::default());
    let r = agent
        .prompt(&info.id, "do the work".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);
    let reqs = env.requests();
    assert_eq!(reqs.len(), 4, "request, request, summary, request");
    // The compaction ran mid-turn, before the request that would have crossed the threshold.
    assert!(reqs[2].tools.is_empty(), "the third request summarizes");
    let last = &reqs[3];
    assert!(est(last) + 1024 <= 8192, "under the window");
    assert!(last.system.contains("CLIENT-RULES-XYZ"));
    assert!(last.messages[0].text().contains("THE SUMMARY"));
    // The latest tool call and its result are still there, pairing is valid.
    let res = results(last);
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].0, "b");
    assert!(res[0].1.contains("BBBBBBBBBB"));
    assert_paired(last);
    assert!(client
        .notices()
        .iter()
        .any(|n| n.contains("conversation compacted") && n.contains("latest tool results")));
}

#[tokio::test]
async fn huge_bash_result_is_truncated_head_and_tail() {
    let env = Env::new("", "");
    // 3000 numbered lines, far over the 8K window's per-result cap (10% = ~800 tokens).
    env.mock
        .push(tool_use("a", "Bash", json!({"command": "seq 1 3000"})));
    env.mock.push(Message::assistant_text("ok"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    let reqs = env.requests();
    let (_, out) = results(&reqs[1]).remove(0);
    assert!(out.contains(MARKER), "{out}");
    assert!(out.starts_with("1\n2\n"));
    assert!(out.trim_end().ends_with("2999\n3000") || out.trim_end().ends_with("3000"));
    assert!(out.len() < 3000, "{} bytes", out.len());
    assert!(client
        .notices()
        .iter()
        .any(|n| n.contains("Bash output truncated")));
}

#[tokio::test]
async fn huge_mcp_result_is_truncated_head_and_tail() {
    let env = Env::new("", "");
    let script = format!(
        "{}/../ruckcode-mcp/tests/fake_mcp_server.py",
        env!("CARGO_MANIFEST_DIR")
    );
    env.mock.push(tool_use(
        "a",
        "mcp__fake__echo",
        json!({"text": format!("HEAD{}TAIL", "x".repeat(20_000))}),
    ));
    env.mock.push(Message::assistant_text("ok"));
    let agent = Agent::new(AgentOptions {
        provider_override: Some(env.mock.clone()),
        user_config_dir: Some(env.user.path().to_path_buf()),
        ..Default::default()
    });
    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "fake".to_string(),
        McpServerConfig::stdio("python3", &[&script, "0"]),
    );
    let info = agent
        .new_session_with(env.cwd(), None, extra)
        .await
        .unwrap();
    agent
        .prompt(&info.id, "go".into(), Arc::new(TestClient::default()))
        .await
        .unwrap();
    let reqs = env.requests();
    let (_, out) = results(&reqs[1]).remove(0);
    assert!(out.contains(MARKER), "{}", &out[..200.min(out.len())]);
    assert!(out.contains("echo: HEAD") && out.contains("TAIL"));
    assert!(out.len() < 4000, "{} bytes", out.len());
}

#[tokio::test]
async fn several_large_results_are_fitted_under_the_ceiling() {
    // Per-result caps off and compaction off (threshold 1.0): only the ceiling can help.
    let env = Env::new(
        "compaction_threshold = 1.0",
        "[context]\nmax_tool_result_tokens = 0",
    );
    env.mock.push(Message {
        role: Role::Assistant,
        content: ["a", "b", "c"]
            .iter()
            .map(|id| ContentBlock::ToolUse {
                id: (*id).into(),
                name: "Bash".into(),
                input: json!({"command": format!("yes {id}{id}{id}{id}{id}{id}{id}{id} | head -n 1500")}),
            })
            .collect(),
    });
    env.mock.push(Message::assistant_text("ok"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    let reqs = env.requests();
    let last = &reqs[1];
    // ~3 x 13 KB of output would be ~13K tokens in an 8K window.
    assert!(est(last) + 1024 <= 8192, "estimate {}", est(last));
    let res = results(last);
    assert_eq!(res.len(), 3);
    assert!(res.iter().all(|(_, c)| c.contains(MARKER)));
    assert_paired(last);
    assert!(client
        .notices()
        .iter()
        .any(|n| n.contains("Context nearly full")));
}

#[tokio::test]
async fn max_tokens_on_a_full_context_compacts_and_retries_once() {
    let env = Env::new("", "");
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    env.mock.push(Message::assistant_text("first answer"));
    env.mock.push_usage(Usage::default()); // first answer
    env.mock.push_usage(Usage::default()); // tool call
    env.mock.push_stop(LlmStop::EndTurn);
    env.mock.push_stop(LlmStop::ToolUse);
    env.mock.push_stop(LlmStop::MaxTokens);
    env.mock.push_usage(Usage {
        input_tokens: 7000,
        output_tokens: 1192,
        ..Default::default()
    });
    agent
        .prompt(&info.id, "one".into(), Arc::new(TestClient::default()))
        .await
        .unwrap();
    // Second turn: a tool call, then a response cut off by the full window with no output.
    env.mock.push(bash_lines("a", "tool-output", 3));
    env.mock.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Thinking {
            thinking: "hmm".into(),
            signature: None,
        }],
    });
    env.mock.push(Message::assistant_text("THE SUMMARY"));
    env.mock.push(Message::assistant_text("recovered"));
    let client = Arc::new(TestClient::default());
    let r = agent
        .prompt(&info.id, "two".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);
    let reqs = env.requests();
    // first, tool call, cut off, summary, retry
    assert_eq!(reqs.len(), 5);
    assert!(reqs[3].tools.is_empty());
    let retry = &reqs[4];
    assert!(retry.messages[0].text().contains("THE SUMMARY"));
    assert_eq!(results(retry).len(), 1, "the latest tool result is kept");
    assert_paired(retry);
    let notices = client.notices();
    assert!(notices.iter().any(|n| n.contains("ran out of context")));
    assert!(notices.iter().any(|n| n.contains("conversation compacted")));
}

#[tokio::test]
async fn a_second_full_context_stop_ends_with_a_clear_message() {
    let env = Env::new("", "");
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    env.mock.push(bash_lines("a", "x", 3));
    let empty = || Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Thinking {
            thinking: "hmm".into(),
            signature: None,
        }],
    };
    env.mock.push(empty());
    env.mock.push(Message::assistant_text("THE SUMMARY"));
    env.mock.push(empty());
    let full = Usage {
        input_tokens: 7500,
        output_tokens: 692,
        ..Default::default()
    };
    env.mock.push_stop(LlmStop::ToolUse);
    env.mock.push_stop(LlmStop::MaxTokens);
    env.mock.push_stop(LlmStop::EndTurn); // the summary
    env.mock.push_stop(LlmStop::MaxTokens);
    env.mock.push_usage(Usage::default());
    env.mock.push_usage(full);
    env.mock.push_usage(Usage::default());
    env.mock.push_usage(full);
    let client = Arc::new(TestClient::default());
    let r = agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::MaxTokens);
    assert_eq!(env.requests().len(), 4, "one retry only");
    assert!(client
        .notices()
        .iter()
        .any(|n| n.contains("still full after compaction")));
}

#[tokio::test]
async fn normal_turns_are_unchanged() {
    let env = Env::new("", "");
    env.mock.push(bash_lines("a", "hello", 3));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    let reqs = env.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].messages.len(), 3);
    assert_eq!(results(&reqs[1])[0].1.trim(), "hello\nhello\nhello");
    assert!(client.notices().is_empty(), "{:?}", client.notices());
}
