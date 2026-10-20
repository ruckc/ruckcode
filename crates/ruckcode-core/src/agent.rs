//! The agent loop: sessions, model requests, tool execution, hooks, modes and compaction.

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use ruckcode_hooks::{fix_turn_prompt, HookEvent, HookOutcome, HookRunner, HooksConfig, OnFailure};
use ruckcode_llm::{
    ContentBlock, Message, Provider, Request, Response, Role, StopReason as LlmStop, StreamEvent,
};
use ruckcode_mcp::{McpManager, McpServerConfig};
use ruckcode_mise::{MiseTask, TaskSelector};
use ruckcode_tools::{FileEdit, TodoItem, Tool, ToolContext, ToolKind, ToolOutput, UserBridge};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;

use crate::config::{make_provider, Config, Profile};
use crate::modes::{ModeDef, ModeSet};
use crate::permissions::{self, Permission};
use crate::prompt::system_prompt;
use crate::transcript::{
    self, compaction_message, interrupted_results, EditRecord, Initiator, McpServerRecord, Metrics,
    PermissionOutcome, Record, SessionMeta, SessionSummary, TranscriptReader, TranscriptWriter,
    UserSource,
};

const SWITCH_MODE: &str = "SwitchMode";
const INTERRUPTED: &str = "[Request interrupted by user]";
const SUMMARY_PROMPT: &str = "Summarize the conversation so far so that work can continue from \
the summary alone. Cover: the user's goals, decisions made, files read or changed, the current \
state of the work, and concrete next steps. Be thorough but compact. Do not call any tools.";

#[derive(Debug, Clone, PartialEq)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// Replayed user prompt when loading a session.
    UserText(String),
    AgentText(String),
    AgentThought(String),
    ToolCall {
        id: String,
        title: String,
        kind: ToolKind,
        locations: Vec<String>,
        input: Value,
    },
    ToolCallUpdate {
        id: String,
        status: ToolStatus,
        output: Option<String>,
        edits: Vec<FileEdit>,
    },
    Plan(Vec<TodoItem>),
    ModeChanged(String),
    /// Informational message for the user.
    Notice(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    AllowAlways,
    Reject,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequest {
    pub tool_call_id: String,
    pub title: String,
    pub kind: ToolKind,
    pub input: Value,
    pub allow_always: bool,
}

/// The front end driving the agent (ACP binding, TUI, tests).
#[async_trait]
pub trait Client: UserBridge {
    async fn request_permission(
        &self,
        session_id: &str,
        req: PermissionRequest,
    ) -> PermissionDecision;

    fn send_event(&self, session_id: &str, event: SessionEvent);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Cancelled,
    Refusal,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::MaxTurnRequests => "max_turn_requests",
            StopReason::Cancelled => "cancelled",
            StopReason::Refusal => "refusal",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub current_mode: String,
    pub modes: Vec<ModeInfo>,
}

pub struct AgentOptions {
    pub profile_override: Option<String>,
    pub model_override: Option<String>,
    /// Use this provider for every request (tests).
    pub provider_override: Option<Arc<dyn Provider>>,
    /// Write transcripts to `<project>/.ruckcode/sessions`.
    pub persist: bool,
    /// Directory holding user `config.toml` / `modes.toml`; `None` means `~/.config/ruckcode`.
    pub user_config_dir: Option<PathBuf>,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            profile_override: None,
            model_override: None,
            provider_override: None,
            persist: true,
            user_config_dir: None,
        }
    }
}

struct SessionState {
    id: String,
    cwd: PathBuf,
    config: Config,
    modes: ModeSet,
    mise_tasks: Vec<MiseTask>,
    /// MCP servers passed by the client (ACP `mcpServers`); override same-named config entries.
    extra_mcp: BTreeMap<String, McpServerConfig>,
    /// Connected lazily on the first prompt.
    mcp: Option<McpManager>,
    mode: String,
    profile: String,
    messages: Vec<Message>,
    allow_always: HashSet<String>,
    edited: BTreeSet<PathBuf>,
    transcript: Option<TranscriptWriter>,
    providers: HashMap<String, Arc<dyn Provider>>,
    started: bool,
    request_counter: u64,
}

impl SessionState {
    fn rec(&mut self, r: Record) {
        if let Some(w) = &mut self.transcript {
            if let Err(e) = w.append(&r) {
                tracing::warn!("failed to write transcript: {e:#}");
            }
        }
    }

    fn push(&mut self, m: Message, source: UserSource) {
        self.rec(Record::UserMessage {
            message: m.clone(),
            source,
        });
        self.messages.push(m);
    }

    /// Close a user message that never got a reply so roles keep alternating.
    fn push_interruption(&mut self) {
        let message = Message::assistant_text(INTERRUPTED);
        self.request_counter += 1;
        let request_id = self.request_counter;
        self.rec(Record::AssistantMessage {
            message: message.clone(),
            request_id,
            model: String::new(),
            stop_reason: LlmStop::EndTurn,
            usage: Default::default(),
            metrics: Metrics {
                duration_ms: 0,
                time_to_first_token_ms: None,
                output_tokens_per_sec: None,
            },
        });
        self.messages.push(message);
    }

    fn mode_def(&self) -> &ModeDef {
        self.modes.get(&self.mode).expect("current mode exists")
    }

    fn hooks(&self) -> HooksConfig {
        self.config.hooks.merged(&self.mode_def().hooks)
    }

    fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            current_mode: self.mode.clone(),
            modes: self
                .modes
                .modes
                .iter()
                .map(|(id, m)| ModeInfo {
                    id: id.clone(),
                    name: self.modes.display_name(id),
                    description: m.description.clone(),
                })
                .collect(),
        }
    }

    fn record_hooks(&mut self, event: HookEvent, outcomes: &[HookOutcome]) {
        for o in outcomes {
            self.rec(Record::HookRun {
                event: event.as_str().into(),
                command: o.spec.run.clone(),
                success: o.success,
                exit_code: o.exit_code,
                output: o.output.clone(),
                duration_ms: o.duration_ms,
            });
        }
    }
}

