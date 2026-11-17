//! The thinking-loop guard: abort a request whose reasoning repeats or overruns its budget,
//! discard that reasoning, nudge the model, and stop the turn after too many nudges.

use async_trait::async_trait;
use ruckcode_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
};
use ruckcode_llm::{
    ContentBlock, Message, Provider, Request, Response, Role, StopReason as LlmStop, StreamEvent,
    Usage,
};
use ruckcode_tools::{Answer, Question, UserBridge};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone)]
enum Step {
    /// Stream the same sentence over and over, then never finish.
    Repeat,
    /// Stream `n` distinct sentences of thinking, then never finish.
    Unique(usize),
    /// Stream `n` distinct sentences of thinking, then answer with this text.
    ThinkThenAnswer(usize, &'static str),
    Answer(&'static str),
}

#[derive(Default)]
struct Scripted {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<Request>>,
    aborted: AtomicUsize,
}

struct OnDrop<'a>(&'a AtomicUsize);
impl Drop for OnDrop<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn unique(i: usize) -> String {
    format!(
        "Distinct reasoning step number {i} looks at module {} carefully. ",
        i * 31
    )
}

#[async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }
    async fn complete(
        &self,
        request: Request,
        events: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response> {
        self.requests.lock().unwrap().push(request);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("script ran out");
        let think = |text: String| {
            let _ = events.send(StreamEvent::ThinkingDelta(text));
        };
        match step {
            Step::Repeat | Step::Unique(_) => {
                // Only reached by dropping this future: the guard aborting the stream.
                let _g = OnDrop(&self.aborted);
                for i in 0..10_000 {
                    think(match step {
                        Step::Repeat => {
                            "Actually, I will also fix the any in src/service-worker.ts. "
                                .to_string()
                        }
                        Step::Unique(n) if i >= n => break,
                        _ => unique(i),
                    });
                    tokio::task::yield_now().await;
                }
                std::future::pending::<()>().await;
                unreachable!()
            }
            Step::ThinkThenAnswer(n, text) => {
                for i in 0..n {
                    think(unique(i));
                    tokio::task::yield_now().await;
                }
                Ok(answer(text))
            }
            Step::Answer(text) => Ok(answer(text)),
        }
    }
}

fn answer(text: &str) -> Response {
    Response {
        message: Message::assistant_text(text),
        stop_reason: LlmStop::EndTurn,
        usage: Usage::default(),
        rate_limit: None,
    }
}

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
}

struct Run {
    provider: Arc<Scripted>,
    stop: StopReason,
    usage: Option<Usage>,
    client: Arc<TestClient>,
    agent: Agent,
    session: String,
}

/// Run one prompt against the script with `config` as the project config.
async fn run(steps: Vec<Step>, config: &str) -> Run {
    let proj = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
    std::fs::write(proj.path().join(".ruckcode/config.toml"), config).unwrap();
    let provider = Arc::new(Scripted {
        steps: Mutex::new(steps.into()),
        ..Default::default()
    });
    let agent = Agent::new(AgentOptions {
        provider_override: Some(provider.clone()),
        user_config_dir: Some(user.path().to_path_buf()),
        ..Default::default()
    });
    let info = agent
        .new_session(proj.path().to_path_buf(), None)
        .await
        .unwrap();
    let client = Arc::new(TestClient::default());
    let (stop, usage) = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        agent.prompt_with_usage(&info.id, "fix the types".into(), client.clone()),
    )
    .await
    .expect("turn hung: the stream was not aborted")
    .unwrap();
    // Keep the temp dirs alive until the turn is over.
    drop((proj, user));
    Run {
        provider,
        stop,
        usage,
        client,
        agent,
        session: info.id,
    }
}

fn texts(r: &Request) -> Vec<(Role, String)> {
    r.messages.iter().map(|m| (m.role, m.text())).collect()
}

const NUDGE: &str = "You were repeating yourself without acting. Take the next concrete step now with a tool call, or answer.";

#[tokio::test]
async fn repetition_aborts_discards_thinking_and_nudges() {
    let r = run(vec![Step::Repeat, Step::Answer("done")], "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    // The looping request was dropped (the stream aborted).
    assert_eq!(r.provider.aborted.load(Ordering::SeqCst), 1);
    let reqs = r.provider.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    // The retry carries no thinking of the aborted request, only the nudge merged into the
    // user message, so roles still alternate.
    let m = texts(&reqs[1]);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].0, Role::User);
    assert!(
        m[0].1.contains("fix the types") && m[0].1.contains(NUDGE),
        "{m:?}"
    );
    assert!(reqs[1]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .all(|b| !matches!(b, ContentBlock::Thinking { .. })));
    let notices = r.client.notices();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("repeating itself") && n.contains("nudge 1/2")),
        "{notices:?}"
    );
    // The aborted request's (estimated) tokens count toward the turn.
    assert!(r
        .usage
        .is_some_and(|u| u.output_tokens > 0 && u.input_tokens > 0));
    let _ = (&r.agent, &r.session);
}

#[tokio::test]
async fn second_loop_stops_the_turn_after_max_nudges() {
    let r = run(
        vec![Step::Repeat, Step::Repeat],
        "[thinking_guard]\nmax_nudges = 1\n",
    )
    .await;
    assert_eq!(r.stop, StopReason::ThinkingLoop);
    assert_eq!(r.stop.as_str(), "thinking_loop");
    assert_eq!(r.provider.aborted.load(Ordering::SeqCst), 2);
    let notices = r.client.notices();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("stopping this turn after 1 nudge")),
        "{notices:?}"
    );
    // Both aborted requests are accounted for.
    assert!(r.usage.is_some_and(|u| u.output_tokens > 0));
}

#[tokio::test]
async fn long_non_repeating_thinking_does_not_trip() {
    let r = run(vec![Step::ThinkThenAnswer(400, "ok")], "").await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert_eq!(r.provider.aborted.load(Ordering::SeqCst), 0);
    assert_eq!(r.provider.requests.lock().unwrap().len(), 1);
    assert!(!r.client.notices().iter().any(|n| n.contains("nudg")));
}

#[tokio::test]
async fn budget_trips_on_non_repeating_thinking() {
    let r = run(
        vec![Step::Unique(300), Step::Answer("ok")],
        "[thinking_guard]\nbudget_tokens = 200\n",
    )
    .await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert_eq!(r.provider.aborted.load(Ordering::SeqCst), 1);
    assert!(r
        .client
        .notices()
        .iter()
        .any(|n| n.contains("thinking past its budget")));
}

#[tokio::test]
async fn disabled_guard_lets_the_request_run() {
    // With the guard off the looping stream is never aborted by us; use a finite script.
    let r = run(
        vec![Step::ThinkThenAnswer(400, "ok")],
        "[thinking_guard]\nenabled = false\nbudget_tokens = 10\n",
    )
    .await;
    assert_eq!(r.stop, StopReason::EndTurn);
    assert_eq!(r.provider.requests.lock().unwrap().len(), 1);
}
