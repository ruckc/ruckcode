//! Model-managed context (`[context] model_managed`): tagged history, the usage line, the
//! `Context` tool (collapse and recall), resume, and the harness safety net. Scripted provider.

use async_trait::async_trait;
use ruck_harness_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
};
use ruck_harness_core::context::estimate_tokens;
use ruck_harness_core::transcript::{session_path, Record, TranscriptReader};
use ruck_harness_llm::mock::MockProvider;
use ruck_harness_llm::{ContentBlock, Message, Request, Role};
use ruck_harness_tools::{Answer, Question, UserBridge};
use ruckcode_agent::extension::RuckcodeAgent as _;
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

fn base(window: u32, extra_profile: &str, context: &str) -> String {
    format!(
        "default_profile = \"p\"\n[profiles.p]\nprovider = \"mock\"\nmodel = \"m\"\n\
         context_window = {window}\nmax_tokens = 1024\n{extra_profile}\n\
         [profiles.p.context]\n{context}\n"
    )
}

impl Env {
    fn new(config: &str) -> Self {
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(proj.path().join(".ruckcode/config.toml"), config).unwrap();
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

    /// A managed-context project with a 16K window.
    fn managed() -> Self {
        Self::new(&base(16384, "", "model_managed = true"))
    }

    fn agent_with(&self, mock: Arc<MockProvider>) -> Agent {
        Agent::ruckcode(AgentOptions {
            provider_override: Some(mock),
            user_config_dir: Some(self.user.path().to_path_buf()),
            ..Default::default()
        })
    }

    fn agent(&self) -> Agent {
        self.agent_with(self.mock.clone())
    }

    fn cwd(&self) -> PathBuf {
        self.proj.path().to_path_buf()
    }

    fn requests(&self) -> Vec<Request> {
        self.mock.requests.lock().unwrap().clone()
    }

    fn records(&self, id: &str) -> Vec<Record> {
        TranscriptReader::load(&session_path(
            &ruckcode_agent::identity::RUCKCODE,
            self.proj.path(),
            id,
        ))
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

/// A Bash call printing `n` lines of `tag`.
fn bash_lines(id: &str, tag: &str, n: usize) -> Message {
    tool_use(
        id,
        "Bash",
        json!({"command": format!("yes {tag} | head -n {n}")}),
    )
}

fn collapse(id: &str, from: &str, to: &str, summary: &str) -> Message {
    tool_use(
        id,
        "Context",
        json!({"action": "collapse", "from": from, "to": to, "summary": summary}),
    )
}

fn recall(id: &str, tag: &str) -> Message {
    tool_use(id, "Context", json!({"action": "recall", "tag": tag}))
}

fn result_of(r: &Request, id: &str) -> Option<(String, bool)> {
    r.messages
        .iter()
        .flat_map(|m| m.content.iter())
        .find_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } if tool_use_id == id => Some((content.clone(), *is_error)),
            _ => None,
        })
}

fn all_text(r: &Request) -> String {
    r.messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::ToolResult { content, .. } => content.clone(),
            ContentBlock::ToolUse { name, input, .. } => format!("{name} {input}"),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn est(r: &Request) -> u64 {
    estimate_tokens(&r.system, &r.tools, &r.messages)
}

fn has_context_tool(r: &Request) -> bool {
    r.tools.iter().any(|t| t.name == "Context")
}

async fn prompt(agent: &Agent, id: &str, text: &str, client: &Arc<TestClient>) -> StopReason {
    agent.prompt(id, text.into(), client.clone()).await.unwrap()
}

#[tokio::test]
async fn off_by_default_requests_carry_no_tags_note_or_tool() {
    let env = Env::new(&base(16384, "", ""));
    env.mock.push(bash_lines("a", "AAAA", 3));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "do it", &client).await;
    for r in env.requests() {
        assert!(!has_context_tool(&r));
        assert!(!r.system.contains("## Context"));
        let t = all_text(&r);
        assert!(!t.contains("[e1]") && !t.contains("context:"), "{t}");
    }
    assert_eq!(env.requests()[0].messages[0].text(), "do it");
}

