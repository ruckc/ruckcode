//! The tool-loop guard: repeated no-progress tool calls are dropped from the history, nudged,
//! and end the turn after too many nudges; legitimate progress never trips it.

use async_trait::async_trait;
use ruckcode_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
};
use ruckcode_core::transcript::{session_path, Record, TranscriptReader};
use ruckcode_llm::mock::MockProvider;
use ruckcode_llm::{ContentBlock, Message, Request, Role};
use ruckcode_tools::{Answer, Question, UserBridge};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const NUDGE: &str =
    "You are repeating the same tool call without progress. Take a different next step, or answer.";

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
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
    fn plans(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, SessionEvent::Plan(_)))
            .count()
    }
    fn outputs(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                SessionEvent::ToolCallUpdate {
                    output: Some(o), ..
                } => Some(o.clone()),
                _ => None,
            })
            .collect()
    }
}

fn call(id: &str, name: &str, input: Value) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }],
    }
}

struct Run {
    stop: StopReason,
    client: Arc<TestClient>,
    mock: Arc<MockProvider>,
    records: Vec<Record>,
}

/// Run one prompt against the scripted `steps` with `config` as the project config.
async fn run(steps: Vec<Message>, config: &str) -> Run {
    let proj = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
    std::fs::write(proj.path().join(".ruckcode/config.toml"), config).unwrap();
    std::fs::write(
        user.path().join("config.toml"),
        "[sandbox]\nenabled = false\n",
    )
    .unwrap();
    for n in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        std::fs::write(proj.path().join(format!("{n}.txt")), format!("file {n}")).unwrap();
    }
    let mock = Arc::new(MockProvider::new(steps));
    let agent = Agent::new(AgentOptions {
        provider_override: Some(mock.clone()),
        user_config_dir: Some(user.path().to_path_buf()),
        ..Default::default()
    });
    let info = agent
        .new_session(proj.path().to_path_buf(), None)
        .await
        .unwrap();
    let client = Arc::new(TestClient::default());
    let stop = agent
        .prompt(&info.id, "design a workflow".into(), client.clone())
        .await
        .unwrap();
    let records = TranscriptReader::load(&session_path(proj.path(), &info.id))
        .unwrap()
        .records
        .into_iter()
        .map(|r| r.record)
        .collect();
    Run {
        stop,
        client,
        mock,
        records,
    }
}

fn plan(statuses: [&str; 3]) -> Value {
    let items: Vec<Value> = ["draft", "review", "ship"]
        .iter()
        .zip(statuses)
        .map(|(c, s)| json!({"content": c, "status": s}))
        .collect();
    json!({ "todos": items })
}

fn tool_uses(r: &Request) -> usize {
    r.messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
        .count()
}

#[tokio::test]
async fn repeated_noop_plan_updates_are_local_dropped_nudged_and_end_the_turn() {
    let same = plan(["pending", "pending", "pending"]);
    let steps = (0..10)
        .map(|i| call(&format!("t{i}"), "TodoWrite", same.clone()))
        .collect();
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::ToolLoop);
    assert_eq!(r.stop.as_str(), "tool_loop");
    // Only the first update ran and reached clients; the other nine were answered locally.
    assert_eq!(r.client.plans(), 1);
    let outputs = r.client.outputs();
    assert_eq!(outputs.len(), 10);
    assert!(outputs[0].starts_with("Plan updated"));
    assert!(outputs[1..]
        .iter()
        .all(|o| o == "Plan unchanged (no items or statuses changed)."));
    // 1 real + 3 no-ops trips; nudged twice; the third trip ends the turn.
    let notices = r.client.notices();
    assert_eq!(notices.len(), 3, "{notices:?}");
    assert!(notices[0].contains("nudge 1/2") && notices[1].contains("nudge 2/2"));
    assert!(notices[2].contains("stopping this turn after 2 nudge"));
    let reqs = r.mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 10);
    // The request after the first trip carries the first update only, and the nudge merged
    // into the tool-result message so roles still alternate.
    let after = &reqs[4];
    assert_eq!(tool_uses(after), 1);
    assert_eq!(after.messages.len(), 3);
    assert_eq!(after.messages[2].role, Role::User);
    assert!(after.messages[2].text().contains(NUDGE));
    // Every request is bounded: the history never regrows past one kept call + one nudge each.
    assert!(reqs.iter().all(|q| tool_uses(q) <= 3));
    // The turn is unfinished and the drops are recorded for resume.
    assert!(r
        .records
        .iter()
        .any(|x| matches!(x, Record::ToolCallsDropped { .. })));
    assert!(r
        .records
        .iter()
        .any(|x| matches!(x, Record::TurnEnd { stop_reason, .. } if stop_reason == "tool_loop")));
}