struct TurnStats {
    started: Instant,
    model_requests: u32,
    tool_calls: u32,
    input_tokens: u64,
    output_tokens: u64,
}

/// Agent-level tool for proposing a mode transition; executed by the agent itself.
struct SwitchModeTool {
    /// (id, display name, description)
    targets: Vec<(String, String, String)>,
}

#[async_trait]
impl Tool for SwitchModeTool {
    fn name(&self) -> &str {
        SWITCH_MODE
    }

    fn description(&self) -> String {
        let mut d = String::from(
            "Propose switching to another lifecycle mode when the current phase's work is done. \
             The user may need to approve. Available modes:\n",
        );
        for (id, _, desc) in &self.targets {
            d.push_str(&format!("- {id}: {desc}\n"));
        }
        d
    }

    fn input_schema(&self) -> Value {
        let ids: Vec<&str> = self.targets.iter().map(|(i, _, _)| i.as_str()).collect();
        json!({
            "type": "object",
            "properties": {
                "mode_id": {"type": "string", "enum": ids},
                "reason": {"type": "string", "description": "Why the switch is appropriate now"}
            },
            "required": ["mode_id", "reason"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::SwitchMode
    }

    fn title(&self, input: &Value) -> String {
        let id = input["mode_id"].as_str().unwrap_or_default();
        let name = self
            .targets
            .iter()
            .find(|(i, _, _)| i == id)
            .map_or(id, |(_, n, _)| n.as_str());
        let reason = input["reason"].as_str().unwrap_or_default();
        format!("Switch to {name} mode: {reason}")
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    async fn call(&self, _: Value, _: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::error("SwitchMode is handled by the agent"))
    }
}

pub struct Agent {
    options: AgentOptions,
    sessions: Mutex<HashMap<String, Arc<tokio::sync::Mutex<SessionState>>>>,
    cancels: Mutex<HashMap<String, CancellationToken>>,
    shared: Mutex<HashMap<String, Arc<Shared>>>,
}

/// Per-session data reachable without the session lock.
struct Shared {
    mode_ids: Vec<String>,
    /// Mode requested by the user while a prompt was running; applied before the next request.
    pending_mode: Mutex<Option<String>>,
}

impl Agent {
    pub fn new(options: AgentOptions) -> Self {
        Self {
            options,
            sessions: Mutex::default(),
            cancels: Mutex::default(),
            shared: Mutex::default(),
        }
    }

    async fn build_state(
        &self,
        id: String,
        cwd: PathBuf,
        mode: Option<String>,
        extra_mcp: BTreeMap<String, McpServerConfig>,
    ) -> Result<SessionState> {
        let user_dir = self.options.user_config_dir.as_deref();
        let config = Config::load(&cwd, user_dir)?;
        let modes = ModeSet::load(&cwd, user_dir)?;
        let profile = self
            .options
            .profile_override
            .clone()
            .unwrap_or_else(|| config.default_profile.clone());
        config.profile(&profile)?;
        let mode = mode.unwrap_or_else(|| modes.default_mode.clone());
        if modes.get(&mode).is_none() {
            bail!("unknown mode `{mode}`");
        }
        let mise_tasks = ruckcode_mise::discover(&cwd).await;
        Ok(SessionState {
            id,
            cwd,
            config,
            modes,
            mise_tasks,
            extra_mcp,
            mcp: None,
            mode,
            profile,
            messages: vec![],
            allow_always: HashSet::new(),
            edited: BTreeSet::new(),
            transcript: None,
            providers: HashMap::new(),
            started: false,
            request_counter: 0,
        })
    }

    /// Resolve (model id, profile) for the session's current mode. The returned profile's
    /// `model` is the effective model, so provider caching keys on it.
    fn model_for(&self, s: &SessionState) -> Result<(String, Profile)> {
        let mode = s.mode_def();
        let mut profile_name = mode.profile.clone();
        let mut mode_model = mode.model.clone();
        if profile_name.is_none() {
            if let Some(m) = &mode_model {
                // Legacy: `model` used to name a profile.
                if s.config.profiles.contains_key(m) {
                    profile_name = mode_model.take();
                }
            }
        }
        let other_profile = profile_name.as_ref().is_some_and(|p| *p != s.profile);
        let mut profile = s
            .config
            .profile(profile_name.as_deref().unwrap_or(&s.profile))?
            .clone();
        // `--model` is an id for the session profile, so it never applies to a mode that
        // picks a different profile or its own model.
        let model = match (mode_model, &self.options.model_override) {
            (Some(m), _) => m,
            (None, Some(m)) if !other_profile => m.clone(),
            _ => profile.model.clone(),
        };
        profile.model = model.clone();
        Ok((model, profile))
    }

    fn provider_for(&self, s: &mut SessionState, profile: &Profile) -> Result<Arc<dyn Provider>> {
        if let Some(p) = &self.options.provider_override {
            return Ok(p.clone());
        }
        let key = format!(
            "{:?}/{}/{:?}/{:?}",
            profile.provider, profile.model, profile.base_url, profile.api_key_env
        );
        if let Some(p) = s.providers.get(&key) {
            return Ok(p.clone());
        }
        let p = make_provider(profile)?;
        s.providers.insert(key, p.clone());
        Ok(p)
    }

    fn register(&self, s: SessionState) -> SessionInfo {
        let info = s.info();
        self.shared.lock().unwrap().insert(
            s.id.clone(),
            Arc::new(Shared {
                mode_ids: s.modes.modes.iter().map(|(i, _)| i.clone()).collect(),
                pending_mode: Mutex::new(None),
            }),
        );
        self.sessions
            .lock()
            .unwrap()
            .insert(s.id.clone(), Arc::new(tokio::sync::Mutex::new(s)));
        info
    }

    fn session(&self, id: &str) -> Result<Arc<tokio::sync::Mutex<SessionState>>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown session `{id}`"))
    }

    pub async fn new_session(
        &self,
        cwd: PathBuf,
        initial_mode: Option<String>,
    ) -> Result<SessionInfo> {
        self.new_session_with(cwd, initial_mode, BTreeMap::new())
            .await
    }

    /// Like [`new_session`](Self::new_session), plus extra MCP servers for this session only.
    pub async fn new_session_with(
        &self,
        cwd: PathBuf,
        initial_mode: Option<String>,
        extra_mcp: BTreeMap<String, McpServerConfig>,
    ) -> Result<SessionInfo> {
        let id = transcript::new_session_id();
        let mut s = self
            .build_state(id.clone(), cwd, initial_mode, extra_mcp)
            .await?;
        if self.options.persist {
            let (model, _) = self.model_for(&s)?;
            s.transcript = Some(TranscriptWriter::create(
                &s.cwd,
                SessionMeta {
                    id,
                    cwd: s.cwd.display().to_string(),
                    created_at: transcript::now_rfc3339(),
                    ruckcode_version: env!("CARGO_PKG_VERSION").into(),
                    profile: s.profile.clone(),
                    model,
                    mode: s.mode.clone(),
                },
            )?);
        }
        Ok(self.register(s))
    }

    /// Restore a session from its transcript and replay its history to `client`.
    pub async fn load_session(
        &self,
        id: &str,
        cwd: PathBuf,
        client: Arc<dyn Client>,
    ) -> Result<SessionInfo> {
        self.load_session_with(id, cwd, client, BTreeMap::new())
            .await
    }

    /// Like [`load_session`](Self::load_session), plus extra MCP servers for this session only.
    pub async fn load_session_with(
        &self,
        id: &str,
        cwd: PathBuf,
        client: Arc<dyn Client>,
        extra_mcp: BTreeMap<String, McpServerConfig>,
    ) -> Result<SessionInfo> {
        let path = transcript::session_path(&cwd, id);
        let loaded = TranscriptReader::load(&path)?;
        let mode = Some(loaded.mode.clone()).filter(|m| {
            ModeSet::load(&cwd, self.options.user_config_dir.as_deref())
                .map(|ms| ms.get(m).is_some())
                .unwrap_or(false)
        });
        let mut s = self
            .build_state(id.to_string(), cwd.clone(), mode, extra_mcp)
            .await?;
        s.started = true;
        s.messages = loaded.messages.clone();
        if self.options.persist {
            s.transcript = Some(TranscriptWriter::open(&path)?);
        }
        let dangling: Vec<String> = loaded
            .dangling_tool_uses()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if !dangling.is_empty() {
            s.push(interrupted_results(&dangling), UserSource::ToolResults);
        }
        if s.messages.last().is_some_and(|m| m.role == Role::User) {
            s.push_interruption();
        }
        s.rec(Record::SessionResumed {});
        replay(&cwd, id, &loaded.records, client.as_ref());
        Ok(self.register(s))
    }

    pub fn list_sessions(&self, cwd: &Path) -> Result<Vec<SessionSummary>> {
        Ok(transcript::list_sessions(cwd))
    }

    pub fn session_modes(&self, session_id: &str) -> Option<SessionInfo> {
        let s = self.session(session_id).ok()?;
        let info = s.try_lock().ok().map(|s| s.info());
        info
    }

    /// Request cancellation of the running prompt; does not take the session lock.
    pub fn cancel(&self, session_id: &str) {
        if let Some(t) = self.cancels.lock().unwrap().get(session_id) {
            t.cancel();
        }
    }

    /// User-initiated mode switch. If a prompt is running the switch does not wait for it:
    /// `ModeChanged` is emitted now and the change is applied (and recorded) before the
    /// prompt's next model request.
    pub async fn set_mode(
        &self,
        session_id: &str,
        mode_id: &str,
        client: Arc<dyn Client>,
    ) -> Result<()> {
        let session = self.session(session_id)?;
        let shared = self.shared_for(session_id)?;
        if !shared.mode_ids.iter().any(|m| m == mode_id) {
            bail!("unknown mode `{mode_id}`");
        }
        if let Ok(mut s) = session.try_lock() {
            *shared.pending_mode.lock().unwrap() = None;
            self.switch_mode(
                &mut s,
                mode_id,
                "user request",
                Initiator::User,
                &client,
                true,
            )
            .await;
            return Ok(());
        }
        *shared.pending_mode.lock().unwrap() = Some(mode_id.to_string());
        client.send_event(session_id, SessionEvent::ModeChanged(mode_id.to_string()));
        Ok(())
    }

    fn shared_for(&self, id: &str) -> Result<Arc<Shared>> {
        self.shared
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown session `{id}`"))
    }

    /// Apply a user mode switch requested during a prompt.
    async fn apply_pending_mode(&self, s: &mut SessionState, client: &Arc<dyn Client>) {
        let Ok(shared) = self.shared_for(&s.id) else {
            return;
        };
        let pending = shared.pending_mode.lock().unwrap().take();
        if let Some(to) = pending.filter(|m| *m != s.mode) {
            self.switch_mode(s, &to, "user request", Initiator::User, client, false)
                .await;
        }
    }

    async fn switch_mode(
        &self,
        s: &mut SessionState,
        to: &str,
        reason: &str,
        by: Initiator,
        client: &Arc<dyn Client>,
        emit: bool,
    ) {
        let old_model = self.model_for(s).ok().map(|(m, _)| m);
        let from = std::mem::replace(&mut s.mode, to.to_string());
        s.mise_tasks = ruckcode_mise::discover(&s.cwd).await;
        if emit {
            client.send_event(&s.id, SessionEvent::ModeChanged(to.to_string()));
        }
        s.rec(Record::ModeChange {
            from: from.clone(),
            to: to.to_string(),
            reason: reason.to_string(),
            initiated_by: by,
        });
        if let (Some(old), Ok((new, _))) = (old_model, self.model_for(s)) {
            if old != new {
                client.send_event(
                    &s.id,
                    SessionEvent::Notice(format!("model changed: {old} -> {new}")),
                );
            }
        }
        let payload = json!({"event": "mode_change", "session_id": s.id, "from": from, "to": to});
        let out = self
            .run_hooks(s, HookEvent::ModeChange, None, &payload)
            .await;
        notify_blocked(s, client, &out);
    }

    pub async fn prompt(
        &self,
        session_id: &str,
        text: String,
        client: Arc<dyn Client>,
    ) -> Result<StopReason> {
        let session = self.session(session_id)?;
        let cancel = CancellationToken::new();
        self.cancels
            .lock()
            .unwrap()
            .insert(session_id.to_string(), cancel.clone());
        let mut s = session.lock().await;
        let mut stats = TurnStats {
            started: Instant::now(),
            model_requests: 0,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
        };
        self.apply_pending_mode(&mut s, &client).await;
        let result = self
            .run_prompt(&mut s, text, &client, &cancel, &mut stats)
            .await;
        if matches!(result, Ok(StopReason::Cancelled))
            && s.messages.last().is_some_and(|m| m.role == Role::User)
        {
            s.push_interruption();
        }
        self.apply_pending_mode(&mut s, &client).await;
        {
            let stop = match &result {
                Ok(reason) => reason.as_str().to_string(),
                Err(_) => "error".to_string(),
            };
            s.rec(Record::TurnEnd {
                stop_reason: stop,
                duration_ms: stats.started.elapsed().as_millis() as u64,
                model_requests: stats.model_requests,
                tool_calls: stats.tool_calls,
                input_tokens: stats.input_tokens,
                output_tokens: stats.output_tokens,
            });
        }
        self.cancels.lock().unwrap().remove(session_id);
        result
    }

    async fn run_hooks(
        &self,
        s: &mut SessionState,
        event: HookEvent,
        tool: Option<&str>,
        payload: &Value,
    ) -> Vec<HookOutcome> {
        let hooks = s.hooks();
        if hooks.get(event).is_empty() {
            return vec![];
        }
        let out = HookRunner::new(&s.cwd)
            .run(&hooks, event, tool, payload)
            .await;
        s.record_hooks(event, &out);
        out
    }

    /// Connect to the session's MCP servers once (first prompt), reporting failures.
    async fn start_mcp(&self, s: &mut SessionState, client: &Arc<dyn Client>) {
        if s.mcp.is_some() {
            return;
        }
        let mut configs = s.config.mcp_servers.clone();
        configs.extend(s.extra_mcp.clone());
        if configs.is_empty() {
            return;
        }
        let manager = McpManager::start(configs, &s.cwd).await;
        let mut records = vec![];
        for (name, result) in manager.statuses() {
            match result {
                Ok(tools) => records.push(McpServerRecord {
                    name,
                    ok: true,
                    tools,
                    error: None,
                }),
                Err(e) => {
                    client.send_event(
                        &s.id,
                        SessionEvent::Notice(format!("MCP server `{name}` failed to start: {e}")),
                    );
                    records.push(McpServerRecord {
                        name,
                        ok: false,
                        tools: 0,
                        error: Some(e),
                    });
                }
            }
        }
        s.rec(Record::McpServers { servers: records });
        s.mcp = Some(manager);
    }

    async fn run_prompt(
        &self,
        s: &mut SessionState,
        text: String,
        client: &Arc<dyn Client>,
        cancel: &CancellationToken,
        stats: &mut TurnStats,
    ) -> Result<StopReason> {
        if !s.started {
            s.started = true;
            let payload = json!({"event": "session_start", "session_id": s.id});
            let out = self
                .run_hooks(s, HookEvent::SessionStart, None, &payload)
                .await;
            notify_blocked(s, client, &out);
        }
        let payload = json!({"event": "user_prompt_submit", "session_id": s.id, "prompt": text});
        let out = self
            .run_hooks(s, HookEvent::UserPromptSubmit, None, &payload)
            .await;
        if let Some(o) = out
            .iter()
            .find(|o| !o.success && o.on_failure() == OnFailure::Block)
        {
            client.send_event(
                &s.id,
                SessionEvent::Notice(format!(
                    "prompt blocked by hook `{}`:\n{}",
                    o.spec.run, o.output
                )),
            );
            return Ok(StopReason::EndTurn);
        }
        self.start_mcp(s, client).await;
        let system = system_prompt(&s.cwd, &s.modes, &s.mode).await;
        self.compact_if_needed(s, client, cancel, &system).await;
        s.push(Message::user_text(text), UserSource::User);

        let mut fix_turns = 0u32;
        for iteration in 0..s.config.max_turn_requests {
            self.apply_pending_mode(s, client).await;
            let system = system_prompt(&s.cwd, &s.modes, &s.mode).await;
            if iteration > 0 {
                self.compact_if_needed(s, client, cancel, &system).await;
            }
            if cancel.is_cancelled() {
                return Ok(StopReason::Cancelled);
            }
            let tools = build_tools(s);
            let (model, profile) = self.model_for(s)?;
            let provider = self.provider_for(s, &profile)?;
            let request = Request {
                model: model.clone(),
                system,
                messages: normalize_messages(&s.messages),
                tools: tools.iter().map(|t| t.spec()).collect(),
                max_tokens: profile.max_tokens,
                thinking: profile.thinking.clone(),
            };
            let Some((response, metrics)) =
                stream_request(&s.id, client.as_ref(), provider.as_ref(), request, cancel).await?
            else {
                return Ok(StopReason::Cancelled);
            };
            stats.model_requests += 1;
            stats.input_tokens += response.usage.input_tokens;
            stats.output_tokens += response.usage.output_tokens;
            s.request_counter += 1;
            let request_id = s.request_counter;
            s.rec(Record::AssistantMessage {
                message: response.message.clone(),
                request_id,
                model,
                stop_reason: response.stop_reason,
                usage: response.usage,
                metrics,
            });
            s.messages.push(response.message.clone());

            let uses: Vec<(String, String, Value)> = response
                .message
                .tool_uses()
                .map(|(i, n, v)| (i.to_string(), n.to_string(), v.clone()))
                .collect();
            if uses.is_empty() {
                if let Some(fix) = self.finish_edits(s, client, &mut fix_turns).await {
                    s.push(Message::user_text(fix), UserSource::FixTurn);
                    continue;
                }
                let payload = json!({"event": "stop", "session_id": s.id});
                let out = self.run_hooks(s, HookEvent::Stop, None, &payload).await;
                notify_blocked(s, client, &out);
                return Ok(if response.stop_reason == LlmStop::MaxTokens {
                    StopReason::MaxTokens
                } else {
                    StopReason::EndTurn
                });
            }

            let tools: HashMap<String, Arc<dyn Tool>> = tools
                .into_iter()
                .map(|t| (t.name().to_string(), t))
                .collect();
            let mut results = Vec::new();
            let mut cancelled = false;
            for (id, name, input) in &uses {
                if cancelled {
                    results.push(self.skip_tool_use(s, id, name, input));
                    continue;
                }
                stats.tool_calls += 1;
                let (block, c) = self
                    .execute_tool_use(s, client, cancel, &tools, id, name, input)
                    .await;
                cancelled = c;
                results.push(block);
            }
            s.push(
                Message {
                    role: Role::User,
                    content: results,
                },
                UserSource::ToolResults,
            );
            if cancelled {
                return Ok(StopReason::Cancelled);
            }
        }
        // Never leave this turn's edits unformatted; a failing hook cannot start a fix turn here.
        let mut no_more_fix_turns = u32::MAX;
        let _ = self.finish_edits(s, client, &mut no_more_fix_turns).await;
        Ok(StopReason::MaxTurnRequests)
    }

    /// Run post-edit hooks over files edited this turn; returns a fix-turn prompt if one is due.
    async fn finish_edits(
        &self,
        s: &mut SessionState,
        client: &Arc<dyn Client>,
        fix_turns: &mut u32,
    ) -> Option<String> {
        if s.edited.is_empty() {
            return None;
        }
        let files: Vec<PathBuf> = std::mem::take(&mut s.edited).into_iter().collect();
        let hooks = s.hooks();
        if hooks.post_edit.is_empty() {
            return None;
        }
        let out = HookRunner::new(&s.cwd)
            .run_post_edit(&hooks, &s.id, &files)
            .await;
        s.record_hooks(HookEvent::PostEdit, &out);
        notify_blocked(s, client, &out);
        let prompt = fix_turn_prompt(&out)?;
        let failing = out
            .iter()
            .filter(|o| !o.success && o.on_failure() == OnFailure::FixTurn);
        let max = failing.clone().map(|o| o.spec.max_fix_turns).min()?;
        let cmd = failing
            .map(|o| o.spec.run.clone())
            .collect::<Vec<_>>()
            .join("`, `");
        if *fix_turns >= max {
            client.send_event(
                &s.id,
                SessionEvent::Notice(format!(
                    "post-edit hook `{cmd}` still failing after {max} fix turn(s)"
                )),
            );
            return None;
        }
        *fix_turns += 1;
        client.send_event(
            &s.id,
            SessionEvent::Notice(format!(
                "post-edit hook `{cmd}` failed; starting fix turn {}/{max}",
                *fix_turns
            )),
        );
        Some(prompt)
    }

    /// Error result for a tool use that was never started (cancellation).
    fn skip_tool_use(
        &self,
        s: &mut SessionState,
        id: &str,
        name: &str,
        input: &Value,
    ) -> ContentBlock {
        let msg = "cancelled before execution";
        s.rec(Record::ToolCall {
            tool_use_id: id.into(),
            name: name.into(),
            input: input.clone(),
            title: name.into(),
            kind: ToolKind::Other.as_str().into(),
        });
        s.rec(Record::ToolResult {
            tool_use_id: id.into(),
            name: name.into(),
            output: msg.into(),
            is_error: true,
            permission: PermissionOutcome::Rejected,
            duration_ms: 0,
            edits: vec![],
        });
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: msg.into(),
            is_error: true,
        }
    }

    /// Returns the tool result block and whether the turn was cancelled.
    #[allow(clippy::too_many_arguments)]
    async fn execute_tool_use(
        &self,
        s: &mut SessionState,
        client: &Arc<dyn Client>,
        cancel: &CancellationToken,
        tools: &HashMap<String, Arc<dyn Tool>>,
        id: &str,
        name: &str,
        input: &Value,
    ) -> (ContentBlock, bool) {
        let tool = tools.get(name).cloned();
        let (title, kind, locations) = match &tool {
            Some(t) => (t.title(input), t.kind(), t.locations(input)),
            None => (name.to_string(), ToolKind::Other, vec![]),
        };
        client.send_event(
            &s.id,
            SessionEvent::ToolCall {
                id: id.into(),
                title: title.clone(),
                kind,
                locations,
                input: input.clone(),
            },
        );
        s.rec(Record::ToolCall {
            tool_use_id: id.into(),
            name: name.into(),
            input: input.clone(),
            title: title.clone(),
            kind: kind.as_str().into(),
        });
        let started = Instant::now();
        let mut permission = PermissionOutcome::Allow;
        let mut cancelled = false;
        let mut edits: Vec<FileEdit> = vec![];
        let mut ran = false;
        let mut switch_to: Option<(String, String)>;

        let (mut content, is_error) = 'run: {
            let Some(tool) = tool else {
                break 'run (format!("unknown tool `{name}`"), true);
            };
            if cancel.is_cancelled() {
                cancelled = true;
                permission = PermissionOutcome::Rejected;
                break 'run ("cancelled before execution".into(), true);
            }
            let gate = if name == SWITCH_MODE {
                self.switch_gate(s, client, id, &title, input).await
            } else {
                self.tool_gate(s, client, id, name, &title, kind, input)
                    .await
            };
            match gate {
                Gate::Run { outcome, target } => {
                    permission = outcome;
                    switch_to = target;
                }
                Gate::Skip {
                    message,
                    outcome,
                    cancelled: c,
                } => {
                    permission = outcome;
                    cancelled = c;
                    break 'run (message, true);
                }
            }
            let payload = json!({
                "event": "pre_tool_use", "session_id": s.id,
                "tool_name": name, "tool_input": input,
            });
            let pre = self
                .run_hooks(s, HookEvent::PreToolUse, Some(name), &payload)
                .await;
            if let Some(o) = pre
                .iter()
                .find(|o| !o.success && o.on_failure() == OnFailure::Block)
            {
                permission = PermissionOutcome::Denied;
                break 'run (
                    format!(
                        "blocked by pre_tool_use hook `{}`:\n{}",
                        o.spec.run, o.output
                    ),
                    true,
                );
            }
            client.send_event(
                &s.id,
                SessionEvent::ToolCallUpdate {
                    id: id.into(),
                    status: ToolStatus::InProgress,
                    output: None,
                    edits: vec![],
                },
            );
            ran = true;
            if let Some((to, reason)) = switch_to.take() {
                self.switch_mode(s, &to, &reason, Initiator::Agent, client, true)
                    .await;
                let name = s.modes.display_name(&to);
                break 'run (
                    format!(
                        "Switched to {name} mode. The tools for this mode are available starting \
                         with your next request."
                    ),
                    false,
                );
            }
            let ctx = ToolContext {
                cwd: s.cwd.clone(),
                session_id: s.id.clone(),
                user: client.clone(),
                cancel: cancel.clone(),
            };
            match tool.call(input.clone(), &ctx).await {
                Ok(out) => {
                    if let Some(plan) = out.plan {
                        client.send_event(&s.id, SessionEvent::Plan(plan));
                    }
                    if !out.is_error {
                        s.edited.extend(out.edits.iter().map(|e| e.path.clone()));
                    }
                    edits = out.edits;
                    (out.content, out.is_error)
                }
                Err(e) => (format!("{e:#}"), true),
            }
        };

        client.send_event(
            &s.id,
            SessionEvent::ToolCallUpdate {
                id: id.into(),
                status: if is_error {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Completed
                },
                output: Some(content.clone()),
                edits: edits.clone(),
            },
        );
        let (record_output, duration) = (content.clone(), started.elapsed());
        if ran {
            let payload = json!({
                "event": "post_tool_use", "session_id": s.id, "tool_name": name,
                "tool_input": input, "tool_response": content, "is_error": is_error,
            });
            let post = self
                .run_hooks(s, HookEvent::PostToolUse, Some(name), &payload)
                .await;
            for o in post
                .iter()
                .filter(|o| !o.success && o.on_failure() == OnFailure::FixTurn)
            {
                content.push_str(&format!(
                    "\n\n[post_tool_use hook `{}` failed]\n{}",
                    o.spec.run, o.output
                ));
            }
        }
        let cwd = s.cwd.clone();
        s.rec(Record::ToolResult {
            tool_use_id: id.into(),
            name: name.into(),
            output: record_output,
            is_error,
            permission,
            duration_ms: duration.as_millis() as u64,
            edits: edits.iter().map(|e| edit_record(&cwd, e)).collect(),
        });
        (
            ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content,
                is_error,
            },
            cancelled,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn tool_gate(
        &self,
        s: &mut SessionState,
        client: &Arc<dyn Client>,
        id: &str,
        name: &str,
        title: &str,
        kind: ToolKind,
        input: &Value,
    ) -> Gate {
        if s.allow_always.contains(name) {
            return Gate::Run {
                outcome: PermissionOutcome::AllowAlways,
                target: None,
            };
        }
        match permissions::resolve(name, kind, s.mode_def(), &s.config) {
            Permission::Allow => Gate::Run {
                outcome: PermissionOutcome::Allow,
                target: None,
            },
            Permission::Deny => Gate::Skip {
                message: "denied by policy".into(),
                outcome: PermissionOutcome::Denied,
                cancelled: false,
            },
            Permission::Ask => {
                let req = PermissionRequest {
                    tool_call_id: id.into(),
                    title: title.into(),
                    kind,
                    input: input.clone(),
                    allow_always: true,
                };
                match client.request_permission(&s.id, req).await {
                    PermissionDecision::AllowOnce => Gate::Run {
                        outcome: PermissionOutcome::AskApproved,
                        target: None,
                    },
                    PermissionDecision::AllowAlways => {
                        s.allow_always.insert(name.into());
                        Gate::Run {
                            outcome: PermissionOutcome::AllowAlways,
                            target: None,
                        }
                    }
                    PermissionDecision::Reject => Gate::Skip {
                        message: "user rejected".into(),
                        outcome: PermissionOutcome::Rejected,
                        cancelled: false,
                    },
                    PermissionDecision::Cancelled => Gate::Skip {
                        message: "cancelled by user".into(),
                        outcome: PermissionOutcome::Rejected,
                        cancelled: true,
                    },
                }
            }
        }
    }

    async fn switch_gate(
        &self,
        s: &mut SessionState,
        client: &Arc<dyn Client>,
        id: &str,
        title: &str,
        input: &Value,
    ) -> Gate {
        let target = input["mode_id"].as_str().unwrap_or_default().to_string();
        let reason = input["reason"].as_str().unwrap_or_default().to_string();
        if !s.modes.switch_targets(&s.mode).contains(&target.as_str()) {
            return Gate::Skip {
                message: format!("cannot switch to mode `{target}` from `{}`", s.mode),
                outcome: PermissionOutcome::Denied,
                cancelled: false,
            };
        }
        let run = |outcome| Gate::Run {
            outcome,
            target: Some((target.clone(), reason.clone())),
        };
        if s.mode_def().auto_advance_to.contains(&target) {
            return run(PermissionOutcome::Allow);
        }
        let req = PermissionRequest {
            tool_call_id: id.into(),
            title: title.into(),
            kind: ToolKind::SwitchMode,
            input: input.clone(),
            allow_always: false,
        };
        match client.request_permission(&s.id, req).await {
            PermissionDecision::AllowOnce | PermissionDecision::AllowAlways => {
                run(PermissionOutcome::AskApproved)
            }
            PermissionDecision::Reject => Gate::Skip {
                message: "user declined the mode switch; stay in the current mode".into(),
                outcome: PermissionOutcome::Rejected,
                cancelled: false,
            },
            PermissionDecision::Cancelled => Gate::Skip {
                message: "cancelled by user".into(),
                outcome: PermissionOutcome::Rejected,
                cancelled: true,
            },
        }
    }

    /// Summarize and replace history when the estimated context exceeds the threshold.
    /// Must only be called when the history is valid (no dangling tool uses).
    async fn compact_if_needed(
        &self,
        s: &mut SessionState,
        client: &Arc<dyn Client>,
        cancel: &CancellationToken,
        system: &str,
    ) {
        let Ok((model, profile)) = self.model_for(s) else {
            return;
        };
        let chars = serde_json::to_string(&s.messages).map_or(0, |j| j.len()) + system.len();
        let estimate = (chars / 4) as f64;
        if s.messages.len() < 4
            || estimate <= s.config.compaction_threshold * profile.context_window as f64
        {
            return;
        }
        let Ok(provider) = self.provider_for(s, &profile) else {
            return;
        };
        let mut messages = normalize_messages(&s.messages);
        match messages.last_mut() {
            Some(m) if m.role == Role::User => m.content.push(ContentBlock::Text {
                text: SUMMARY_PROMPT.into(),
            }),
            _ => messages.push(Message::user_text(SUMMARY_PROMPT)),
        }
        let request = Request {
            model,
            system: system.to_string(),
            messages,
            tools: vec![],
            max_tokens: profile.max_tokens,
            thinking: None,
        };
        let (tx, _rx) = unbounded_channel();
        let response = tokio::select! {
            _ = cancel.cancelled() => return,
            r = provider.complete(request, tx) => r,
        };
        let summary = match response {
            Ok(r) if !r.message.text().trim().is_empty() => r.message.text(),
            Ok(_) => return,
            Err(e) => {
                client.send_event(
                    &s.id,
                    SessionEvent::Notice(format!("context compaction failed: {e:#}")),
                );
                return;
            }
        };
        let replaced = s.messages.len();
        s.messages = vec![compaction_message(&summary)];
        s.rec(Record::Compaction {
            summary,
            replaced_messages: replaced,
        });
        client.send_event(
            &s.id,
            SessionEvent::Notice(format!(
                "conversation compacted ({replaced} messages summarized)"
            )),
        );
    }
}