#[tokio::test]
async fn on_requests_are_tagged_end_with_the_usage_line_and_offer_the_tool() {
    let env = Env::managed();
    env.mock.push(bash_lines("a", "AAAA", 3));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "do it", &client).await;
    let reqs = env.requests();
    assert!(reqs.iter().all(has_context_tool));
    let ctx_tool = reqs[0].tools.iter().find(|t| t.name == "Context").unwrap();
    assert!(!ctx_tool.input_schema.to_string().contains("oneOf"));
    assert!(reqs[0].system.contains("## Context"));
    // The system prompt is the same on every request (prompt caching).
    assert_eq!(reqs[0].system, reqs[1].system);
    // First request: the user message is tagged and the usage line closes the request.
    let first = &reqs[0].messages;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].text().lines().next().unwrap(), "[e1] do it");
    assert!(first[0].text().starts_with("[e1] do it\n\ncontext: "));
    let last_text = first[0].text();
    assert!(
        last_text.contains("context: ")
            && last_text
                .contains("% of 16384 tokens; compact with the Context tool only after 85%"),
        "{last_text}"
    );
    assert!(!last_text.contains("WARNING"));
    // Second request: assistant call e2 and result e3 are tagged; the note is at the tail only.
    let second = &reqs[1].messages;
    assert!(matches!(&second[1].content[0],
        ContentBlock::Text { text } if text == "[e2]"));
    let (res, _) = result_of(&reqs[1], "a").unwrap();
    assert!(res.starts_with("[e3] "), "{res}");
    assert_eq!(all_text(&reqs[1]).matches("context: ").count(), 1);
    // The stored transcript never carries labels or the note.
    let text = std::fs::read_to_string(session_path(
        &ruckcode_agent::identity::RUCKCODE,
        env.proj.path(),
        &info.id,
    ))
    .unwrap();
    assert!(!text.contains("[e1]") && !text.contains("compact with the Context tool"));
}

#[tokio::test]
async fn pressure_warning_appears_past_the_high_mark() {
    // 8K window, 256 reserved: the harness compacts at 95%; the model is warned from 90%.
    let env = Env::new(&base(
        8192,
        "",
        "model_managed = true\noutput_reserve_tokens = 256\nmax_tool_result_tokens = 0",
    ));
    env.mock.push(bash_lines("a", "AAAAAAAA", 2757));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "go", &client).await;
    let reqs = env.requests();
    let e = est(&reqs[1]);
    assert!(
        (7373..7782).contains(&e),
        "tune the output size: estimate {e}"
    );
    let t = all_text(&reqs[1]);
    assert!(t.contains("WARNING: context pressure"), "{t}");
    assert!(!all_text(&reqs[0]).contains("WARNING"));
}

#[tokio::test]
async fn collapse_replaces_whole_exchanges_and_the_next_request_is_smaller() {
    let env = Env::managed();
    env.mock.push(bash_lines("a", "AAAAAAAA", 400));
    env.mock.push(bash_lines("b", "BBBBBBBB", 400));
    env.mock.push(collapse(
        "c",
        "e2",
        "e5",
        "ran two big commands; nothing needed",
    ));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    assert_eq!(
        prompt(&agent, &info.id, "do it", &client).await,
        StopReason::EndTurn
    );
    let reqs = env.requests();
    assert_eq!(reqs.len(), 4);
    // The request that issued the collapse had both big results; the next has the summary.
    assert!(all_text(&reqs[2]).contains("AAAAAAAA"));
    let after = &reqs[3];
    let t = all_text(after);
    assert!(!t.contains("AAAAAAAA") && !t.contains("BBBBBBBB"), "{t}");
    assert!(t.contains("[e2..e5]") && t.contains("ran two big commands"));
    assert!(
        est(after) + 1500 < est(&reqs[2]),
        "{} vs {}",
        est(after),
        est(&reqs[2])
    );
    let (res, is_err) = result_of(after, "c").unwrap();
    assert!(
        !is_err && res.contains("Collapsed e2..e5 (4 events -> 1 summary"),
        "{res}"
    );
    // The task and the collapse call itself are still there and tagged.
    assert!(t.contains("[e1] do it") && t.contains("Context"));
    assert!(client
        .notices()
        .iter()
        .any(|n| n.starts_with("the model collapsed e2..e5 (4 events → 1 summary")));
    let rec = env.records(&info.id);
    assert!(rec
        .iter()
        .any(|r| matches!(r, Record::Collapsed { from: 2, to: 5, tag, .. } if tag == "e2..e5")));
}

