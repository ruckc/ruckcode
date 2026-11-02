//! Rate limits and transient failures: retries, waits, cancellation, resumable stops and pacing.

use async_trait::async_trait;
use ruckcode_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, RateLimitEvent,
    SessionEvent, StopReason,
};
use ruckcode_core::transcript::{assess_unfinished, session_path, Record, TranscriptReader};
use ruckcode_llm::mock::MockProvider;
use ruckcode_llm::openai::OpenAi;
use ruckcode_llm::{
    ApiError, LimitKind, Message, Provider, RateLimitInfo, Request, Response, Role,
    StopReason as LlmStop, StreamEvent, Usage,
};
use ruckcode_tools::{Answer, Question, UserBridge};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
}

impl TestClient {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn events(&self) -> Vec<SessionEvent> {
        self.events.lock().unwrap().clone()
    }
    fn rate_limit_events(&self) -> Vec<RateLimitEvent> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                SessionEvent::RateLimit(r) => Some(r),
                _ => None,
            })
            .collect()
    }
    fn text(&self) -> String {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                SessionEvent::AgentText(t) => Some(t),
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
}

impl Env {
    fn new(config: &str) -> Self {
        let proj = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(proj.path().join(".ruckcode/config.toml"), config).unwrap();
        Self { proj, user }
    }
    fn agent(&self, p: Arc<dyn Provider>) -> Agent {
        Agent::new(AgentOptions {
            provider_override: Some(p),
            user_config_dir: Some(self.user.path().to_path_buf()),
            ..Default::default()
        })
    }
    fn cwd(&self) -> PathBuf {
        self.proj.path().to_path_buf()
    }
    fn records(&self, id: &str) -> Vec<Record> {
        TranscriptReader::load(&session_path(&self.cwd(), id))
            .unwrap()
            .records
            .into_iter()
            .map(|r| r.record)
            .collect()
    }
    fn unfinished(&self, id: &str) -> bool {
        let loaded = TranscriptReader::load(&session_path(&self.cwd(), id)).unwrap();
        assess_unfinished(&loaded.records).interrupted
    }
}

fn limited(headers: &[(&str, &str)], body: &str) -> anyhow::Error {
    let h: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let info = RateLimitInfo::parse(&h, body, SystemTime::now());
    ApiError::new("test", 429, body.into(), info).into()
}

const OWNER_BODY: &str = r#"{"error":{"type":"rate_limit_exceeded","message":"slow","param":null,"code":"tokens_per_minute_exceeded","retry_after_seconds":0.05}}"#;

const DAILY_HEADERS: &[(&str, &str)] = &[
    ("RetryAfter", "51"),
    ("X-RateLimit-Limit-Credits-Day", "2000000"),
    ("X-RateLimit-Remaining-Credits-Day", "0"),
    ("X-RateLimit-Reset-Credits-Day", "3600"),
    ("X-RateLimit-Limit-Tokens-Minute", "500000"),
    ("X-RateLimit-Remainint-Tokens-Minute", "0"),
    ("X-RateLimit-Reset-Tokens-Minute", "51"),
];

fn assistant_texts(records: &[Record]) -> Vec<String> {
    records
        .iter()
        .filter_map(|r| match r {
            Record::AssistantMessage { message, .. } => Some(message.text()),
            _ => None,
        })
        .collect()
}

fn turn_ends(records: &[Record]) -> Vec<String> {
    records
        .iter()
        .filter_map(|r| match r {
            Record::TurnEnd { stop_reason, .. } => Some(stop_reason.clone()),
            _ => None,
        })
        .collect()
}

// ---- mock provider: policy through the agent loop ----------------------------------------