enum Gate {
    Run {
        outcome: PermissionOutcome,
        /// For SwitchMode: (target mode, reason).
        target: Option<(String, String)>,
    },
    Skip {
        message: String,
        outcome: PermissionOutcome,
        cancelled: bool,
    },
}

/// Merge adjacent same-role messages so providers always see strictly alternating roles.
/// Tool results stay ahead of any text merged into the same user message.
fn normalize_messages(messages: &[Message]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    for m in messages {
        match out.last_mut() {
            Some(last) if last.role == m.role => {
                last.content.extend(m.content.iter().cloned());
                if last.role == Role::User {
                    last.content
                        .sort_by_key(|b| !matches!(b, ContentBlock::ToolResult { .. }));
                }
            }
            _ => out.push(m.clone()),
        }
    }
    out
}

fn notify_blocked(s: &SessionState, client: &Arc<dyn Client>, outcomes: &[HookOutcome]) {
    for o in outcomes
        .iter()
        .filter(|o| !o.success && o.on_failure() == OnFailure::Block)
    {
        client.send_event(
            &s.id,
            SessionEvent::Notice(format!("hook `{}` failed:\n{}", o.spec.run, o.output)),
        );
    }
}

/// Tools for the session's current mode.
fn build_tools(s: &SessionState) -> Vec<Arc<dyn Tool>> {
    let mode = s.mode_def();
    let mut tools: Vec<Arc<dyn Tool>> = ruckcode_tools::builtin_tools()
        .into_iter()
        .filter(|t| {
            mode.tools
                .as_ref()
                .is_none_or(|l| l.iter().any(|n| n == t.name()))
        })
        .collect();
    let targets: Vec<(String, String, String)> = s
        .modes
        .switch_targets(&s.mode)
        .into_iter()
        .map(|id| {
            let desc = s.modes.get(id).map(|m| m.description.clone());
            (
                id.to_string(),
                s.modes.display_name(id),
                desc.unwrap_or_default(),
            )
        })
        .collect();
    if !targets.is_empty() {
        tools.push(Arc::new(SwitchModeTool { targets }));
    }
    if let Some(mcp) = &s.mcp {
        tools.extend(mcp.tools_for(&mode.mcp_tools, s.config.mcp.lazy_threshold));
    }
    match TaskSelector::new(&mode.mise_tasks) {
        Ok(sel) => {
            let selected = sel.select(&s.mise_tasks).into_iter().cloned().collect();
            tools.extend(ruckcode_mise::tools_for(
                selected,
                s.config.mise.lazy_threshold,
            ));
        }
        Err(e) => tracing::warn!("invalid mise_tasks selector in mode `{}`: {e}", s.mode),
    }
    tools
}