#[tokio::test]
async fn bad_ranges_are_rejected_with_a_reason_and_change_nothing() {
    let env = Env::managed();
    env.mock.push(bash_lines("a", "AAAA", 3)); // e2, e3
    env.mock.push(bash_lines("b", "BBBB", 3)); // e4, e5
    env.mock.push(collapse("x3", "e2", "e99", "does not exist"));
    env.mock.push(collapse("x4", "e2", "e1", "backwards"));
    env.mock.push(tool_use(
        "x5",
        "Context",
        json!({"action": "collapse", "from": "e2"}),
    ));
    env.mock
        .push(tool_use("x6", "Context", json!({"action": "shrink"})));
    env.mock.push(recall("x7", "e99"));
    env.mock.push(recall("x8", "nonsense"));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "do it", &client).await;
    let reqs = env.requests();
    let last = reqs.last().unwrap();
    let err = |id: &str| {
        let (c, e) =
            result_of(last, id).unwrap_or_else(|| panic!("no result for {id}: {}", all_text(last)));
        assert!(e, "{id} should fail: {c}");
        c
    };
    assert!(err("x3").contains("not in the history"));
    assert!(err("x4").contains("after"));
    let c = err("x5");
    assert!(c.contains("missing: to, summary"), "{c}");
    assert!(err("x6").contains("unknown action"));
    assert!(err("x7").contains("e1.."));
    assert!(err("x8").contains("invalid tag"));
    // Nothing was collapsed: both big results are still in the history.
    let t = all_text(last);
    assert!(t.contains("AAAA") && t.contains("BBBB"));
    assert!(!env
        .records(&info.id)
        .iter()
        .any(|r| matches!(r, Record::Collapsed { .. })));
}

#[tokio::test]
async fn ranges_that_split_a_tool_pair_are_widened_and_the_task_stays() {
    let env = Env::managed();
    env.mock.push(bash_lines("a", "AAAA", 3)); // e2, e3
    env.mock.push(bash_lines("b", "BBBB", 3)); // e4, e5
                                               // e3 is a result, e4 a call: both pairs are split; the range is widened to e2..e5.
    env.mock.push(collapse("x1", "e3", "e4", "both commands"));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "do it", &client).await;
    let last = env.requests().pop().unwrap();
    let (c, e) = result_of(&last, "x1").unwrap();
    assert!(!e, "{c}");
    assert!(
        c.contains("Collapsed e2..e5")
            && c.contains("You asked for e3..e4")
            && c.contains("widened"),
        "{c}"
    );
    let t = all_text(&last);
    assert!(!t.contains("AAAA") && !t.contains("BBBB"));
    assert!(t.contains("[e1] do it"), "the task stays: {t}");
    assert!(client
        .notices()
        .iter()
        .any(|n| n.starts_with("the model collapsed e2..e5 (4 events")));

    // A range starting at the task starts after it.
    let env = Env::managed();
    env.mock.push(bash_lines("a", "AAAA", 3));
    env.mock.push(collapse("x1", "e1", "e3", "all of it"));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    prompt(&agent, &info.id, "do it", &Arc::new(TestClient::default())).await;
    let last = env.requests().pop().unwrap();
    let (c, e) = result_of(&last, "x1").unwrap();
    assert!(
        !e && c.contains("Collapsed e2..e3") && c.contains("task"),
        "{c}"
    );
    assert!(all_text(&last).contains("[e1] do it"));
}

