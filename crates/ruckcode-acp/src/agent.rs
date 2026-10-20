//! Serves a [`ruckcode_core::Agent`] over ACP.
//!
//! # Extension: `_ruckcode/ask_user_question` (agent -> client request)
//!
//! Sent only when the client advertised `clientCapabilities._meta.ruckcode.askUserQuestion = true`.
//!
//! ```json
//! // params
//! {"sessionId": "...", "questions": [
//!   {"question": "Which DB?", "header": "DB", "multiSelect": false,
//!    "options": [{"label": "sqlite", "description": "..."}, {"label": "postgres", "description": ""}]}]}
//! // result
//! {"answers": [{"question": "Which DB?", "selected": ["sqlite"], "other": null}], "cancelled": false}
//! ```
//!
//! `other` carries free text when the user chose "Other". `cancelled: true` aborts the tool call.
//! Without the capability, each question is posed through `session/request_permission` instead.
//!
//! # Extension: `_ruckcode/list_sessions` (client -> agent request)
//!
//! `{"cwd": "/path"}` -> `{"sessions": [{"id", "createdAt", "updatedAt", "firstPrompt", "mode"}]}`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use ruckcode_core::{
    Agent, Client, ModeInfo, PermissionDecision, PermissionRequest, SessionEvent, SessionInfo,
    StopReason as CoreStop, ToolStatus,
};
use ruckcode_tools::{Answer, Question, ToolKind as CoreKind, UserBridge};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::rpc::{Connection, Handler, RpcError};
use crate::schema::*;

pub const LIST_SESSIONS: &str = "_ruckcode/list_sessions";

struct Shared {
    agent: Arc<Agent>,
    conn: OnceLock<Connection>,
    caps: Mutex<ClientCapabilities>,
}

impl Shared {
    fn conn(&self) -> anyhow::Result<&Connection> {
        self.conn
            .get()
            .ok_or_else(|| anyhow::anyhow!("connection not ready"))
    }

    fn update(&self, session_id: &str, update: SessionUpdate) {
        let Ok(conn) = self.conn() else { return };
        let n = SessionNotification {
            session_id: session_id.to_string(),
            update,
        };
        if let Ok(v) = serde_json::to_value(&n) {
            // `notify` enqueues on an unbounded channel drained by a single writer task,
            // so notifications keep the order in which they were sent.
            let _ = conn.notify(SESSION_UPDATE, v);
        }
    }

    async fn select(
        &self,
        session_id: &str,
        tool_call: ToolCallUpdate,
        options: Vec<PermissionOption>,
    ) -> Option<String> {
        let conn = self.conn().ok()?;
        let req = RequestPermissionRequest {
            session_id: session_id.to_string(),
            tool_call,
            options,
        };
        let resp: RequestPermissionResponse = conn
            .request_typed(SESSION_REQUEST_PERMISSION, &req)
            .await
            .ok()?;
        match resp.outcome {
            RequestPermissionOutcome::Selected { option_id } => Some(option_id),
            RequestPermissionOutcome::Cancelled => None,
        }
    }
}

fn kind(k: CoreKind) -> ToolKind {
    match k {
        CoreKind::Read => ToolKind::Read,
        CoreKind::Edit => ToolKind::Edit,
        CoreKind::Delete => ToolKind::Delete,
        CoreKind::Move => ToolKind::Move,
        CoreKind::Search => ToolKind::Search,
        CoreKind::Execute => ToolKind::Execute,
        CoreKind::Think => ToolKind::Think,
        CoreKind::Fetch => ToolKind::Fetch,
        CoreKind::SwitchMode => ToolKind::SwitchMode,
        CoreKind::Other => ToolKind::Other,
    }
}

fn text(s: impl Into<String>) -> ContentBlock {
    ContentBlock::text(s)
}