/// Stream one completion, forwarding deltas to the client. `None` means cancelled.
async fn stream_request(
    session_id: &str,
    client: &dyn Client,
    provider: &dyn Provider,
    request: Request,
    cancel: &CancellationToken,
) -> Result<Option<(Response, Metrics)>> {
    let started = Instant::now();
    let mut first_token: Option<Duration> = None;
    let (tx, mut rx) = unbounded_channel();
    let fut = provider.complete(request, tx);
    tokio::pin!(fut);
    let handle = |ev: StreamEvent, first: &mut Option<Duration>| match ev {
        StreamEvent::TextDelta(t) => {
            first.get_or_insert_with(|| started.elapsed());
            client.send_event(session_id, SessionEvent::AgentText(t));
        }
        StreamEvent::ThinkingDelta(t) => {
            first.get_or_insert_with(|| started.elapsed());
            client.send_event(session_id, SessionEvent::AgentThought(t));
        }
        StreamEvent::ToolUseStart { .. } => {}
    };
    let mut open = true;
    let response = loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(None),
            ev = rx.recv(), if open => match ev {
                Some(ev) => handle(ev, &mut first_token),
                None => open = false,
            },
            r = &mut fut => break r?,
        }
    };
    while let Ok(ev) = rx.try_recv() {
        handle(ev, &mut first_token);
    }
    let total = started.elapsed();
    let generating = total.saturating_sub(first_token.unwrap_or_default());
    let out = response.usage.output_tokens;
    let metrics = Metrics {
        duration_ms: total.as_millis() as u64,
        time_to_first_token_ms: first_token.map(|d| d.as_millis() as u64),
        output_tokens_per_sec: (out > 0 && !generating.is_zero())
            .then(|| out as f64 / generating.as_secs_f64()),
    };
    Ok(Some((response, metrics)))
}