#[tokio::test]
async fn the_exchange_in_flight_cannot_be_collapsed() {
    let env = Env::managed();
    // The call being run (e2) is the exchange in flight.
    env.mock.push(collapse("x1", "e2", "e2", "myself"));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    prompt(&agent, &info.id, "go", &Arc::new(TestClient::default())).await;
    let reqs = env.requests();
    let (c, e) = result_of(&reqs[1], "x1").unwrap();
    assert!(
        e && (c.contains("in flight") || c.contains("whole exchanges")),
        "{c}"
    );
}

#[tokio::test]
async fn recall_reads_originals_by_event_and_by_summary_tag_and_is_capped() {
    let env = Env::new(&base(
        16384,
        "",
        "model_managed = true\nmax_tool_result_tokens = 300",
    ));
    env.mock.push(bash_lines("a", "AAAAAAAA", 60)); // e2, e3
    env.mock.push(bash_lines("b", "BBBBBBBB", 2000)); // e4, e5 (capped on entry)
    env.mock.push(collapse("c", "e2", "e5", "two commands"));
    env.mock.push(recall("r1", "e3"));
    env.mock.push(recall("r2", "e2..e5"));
    env.mock.push(recall("r3", "e4"));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "go", &client).await;
    let last = env.requests().pop().unwrap();
    let (r1, e1) = result_of(&last, "r1").unwrap();
    assert!(
        !e1 && r1.contains("[e3 user]") && r1.contains("AAAAAAAA"),
        "{r1}"
    );
    assert!(!r1.contains("still in your context"));
    let (r2, _) = result_of(&last, "r2").unwrap();
    assert!(r2.contains("[e2 assistant]") && r2.contains("-> Bash"));
    // Over the per-result cap (300 tokens): head and tail are kept.
    assert!(
        r2.contains("truncated by ruckcode") || r2.len() < 1500,
        "{}",
        r2.len()
    );
    let (r3, _) = result_of(&last, "r3").unwrap();
    assert!(r3.contains("-> Bash"));
    // A recall does not un-collapse the history.
    assert!(all_text(&last).contains("[e2..e5]"));
    assert!(client
        .notices()
        .iter()
        .any(|n| n.starts_with("the model recalled e3 (1 event")));
}

#[tokio::test]
async fn recall_output_is_cut_to_the_per_result_cap() {
    let env = Env::new(&base(
        16384,
        "",
        "model_managed = true\nmax_tool_result_tokens = 100000\nmax_tool_result_fraction = 0.02",
    ));
    // Cap ~ 327 tokens ~ 980 bytes (MIN 1000).
    env.mock.push(bash_lines("a", "AAAAAAAA", 200));
    env.mock.push(bash_lines("b", "BBBBBBBB", 200));
    env.mock.push(collapse("c", "e2", "e5", "s"));
    env.mock.push(recall("r", "e2..e5"));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "go", &client).await;
    let last = env.requests().pop().unwrap();
    let (r, _) = result_of(&last, "r").unwrap();
    assert!(r.contains("truncated by ruckcode"), "{r}");
    assert!(r.len() < 2500, "{}", r.len());
    assert!(r.contains("[e2 assistant]"), "head kept: {r}");
}