fn convert_event(ev: SessionEvent) -> SessionUpdate {
    match ev {
        SessionEvent::UserText(t) => SessionUpdate::UserMessageChunk { content: text(t) },
        SessionEvent::AgentText(t) => SessionUpdate::AgentMessageChunk { content: text(t) },
        SessionEvent::AgentThought(t) => SessionUpdate::AgentThoughtChunk { content: text(t) },
        SessionEvent::ToolCall {
            id,
            title,
            kind: k,
            locations,
            input,
        } => SessionUpdate::ToolCall(ToolCall {
            tool_call_id: id,
            title,
            kind: kind(k),
            status: ToolCallStatus::Pending,
            content: vec![],
            locations: locations
                .into_iter()
                .map(|p| ToolCallLocation {
                    path: PathBuf::from(p),
                    line: None,
                })
                .collect(),
            raw_input: Some(input),
            raw_output: None,
        }),
        SessionEvent::ToolCallUpdate {
            id,
            status,
            output,
            edits,
        } => {
            let mut content = vec![];
            if let Some(o) = output.filter(|o| !o.is_empty()) {
                content.push(ToolCallContent::Content { content: text(o) });
            }
            content.extend(edits.into_iter().map(|e| ToolCallContent::Diff {
                path: e.path,
                old_text: e.old_text,
                new_text: e.new_text,
            }));
            SessionUpdate::ToolCallUpdate(ToolCallUpdate {
                tool_call_id: id,
                status: Some(match status {
                    ToolStatus::Pending => ToolCallStatus::Pending,
                    ToolStatus::InProgress => ToolCallStatus::InProgress,
                    ToolStatus::Completed => ToolCallStatus::Completed,
                    ToolStatus::Failed => ToolCallStatus::Failed,
                }),
                content: Some(content),
                ..Default::default()
            })
        }
        SessionEvent::Plan(items) => SessionUpdate::Plan {
            entries: items
                .into_iter()
                .map(|i| PlanEntry {
                    content: i.content,
                    priority: PlanPriority::Medium,
                    status: match i.status {
                        ruckcode_tools::TodoStatus::Pending => PlanStatus::Pending,
                        ruckcode_tools::TodoStatus::InProgress => PlanStatus::InProgress,
                        ruckcode_tools::TodoStatus::Completed => PlanStatus::Completed,
                    },
                })
                .collect(),
        },
        SessionEvent::ModeChanged(m) => SessionUpdate::CurrentModeUpdate { current_mode_id: m },
        SessionEvent::Notice(n) => SessionUpdate::AgentMessageChunk {
            content: text(format!("\n> {n}\n")),
        },
    }
}

/// The ACP-facing implementation of the core `Client` / `UserBridge` traits.
struct Bridge(Arc<Shared>);

#[async_trait]
impl UserBridge for Bridge {
    async fn ask_user(
        &self,
        session_id: &str,
        questions: Vec<Question>,
    ) -> anyhow::Result<Vec<Answer>> {
        let sh = &self.0;
        if sh.caps.lock().unwrap().supports_ask_user_question() {
            let req = AskUserQuestionRequest {
                session_id: session_id.to_string(),
                questions: questions
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<Result<_, _>>()?,
            };
            let resp: AskUserQuestionResponse = sh
                .conn()?
                .request_typed(ASK_USER_QUESTION, &req)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if resp.cancelled {
                anyhow::bail!("user cancelled");
            }
            return resp
                .answers
                .into_iter()
                .map(|a| serde_json::from_value(a).map_err(Into::into))
                .collect();
        }
        // Fallback: one permission prompt per question.
        let mut answers = vec![];
        for (qi, q) in questions.into_iter().enumerate() {
            let options = q
                .options
                .iter()
                .enumerate()
                .map(|(i, o)| PermissionOption {
                    option_id: format!("opt-{i}"),
                    name: o.label.clone(),
                    kind: PermissionOptionKind::AllowOnce,
                })
                .collect();
            let tc = ToolCallUpdate {
                tool_call_id: format!("ask-{session_id}-{qi}"),
                title: Some(q.question.clone()),
                kind: Some(ToolKind::Other),
                status: Some(ToolCallStatus::Pending),
                ..Default::default()
            };
            let chosen = sh
                .select(session_id, tc, options)
                .await
                .and_then(|id| id.strip_prefix("opt-")?.parse::<usize>().ok())
                .and_then(|i| q.options.get(i))
                .ok_or_else(|| anyhow::anyhow!("user cancelled"))?;
            answers.push(Answer {
                question: q.question.clone(),
                selected: vec![chosen.label.clone()],
                other: None,
            });
        }
        Ok(answers)
    }
}