fn edit_record(cwd: &Path, e: &FileEdit) -> EditRecord {
    let rel = e
        .path
        .strip_prefix(cwd)
        .unwrap_or(&e.path)
        .display()
        .to_string();
    let from = e
        .old_text
        .as_ref()
        .map_or("/dev/null".to_string(), |_| format!("a/{rel}"));
    let diff = similar::TextDiff::from_lines(e.old_text.as_deref().unwrap_or(""), &e.new_text)
        .unified_diff()
        .context_radius(3)
        .header(&from, &format!("b/{rel}"))
        .to_string();
    EditRecord { path: rel, diff }
}

/// Rebuild an approximate `FileEdit` (hunk text only) from a stored unified diff.
fn edit_from_diff(cwd: &Path, e: &EditRecord) -> FileEdit {
    let (mut old, mut new) = (String::new(), String::new());
    let is_new = e
        .diff
        .lines()
        .next()
        .is_some_and(|l| l.contains("/dev/null"));
    for line in e.diff.lines().skip(2) {
        match line.chars().next() {
            Some(' ') => {
                old.push_str(&line[1..]);
                old.push('\n');
                new.push_str(&line[1..]);
                new.push('\n');
            }
            Some('-') => {
                old.push_str(&line[1..]);
                old.push('\n');
            }
            Some('+') => {
                new.push_str(&line[1..]);
                new.push('\n');
            }
            _ => {}
        }
    }
    FileEdit {
        path: cwd.join(&e.path),
        old_text: (!is_new).then_some(old),
        new_text: new,
    }
}