#[tokio::test]
async fn resume_rebuilds_the_same_history_and_recall_still_works() {
    let env = Env::managed();
    env.mock.push(bash_lines("a", "AAAAAAAA", 50)); // e2 e3
    env.mock.push(bash_lines("b", "BBBBBBBB", 50)); // e4 e5
    env.mock.push(collapse("c", "e2", "e5", "did a and b")); // e6, result e7
    env.mock.push(Message::assistant_text("first turn done")); // e8
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    prompt(&agent, &info.id, "go", &Arc::new(TestClient::default())).await;
    let before = env.requests().pop().unwrap();
    drop(agent);

    // A new process: same transcript, fresh agent.
    let mock2 = Arc::new(MockProvider::default());
    mock2.push(recall("r", "e3"));
    mock2.push(collapse("c2", "e2", "e8", "everything so far"));
    mock2.push(Message::assistant_text("second turn done"));
    let agent2 = env.agent_with(mock2.clone());
    agent2
        .resume_session_with(&info.id, env.cwd(), Default::default())
        .await
        .unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent2, &info.id, "again", &client).await;
    let reqs = mock2.requests.lock().unwrap().clone();
    // History identical: everything the first process sent last is a prefix of what the second
    // sends first (same tags, same summary), apart from the usage line at the tail.
    let strip = |r: &Request| -> Vec<String> {
        r.messages
            .iter()
            .flat_map(|m| m.content.iter())
            .map(|b| match b {
                ContentBlock::Text { text } => text.split("\ncontext: ").next().unwrap().into(),
                ContentBlock::ToolResult { content, .. } => content.clone(),
                ContentBlock::ToolUse { name, input, .. } => format!("{name} {input}"),
                _ => String::new(),
            })
            .filter(|t| !t.starts_with("context: "))
            .collect()
    };
    let b = strip(&before);
    let a = strip(&reqs[0]);
    assert!(a.len() > b.len());
    assert_eq!(a[..b.len()], b[..], "history differs after resume");
    assert!(a
        .iter()
        .any(|t| t.contains("[e2..e5]") && t.contains("did a and b")));
    // Recall of a collapsed original works after resume, and new tags continue the numbering.
    let (r, e) = result_of(&reqs[1], "r").unwrap();
    assert!(
        !e && r.contains("AAAAAAAA") && r.contains("[e3 user]"),
        "{r}"
    );
    let t1 = all_text(&reqs[1]);
    assert!(t1.contains("[e9]") || t1.contains("[e10]"), "{t1}");
    // And a second collapse after resume, over the first summary, works too.
    let (c, e) = result_of(&reqs[2], "c2").unwrap();
    assert!(!e, "{c}");
}