#[tokio::test]
async fn short_rate_limit_waits_and_the_turn_succeeds() {
    let env = Env::new("");
    let mock = Arc::new(MockProvider::default());
    mock.push_failure(limited(&[("RetryAfter", "0.05")], OWNER_BODY));
    mock.push_failure(limited(&[], OWNER_BODY));
    mock.push(Message::assistant_text("done"));
    let agent = env.agent(mock.clone());
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::new();
    let r = agent
        .prompt(&info.id, "hi".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);
    assert_eq!(mock.requests.lock().unwrap().len(), 3);
    // Only the successful response is in the transcript, once.
    let recs = env.records(&info.id);
    assert_eq!(assistant_texts(&recs), ["done"]);
    assert_eq!(turn_ends(&recs), ["end_turn"]);
    assert!(!env.unfinished(&info.id));
    // The client saw a wait for each retry, with the limit and attempt, and each was cleared.
    let ev = client.rate_limit_events();
    let waits: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            RateLimitEvent::Waiting {
                reason,
                limit,
                attempt,
                resume_at_ms,
                ..
            } => Some((reason.clone(), *limit, *attempt, *resume_at_ms)),
            _ => None,
        })
        .collect();
    assert_eq!(waits.len(), 2);
    assert_eq!(waits[0].0, "Rate limited (tokens/minute)");
    assert_eq!(waits[0].1, Some(LimitKind::TokensPerMinute));
    assert_eq!((waits[0].2, waits[1].2), (1, 2));
    assert!(waits[0].3 > 0);
    assert_eq!(
        ev.iter()
            .filter(|e| matches!(e, RateLimitEvent::Resumed))
            .count(),
        2
    );
}

#[tokio::test]
async fn daily_budget_ends_the_turn_resumably_and_the_session_stays_usable() {
    let env = Env::new("");
    let mock = Arc::new(MockProvider::default());
    mock.push_failure(limited(DAILY_HEADERS, OWNER_BODY));
    let agent = env.agent(mock.clone());
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::new();
    let started = std::time::Instant::now();
    let r = agent
        .prompt(&info.id, "first".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::RateLimited);
    assert_eq!(r.as_str(), "rate_limited");
    assert!(started.elapsed() < Duration::from_secs(5), "must not wait");
    assert_eq!(mock.requests.lock().unwrap().len(), 1, "no spinning");
    // Message names the budget, the reset and how to continue.
    let stopped = client
        .rate_limit_events()
        .into_iter()
        .find_map(|e| match e {
            RateLimitEvent::Stopped {
                limit,
                resets_at_ms,
                message,
            } => Some((limit, resets_at_ms, message)),
            _ => None,
        })
        .expect("stopped event");
    assert_eq!(stopped.0, LimitKind::DailyCredits);
    assert!(stopped.1.is_some());
    assert!(stopped.2.contains("daily credit budget"), "{}", stopped.2);
    assert!(stopped.2.contains("1h 0m"), "{}", stopped.2);
    assert!(stopped.2.contains("send a message"), "{}", stopped.2);
    assert!(client
        .events()
        .iter()
        .any(|e| matches!(e, SessionEvent::Notice(n) if n.contains("daily credit budget"))));
    // Flagged unfinished so the resume prompt offers it.
    let recs = env.records(&info.id);
    assert_eq!(turn_ends(&recs), ["rate_limited"]);
    assert!(env.unfinished(&info.id));

    // The next message works normally: roles alternate, nothing is merged into the failed one.
    mock.push(Message::assistant_text("second reply"));
    let r2 = agent
        .prompt(&info.id, "second".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r2, StopReason::EndTurn);
    let reqs = mock.requests.lock().unwrap();
    let roles: Vec<Role> = reqs[1].messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
    assert_eq!(reqs[1].messages[2].text(), "second");
    drop(reqs);
    assert_eq!(
        turn_ends(&env.records(&info.id)),
        ["rate_limited", "end_turn"]
    );
    assert!(!env.unfinished(&info.id));
}

#[tokio::test]
async fn max_auto_wait_is_configurable() {
    let env = Env::new("[rate_limit]\nmax_auto_wait_secs = 1\n");
    let mock = Arc::new(MockProvider::default());
    mock.push_failure(limited(&[("Retry-After", "30")], ""));
    let agent = env.agent(mock.clone());
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let r = agent
        .prompt(&info.id, "x".into(), TestClient::new())
        .await
        .unwrap();
    assert_eq!(r, StopReason::RateLimited);
}