fn kind_from_str(s: &str) -> ToolKind {
    match s {
        "read" => ToolKind::Read,
        "edit" => ToolKind::Edit,
        "delete" => ToolKind::Delete,
        "move" => ToolKind::Move,
        "search" => ToolKind::Search,
        "execute" => ToolKind::Execute,
        "think" => ToolKind::Think,
        "fetch" => ToolKind::Fetch,
        "switch_mode" => ToolKind::SwitchMode,
        _ => ToolKind::Other,
    }
}

/// Send a loaded transcript's history to the client as session events.
fn replay(cwd: &Path, session_id: &str, records: &[transcript::Stamped], client: &dyn Client) {
    for r in records {
        match &r.record {
            Record::UserMessage {
                message,
                source: UserSource::User,
            } => client.send_event(session_id, SessionEvent::UserText(message.text())),
            Record::AssistantMessage { message, .. } => {
                for b in &message.content {
                    match b {
                        ContentBlock::Thinking { thinking, .. } => client
                            .send_event(session_id, SessionEvent::AgentThought(thinking.clone())),
                        ContentBlock::Text { text } => {
                            client.send_event(session_id, SessionEvent::AgentText(text.clone()))
                        }
                        _ => {}
                    }
                }
            }
            Record::ToolCall {
                tool_use_id,
                name,
                input,
                title,
                kind,
            } => {
                let locations = ["file_path", "path"]
                    .iter()
                    .filter_map(|k| input.get(*k).and_then(Value::as_str).map(String::from))
                    .take(1)
                    .collect();
                client.send_event(
                    session_id,
                    SessionEvent::ToolCall {
                        id: tool_use_id.clone(),
                        title: title.clone(),
                        kind: kind_from_str(kind),
                        locations,
                        input: input.clone(),
                    },
                );
                if name == "TodoWrite" {
                    if let Ok(items) =
                        serde_json::from_value::<Vec<TodoItem>>(input["todos"].clone())
                    {
                        client.send_event(session_id, SessionEvent::Plan(items));
                    }
                }
            }
            Record::ToolResult {
                tool_use_id,
                output,
                is_error,
                edits,
                ..
            } => client.send_event(
                session_id,
                SessionEvent::ToolCallUpdate {
                    id: tool_use_id.clone(),
                    status: if *is_error {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Completed
                    },
                    output: Some(output.clone()),
                    edits: edits.iter().map(|e| edit_from_diff(cwd, e)).collect(),
                },
            ),
            Record::ModeChange { to, .. } => {
                client.send_event(session_id, SessionEvent::ModeChanged(to.clone()))
            }
            _ => {}
        }
    }
}