#[tokio::test]
async fn automatic_compaction_still_fires_when_the_model_never_compacts() {
    // Results are not capped, the model never calls Context: the harness compacts at its
    // (raised) threshold, before the window is exceeded.
    let env = Env::new(&base(
        8192,
        "",
        "model_managed = true\nmax_tool_result_tokens = 0\nmanaged_fallback_threshold = 0.8",
    ));
    env.mock.push(bash_lines("a", "AAAAAAAAAA", 700));
    env.mock.push(bash_lines("b", "BBBBBBBBBB", 700));
    env.mock.push(Message::assistant_text("THE SUMMARY"));
    env.mock.push(Message::assistant_text("all done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "do the work", &client).await;
    let reqs = env.requests();
    assert_eq!(reqs.len(), 4, "request, request, summary, request");
    assert!(reqs[2].tools.is_empty());
    let last = &reqs[3];
    assert!(est(last) + 1024 <= 8192);
    assert!(all_text(last).contains("THE SUMMARY"));
    assert!(client
        .notices()
        .iter()
        .any(|n| n.contains("conversation compacted")));
    // The compaction summary is untagged, the kept exchange still has its tags.
    assert!(!all_text(last).contains("[e1]"));
    assert!(all_text(last).contains("[e5] ") || all_text(last).contains("[e4]"));
}

#[tokio::test]
async fn the_harness_waits_for_the_model_by_default_when_managed() {
    // Without `managed_fallback_threshold`, 2 x 7.7 KB results (about 6.5K tokens of 8K) do not
    // trigger the harness at 95% of the window (87.5% after the reserve).
    let env = Env::new(&base(
        8192,
        "",
        "model_managed = true\nmax_tool_result_tokens = 0\noutput_reserve_tokens = 256",
    ));
    env.mock.push(bash_lines("a", "AAAAAAAAAA", 600));
    env.mock.push(bash_lines("b", "BBBBBBBBBB", 600));
    env.mock.push(Message::assistant_text("all done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "go", &client).await;
    assert_eq!(env.requests().len(), 3, "no summarizing request");
    // The same history with the feature off compacts at 80%.
    let off = Env::new(&base(
        8192,
        "",
        "output_reserve_tokens = 256\nmax_tool_result_tokens = 0",
    ));
    off.mock.push(bash_lines("a", "AAAAAAAAAA", 600));
    off.mock.push(bash_lines("b", "BBBBBBBBBB", 600));
    off.mock.push(Message::assistant_text("SUMMARY"));
    off.mock.push(Message::assistant_text("all done"));
    let agent = off.agent();
    let info = agent.new_session(off.cwd(), None).await.unwrap();
    prompt(&agent, &info.id, "go", &Arc::new(TestClient::default())).await;
    assert_eq!(off.requests().len(), 4);
}

#[tokio::test]
async fn the_hard_ceiling_still_truncates_when_the_model_never_compacts() {
    let env = Env::new(&base(
        8192,
        "",
        "model_managed = true\nmax_tool_result_tokens = 0\nmanaged_fallback_threshold = 1.0",
    ));
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
    prompt(&agent, &info.id, "go", &client).await;
    let last = env.requests().pop().unwrap();
    assert!(est(&last) + 1024 <= 8192, "estimate {}", est(&last));
    assert!(client
        .notices()
        .iter()
        .any(|n| n.contains("Context nearly full")));
}

#[tokio::test]
async fn lean_profiles_exclude_the_tool_unless_allowlisted() {
    let lean = Env::new(&base(16384, "lean_tools = true", "model_managed = true"));
    lean.mock.push(Message::assistant_text("hi"));
    let agent = lean.agent();
    let info = agent.new_session(lean.cwd(), None).await.unwrap();
    prompt(&agent, &info.id, "go", &Arc::new(TestClient::default())).await;
    let r = &lean.requests()[0];
    assert!(!has_context_tool(r));
    // Not offered, so nothing about it reaches the model.
    assert!(!r.system.contains("## Context"));
    assert!(!all_text(r).contains("[e1]") && !all_text(r).contains("context: "));

    let listed = Env::new(&base(
        16384,
        "lean_tools = true\ntools = [\"Read\", \"Context\"]",
        "model_managed = true",
    ));
    listed.mock.push(Message::assistant_text("hi"));
    let agent = listed.agent();
    let info = agent.new_session(listed.cwd(), None).await.unwrap();
    prompt(&agent, &info.id, "go", &Arc::new(TestClient::default())).await;
    let r = &listed.requests()[0];
    assert!(has_context_tool(r));
    assert!(r.tools.iter().any(|t| t.name == "Read"));
}

#[tokio::test]
async fn global_setting_applies_and_a_profile_can_turn_it_off() {
    let cfg = format!(
        "{}\n[context]\nmodel_managed = true\n",
        base(16384, "", "model_managed = false")
    );
    let env = Env::new(&cfg);
    env.mock.push(Message::assistant_text("hi"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    prompt(&agent, &info.id, "go", &Arc::new(TestClient::default())).await;
    assert!(!has_context_tool(&env.requests()[0]));
}

#[tokio::test]
async fn identical_recalls_trip_the_loop_guard_but_different_tags_do_not() {
    // Different tags: seven recalls in a row are not a loop.
    let env = Env::managed();
    env.mock.push(bash_lines("a", "AAAA", 3));
    for i in 1..=7 {
        env.mock.push(recall(&format!("r{i}"), &format!("e{i}")));
    }
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    assert_eq!(
        prompt(&agent, &info.id, "go", &client).await,
        StopReason::EndTurn
    );
    assert!(
        !client.notices().iter().any(|n| n.contains("`Context`")),
        "{:?}",
        client.notices()
    );

    // The same tag over and over trips it as for any tool.
    let env = Env::managed();
    env.mock.push(bash_lines("a", "AAAA", 3));
    for i in 1..=5 {
        env.mock.push(recall(&format!("r{i}"), "e3"));
    }
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "go", &client).await;
    assert!(
        client
            .notices()
            .iter()
            .any(|n| n.contains("`Context` 4 times")),
        "{:?}",
        client.notices()
    );
}

#[tokio::test]
async fn collapsed_sessions_load_with_the_summary_in_place() {
    let env = Env::managed();
    env.mock.push(bash_lines("a", "AAAA", 3));
    env.mock.push(collapse("c", "e2", "e3", "ran a"));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    prompt(&agent, &info.id, "go", &Arc::new(TestClient::default())).await;
    let loaded = TranscriptReader::load(&session_path(
        &ruckcode_agent::identity::RUCKCODE,
        env.proj.path(),
        &info.id,
    ))
    .unwrap();
    assert_eq!(loaded.messages.len(), loaded.tags.len());
    assert_eq!(loaded.collapses, 1);
    assert!(loaded.messages[1].text().starts_with("[e2..e3]"));
    assert_eq!(loaded.archive.len(), 2);
    // Events: user, call, result, call(Context), result, final text.
    assert_eq!(loaded.event_count, 6);
}

#[tokio::test]
async fn the_demo_provider_can_collapse_with_mock_collapse() {
    let env = Env::managed();
    let demo = Arc::new(MockProvider::demo());
    let agent = env.agent_with(demo.clone());
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    for p in ["mock:bash echo one", "mock:bash echo two"] {
        prompt(&agent, &info.id, p, &client).await;
    }
    // Commands still work with tags and the usage line on the prompt.
    assert!(!client.notices().iter().any(|n| n.contains("collapsed")));
    prompt(&agent, &info.id, "mock:collapse", &client).await;
    let notices = client.notices();
    assert!(
        notices
            .iter()
            .any(|n| n.starts_with("the model collapsed e2..e") && n.contains("events → 1 summary")),
        "{notices:?}"
    );
}

#[tokio::test]
async fn mock_collapse_explains_itself_when_the_feature_is_off() {
    let env = Env::new(&base(16384, "", ""));
    let agent = env.agent_with(Arc::new(MockProvider::demo()));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "mock:collapse", &client).await;
    assert!(!client.notices().iter().any(|n| n.contains("collapsed")));
}

#[tokio::test]
async fn a_mode_whose_tool_list_omits_context_does_not_get_it() {
    let env = Env::managed();
    std::fs::write(
        env.proj.path().join(".ruckcode/modes.toml"),
        r#"
default_mode = "narrow"
[modes.narrow]
description = "narrow"
tools = ["Read", "Bash"]
next = ["wide"]
[modes.wide]
description = "wide"
tools = ["Read", "Context"]
"#,
    )
    .unwrap();
    env.mock.push(Message::assistant_text("hi"));
    env.mock.push(Message::assistant_text("hi again"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = Arc::new(TestClient::default());
    prompt(&agent, &info.id, "go", &client).await;
    let r = &env.requests()[0];
    assert!(!has_context_tool(r), "mode tool list is respected");
    assert!(!r.system.contains("## Context"));
    assert!(!all_text(r).contains("[e1]") && !all_text(r).contains("context: "));
    agent
        .set_mode(&info.id, "wide", client.clone())
        .await
        .unwrap();
    prompt(&agent, &info.id, "again", &client).await;
    let r = &env.requests()[1];
    assert!(has_context_tool(r));
    assert!(all_text(r).contains("[e1] go"));
}