#[async_trait]
impl Client for Bridge {
    async fn request_permission(
        &self,
        session_id: &str,
        req: PermissionRequest,
    ) -> PermissionDecision {
        let mut options = vec![PermissionOption {
            option_id: "allow_once".into(),
            name: "Allow".into(),
            kind: PermissionOptionKind::AllowOnce,
        }];
        if req.allow_always {
            options.push(PermissionOption {
                option_id: "allow_always".into(),
                name: "Always allow".into(),
                kind: PermissionOptionKind::AllowAlways,
            });
        }
        options.push(PermissionOption {
            option_id: "reject_once".into(),
            name: "Reject".into(),
            kind: PermissionOptionKind::RejectOnce,
        });
        let tc = ToolCallUpdate {
            tool_call_id: req.tool_call_id,
            title: Some(req.title),
            kind: Some(kind(req.kind)),
            status: Some(ToolCallStatus::Pending),
            raw_input: Some(req.input),
            ..Default::default()
        };
        match self.0.select(session_id, tc, options).await.as_deref() {
            Some("allow_once") => PermissionDecision::AllowOnce,
            Some("allow_always") => PermissionDecision::AllowAlways,
            Some(id) if id.starts_with("reject") => PermissionDecision::Reject,
            _ => PermissionDecision::Cancelled,
        }
    }

    fn send_event(&self, session_id: &str, event: SessionEvent) {
        self.0.update(session_id, convert_event(event));
    }
}

/// ACP request handler wrapping a core [`Agent`].
pub struct AcpAgent {
    shared: Arc<Shared>,
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::invalid_params(e.to_string()))
}

fn mode_state(info: &SessionInfo) -> SessionModeState {
    SessionModeState {
        current_mode_id: info.current_mode.clone(),
        available_modes: info
            .modes
            .iter()
            .map(
                |ModeInfo {
                     id,
                     name,
                     description,
                 }| SessionMode {
                    id: id.clone(),
                    name: name.clone(),
                    description: Some(description.clone()).filter(|d| !d.is_empty()),
                },
            )
            .collect(),
    }
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::internal(e.to_string()))
}

impl AcpAgent {
    pub fn new(agent: Arc<Agent>) -> Self {
        Self {
            shared: Arc::new(Shared {
                agent,
                conn: OnceLock::new(),
                caps: Mutex::default(),
            }),
        }
    }

    /// Provide the connection used for calls back to the client. Must be set once.
    pub fn set_connection(&self, conn: Connection) {
        let _ = self.shared.conn.set(conn);
    }

    fn client(&self) -> Arc<dyn Client> {
        Arc::new(Bridge(self.shared.clone()))
    }

    fn agent(&self) -> &Agent {
        &self.shared.agent
    }
}