#[tokio::test]
async fn identical_reads_trip_the_guard_and_drop_the_repeats() {
    let mut steps: Vec<Message> = (0..4)
        .map(|i| call(&format!("r{i}"), "Read", json!({"file_path": "a.txt"})))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    let notices = r.client.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("`Read` 4 times"), "{notices:?}");
    let reqs = r.mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 5);
    assert_eq!(tool_uses(&reqs[4]), 1);
    assert!(reqs[4].messages.last().unwrap().text().contains(NUDGE));
}

#[tokio::test]
async fn zero_nudges_stops_on_the_first_trip() {
    let steps: Vec<Message> = (0..4)
        .map(|i| call(&format!("r{i}"), "Read", json!({"file_path": "a.txt"})))
        .collect();
    let r = run(steps, "[tool_loop_guard]\nmax_nudges = 0\n").await;
    assert_eq!(r.stop, StopReason::ToolLoop);
    assert_eq!(r.mock.requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn reading_different_files_is_progress() {
    let mut steps: Vec<Message> = ["a", "b", "c", "d", "e", "f", "g", "h"]
        .iter()
        .map(|n| call(n, "Read", json!({"file_path": format!("{n}.txt")})))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty());
}

#[tokio::test]
async fn same_command_with_changing_output_is_progress() {
    let cmd = "n=$(cat counter 2>/dev/null || echo 0); echo $((n+1)) > counter; echo run $n";
    let mut steps: Vec<Message> = (0..8)
        .map(|i| call(&format!("b{i}"), "Bash", json!({ "command": cmd })))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty(), "{:?}", r.client.notices());
    let outs = r.client.outputs();
    assert!(outs.iter().any(|o| o.contains("run 7")), "{outs:?}");
}

#[tokio::test]
async fn plan_with_real_changes_is_progress_and_completion_still_reported() {
    let steps = vec![
        call("p0", "TodoWrite", plan(["pending", "pending", "pending"])),
        call(
            "p1",
            "TodoWrite",
            plan(["in_progress", "pending", "pending"]),
        ),
        call(
            "p2",
            "TodoWrite",
            plan(["completed", "in_progress", "pending"]),
        ),
        call(
            "p3",
            "TodoWrite",
            plan(["completed", "completed", "in_progress"]),
        ),
        call(
            "p4",
            "TodoWrite",
            plan(["completed", "completed", "completed"]),
        ),
        Message::assistant_text("all done"),
    ];
    let r = run(steps, "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert!(r.client.notices().is_empty());
    assert_eq!(r.client.plans(), 5);
    assert!(r
        .client
        .outputs()
        .last()
        .unwrap()
        .contains("All plan items are complete"));
}

#[tokio::test]
async fn disabled_guard_runs_everything() {
    let same = plan(["pending", "pending", "pending"]);
    let mut steps: Vec<Message> = (0..10)
        .map(|i| call(&format!("t{i}"), "TodoWrite", same.clone()))
        .collect();
    steps.push(Message::assistant_text("done"));
    let r = run(steps, "[tool_loop_guard]\nenabled = false\n").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert_eq!(r.client.plans(), 10);
    assert!(r.client.notices().is_empty());
}