#[tokio::test]
async fn retries_run_out_as_rate_limited() {
    let env = Env::new("[rate_limit]\nmax_retries = 2\n");
    let mock = Arc::new(MockProvider::default());
    for _ in 0..3 {
        mock.push_failure(limited(&[("retry-after-ms", "10")], ""));
    }
    let agent = env.agent(mock.clone());
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let r = agent
        .prompt(&info.id, "x".into(), TestClient::new())
        .await
        .unwrap();
    assert_eq!(r, StopReason::RateLimited);
    assert_eq!(mock.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn cancelling_a_wait_ends_the_turn_as_cancelled() {
    let env = Env::new("");
    let mock = Arc::new(MockProvider::default());
    mock.push_failure(limited(&[("Retry-After", "30")], ""));
    let agent = Arc::new(env.agent(mock.clone()));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::new();
    let (a, id, c) = (agent.clone(), info.id.clone(), client.clone());
    let task = tokio::spawn(async move { a.prompt(&id, "go".into(), c).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(client
        .rate_limit_events()
        .iter()
        .any(|e| matches!(e, RateLimitEvent::Waiting { wait_ms, .. } if *wait_ms == 30_000)));
    agent.cancel(&info.id);
    let r = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(r, StopReason::Cancelled);
    assert_eq!(turn_ends(&env.records(&info.id)), ["cancelled"]);
    assert!(!env.unfinished(&info.id), "a cancelled turn is settled");
    // The countdown is cleared.
    assert!(matches!(
        client.rate_limit_events().last(),
        Some(RateLimitEvent::Resumed)
    ));
}

#[tokio::test]
async fn other_client_errors_are_not_retried_and_close_the_turn() {
    let env = Env::new("");
    let mock = Arc::new(MockProvider::default());
    mock.push_failure(ApiError::new(
        "test",
        401,
        "bad key".into(),
        RateLimitInfo::default(),
    ));
    let agent = env.agent(mock.clone());
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let err = agent
        .prompt(&info.id, "x".into(), TestClient::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("401"));
    assert_eq!(mock.requests.lock().unwrap().len(), 1);
    assert_eq!(turn_ends(&env.records(&info.id)), ["error"]);
    // The failed prompt no longer dangles: the next prompt is its own message.
    mock.push(Message::assistant_text("fine"));
    agent
        .prompt(&info.id, "again".into(), TestClient::new())
        .await
        .unwrap();
    let reqs = mock.requests.lock().unwrap();
    let roles: Vec<Role> = reqs[1].messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
}

#[tokio::test]
async fn proactive_pacing_waits_for_the_reset_instead_of_sending() {
    let env = Env::new("");
    let mock = Arc::new(MockProvider::default());
    let headers = |remaining: &str| -> Vec<(String, String)> {
        vec![
            ("x-ratelimit-limit-tokens-minute".into(), "1000".into()),
            (
                "x-ratelimit-remaining-tokens-minute".into(),
                remaining.into(),
            ),
            ("x-ratelimit-reset-tokens-minute".into(), "1".into()),
        ]
    };
    mock.set_rate_limit(Some(RateLimitInfo::parse(
        &headers("0"),
        "",
        SystemTime::now(),
    )));
    mock.push(Message::assistant_text("one"));
    mock.push(Message::assistant_text("two"));
    let agent = env.agent(mock.clone());
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::new();
    agent
        .prompt(&info.id, "a".into(), client.clone())
        .await
        .unwrap();
    assert!(
        client.rate_limit_events().is_empty(),
        "first request goes out"
    );
    let t = std::time::Instant::now();
    agent
        .prompt(&info.id, "b".into(), client.clone())
        .await
        .unwrap();
    assert!(
        t.elapsed() >= Duration::from_millis(800),
        "{:?}",
        t.elapsed()
    );
    assert!(matches!(
        client.rate_limit_events().first(),
        Some(RateLimitEvent::Waiting {
            attempt: 0,
            limit: Some(LimitKind::TokensPerMinute),
            ..
        })
    ));
    assert_eq!(mock.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn pacing_can_be_disabled() {
    let env = Env::new("[rate_limit]\npacing = false\n");
    let mock = Arc::new(MockProvider::default());
    mock.set_rate_limit(Some(RateLimitInfo::parse(
        &[
            ("x-ratelimit-remaining-tokens-minute".into(), "0".into()),
            ("x-ratelimit-reset-tokens-minute".into(), "30".into()),
        ],
        "",
        SystemTime::now(),
    )));
    mock.push(Message::assistant_text("one"));
    mock.push(Message::assistant_text("two"));
    let agent = env.agent(mock);
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let t = std::time::Instant::now();
    for p in ["a", "b"] {
        agent
            .prompt(&info.id, p.into(), TestClient::new())
            .await
            .unwrap();
    }
    assert!(t.elapsed() < Duration::from_secs(5));
}

/// Streams some text, then fails with a 503: the partial text must not reach the transcript.
struct Flaky {
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for Flaky {
    fn name(&self) -> &str {
        "flaky"
    }
    async fn complete(
        &self,
        _: Request,
        events: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let _ = events.send(StreamEvent::TextDelta("partial ".into()));
            let info = RateLimitInfo::parse(
                &[("retry-after-ms".into(), "20".into())],
                "",
                SystemTime::now(),
            );
            return Err(ApiError::new("flaky", 503, "unavailable".into(), info).into());
        }
        let _ = events.send(StreamEvent::TextDelta("complete answer".into()));
        Ok(Response {
            message: Message::assistant_text("complete answer"),
            stop_reason: LlmStop::EndTurn,
            usage: Usage::default(),
            rate_limit: None,
        })
    }
}

#[tokio::test]
async fn mid_stream_failure_restarts_without_duplicating_the_transcript() {
    let env = Env::new("");
    let agent = env.agent(Arc::new(Flaky {
        calls: AtomicUsize::new(0),
    }));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::new();
    let r = agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);
    let recs = env.records(&info.id);
    assert_eq!(assistant_texts(&recs), ["complete answer"]);
    // The live view is told the partial output was discarded.
    assert!(client
        .events()
        .iter()
        .any(|e| matches!(e, SessionEvent::Notice(n) if n.contains("cut off"))));
    assert!(client.text().ends_with("complete answer"));
}

// ---- real HTTP client against a local server ------------------------------------------------

struct Canned {
    status: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
}

struct Server {
    url: String,
    queue: Arc<Mutex<VecDeque<Canned>>>,
    hits: Arc<AtomicUsize>,
}

fn sse(chunks: &[&str]) -> String {
    let mut out = String::new();
    for (i, c) in chunks.iter().enumerate() {
        let finish = if i + 1 == chunks.len() {
            r#","finish_reason":"stop""#
        } else {
            ""
        };
        out.push_str(&format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":{}}}{finish}}}]}}\n\n",
            serde_json::to_string(c).unwrap()
        ));
    }
    out.push_str("data: [DONE]\n\n");
    out
}

async fn serve() -> Server {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for k in ["NO_PROXY", "no_proxy"] {
            // Runs once before any client in this process is built.
            std::env::set_var(k, "127.0.0.1,localhost");
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let queue: Arc<Mutex<VecDeque<Canned>>> = Arc::default();
    let hits = Arc::new(AtomicUsize::new(0));
    let (q, h) = (queue.clone(), hits.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let (q, h) = (q.clone(), h.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head_end, len) = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..p]).to_ascii_lowercase();
                        let cl = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                        break (p + 4, cl);
                    }
                };
                while buf.len() < head_end + len {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                h.fetch_add(1, Ordering::SeqCst);
                let c = q.lock().unwrap().pop_front().unwrap_or(Canned {
                    status: "500 Internal Server Error",
                    headers: vec![],
                    body: "script exhausted".into(),
                });
                let ct = if c.status.starts_with("200") {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let mut head = format!("HTTP/1.1 {}\r\ncontent-type: {ct}\r\n", c.status);
                for (k, v) in &c.headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str(&format!(
                    "content-length: {}\r\nconnection: close\r\n\r\n",
                    c.body.len()
                ));
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(c.body.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Server { url, queue, hits }
}

impl Server {
    fn push(&self, c: Canned) {
        self.queue.lock().unwrap().push_back(c);
    }
}

#[tokio::test]
async fn http_429_then_200_succeeds_without_duplicated_content() {
    let server = serve().await;
    server.push(Canned {
        status: "429 Too Many Requests",
        headers: vec![
            ("RetryAfter", "0"),
            ("X-RateLimit-Limit-Tokens-Minute", "500000"),
            ("X-RateLimit-Remainint-Tokens-Minute", "0"),
            ("X-RateLimit-Reset-Tokens-Minute", "1"),
        ],
        body: OWNER_BODY.into(),
    });
    server.push(Canned {
        status: "200 OK",
        headers: vec![
            ("X-RateLimit-Limit-Tokens-Minute", "500000"),
            ("X-RateLimit-Remaining-Tokens-Minute", "450000"),
        ],
        body: sse(&["hello ", "world"]),
    });
    let env = Env::new("");
    let agent = env.agent(Arc::new(OpenAi::new(None, Some(server.url.clone()))));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::new();
    let r = agent
        .prompt(&info.id, "hi".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);
    assert_eq!(server.hits.load(Ordering::SeqCst), 2);
    let recs = env.records(&info.id);
    assert_eq!(assistant_texts(&recs), ["hello world"]);
    assert_eq!(client.text(), "hello world");
    assert_eq!(turn_ends(&recs), ["end_turn"]);
    assert!(client
        .rate_limit_events()
        .iter()
        .any(|e| matches!(e, RateLimitEvent::Waiting { reason, .. } if reason == "Rate limited (tokens/minute)")));
}

#[tokio::test]
async fn http_daily_budget_429_ends_rate_limited_then_next_turn_works() {
    let server = serve().await;
    server.push(Canned {
        status: "429 Too Many Requests",
        headers: DAILY_HEADERS.to_vec(),
        body: OWNER_BODY.into(),
    });
    let env = Env::new("");
    let agent = env.agent(Arc::new(OpenAi::new(None, Some(server.url.clone()))));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::new();
    let r = agent
        .prompt(&info.id, "hi".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::RateLimited);
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    assert!(env.unfinished(&info.id));
    assert_eq!(turn_ends(&env.records(&info.id)), ["rate_limited"]);

    // Daily budgets are not gated client-side (headers may be stale): the server decides.
    server.push(Canned {
        status: "429 Too Many Requests",
        headers: DAILY_HEADERS.to_vec(),
        body: OWNER_BODY.into(),
    });
    let r2 = agent
        .prompt(&info.id, "again".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r2, StopReason::RateLimited);
    assert_eq!(server.hits.load(Ordering::SeqCst), 2);

    // A new agent (fresh process, nothing remembered) can continue once the server accepts.
    server.push(Canned {
        status: "200 OK",
        headers: vec![],
        body: sse(&["back ", "again"]),
    });
    let agent2 = env.agent(Arc::new(OpenAi::new(None, Some(server.url.clone()))));
    agent2
        .load_session(&info.id, env.cwd(), TestClient::new())
        .await
        .unwrap();
    let r3 = agent2
        .prompt(&info.id, "continue".into(), TestClient::new())
        .await
        .unwrap();
    assert_eq!(r3, StopReason::EndTurn);
    let recs = env.records(&info.id);
    assert_eq!(assistant_texts(&recs).last().unwrap(), "back again");
    assert!(!env.unfinished(&info.id));
}