#[async_trait]
impl Handler for AcpAgent {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        // The connection is installed right after `Connection::new`; tolerate the tiny race.
        for _ in 0..100 {
            if self.shared.conn.get().is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        match method {
            INITIALIZE => {
                let req: InitializeRequest = parse(params)?;
                *self.shared.caps.lock().unwrap() = req.client_capabilities;
                to_value(&InitializeResponse {
                    protocol_version: req.protocol_version.clamp(1, crate::PROTOCOL_VERSION),
                    agent_capabilities: AgentCapabilities {
                        load_session: true,
                        prompt_capabilities: PromptCapabilities {
                            image: false,
                            audio: false,
                            embedded_context: true,
                        },
                    },
                    auth_methods: vec![],
                    agent_info: Some(Implementation {
                        name: "ruckcode".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                    }),
                })
            }
            AUTHENTICATE => Ok(json!({})),
            SESSION_NEW => {
                let req: NewSessionRequest = parse(params)?;
                let info = self.agent().new_session(req.cwd, None).await?;
                to_value(&NewSessionResponse {
                    modes: Some(mode_state(&info)),
                    session_id: info.id,
                })
            }
            SESSION_LOAD => {
                let req: LoadSessionRequest = parse(params)?;
                let info = self
                    .agent()
                    .load_session(&req.session_id, req.cwd, self.client())
                    .await?;
                to_value(&LoadSessionResponse {
                    modes: Some(mode_state(&info)),
                })
            }
            SESSION_PROMPT => {
                let req: PromptRequest = parse(params)?;
                let stop = self
                    .agent()
                    .prompt(&req.session_id, prompt_to_text(&req.prompt), self.client())
                    .await?;
                to_value(&PromptResponse {
                    stop_reason: match stop {
                        CoreStop::EndTurn => StopReason::EndTurn,
                        CoreStop::MaxTokens => StopReason::MaxTokens,
                        CoreStop::MaxTurnRequests => StopReason::MaxTurnRequests,
                        CoreStop::Cancelled => StopReason::Cancelled,
                        CoreStop::Refusal => StopReason::Refusal,
                    },
                })
            }
            SESSION_SET_MODE => {
                let req: SetSessionModeRequest = parse(params)?;
                // The agent emits `ModeChanged`, which the bridge forwards as current_mode_update.
                self.agent()
                    .set_mode(&req.session_id, &req.mode_id, self.client())
                    .await?;
                Ok(json!({}))
            }
            LIST_SESSIONS => {
                let cwd: PathBuf = params
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .ok_or_else(|| RpcError::invalid_params("missing cwd"))?;
                let sessions: Vec<Value> = self
                    .agent()
                    .list_sessions(&cwd)?
                    .into_iter()
                    .map(|s| {
                        json!({
                            "id": s.id,
                            "createdAt": s.created_at,
                            "updatedAt": s.updated_at,
                            "firstPrompt": s.first_prompt,
                            "mode": s.mode,
                        })
                    })
                    .collect();
                Ok(json!({ "sessions": sessions }))
            }
            m => Err(RpcError::method_not_found(m)),
        }
    }

    async fn notification(&self, method: &str, params: Value) {
        if method == SESSION_CANCEL {
            if let Ok(n) = serde_json::from_value::<CancelNotification>(params) {
                self.agent().cancel(&n.session_id);
            }
        }
    }
}

/// Serve over arbitrary streams; returns when the peer disconnects.
pub async fn serve<R, W>(agent: Arc<Agent>, reader: R, writer: W) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let acp = Arc::new(AcpAgent::new(agent));
    let (conn, join) = Connection::new(reader, writer, acp.clone());
    acp.set_connection(conn);
    join.await?;
    Ok(())
}

pub async fn serve_stdio(agent: Arc<Agent>) -> anyhow::Result<()> {
    serve(agent, tokio::io::stdin(), tokio::io::stdout()).await
}

/// Wire an agent to a client-side handler over a duplex pipe; returns the client-side connection.
pub fn spawn_in_process(agent: Arc<Agent>, client_handler: Arc<dyn Handler>) -> Connection {
    let (a, b) = tokio::io::duplex(1024 * 1024);
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    tokio::spawn(async move {
        let _ = serve(agent, ar, aw).await;
    });
    let (client, _) = Connection::new(br, bw, client_handler);
    client
}
