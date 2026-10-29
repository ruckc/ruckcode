//! Append-only JSONL session transcripts stored in the project for audit and resume.

use anyhow::{Context, Result};
use ruckcode_llm::{ContentBlock, Message, Role, StopReason, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub cwd: String,
    pub created_at: String,
    pub ruckcode_version: String,
    pub profile: String,
    pub model: String,
    pub mode: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserSource {
    User,
    FixTurn,
    Hook,
    ToolResults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOutcome {
    Allow,
    AllowAlways,
    AskApproved,
    Denied,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Initiator {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub duration_ms: u64,
    pub time_to_first_token_ms: Option<u64>,
    pub output_tokens_per_sec: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditRecord {
    pub path: String,
    /// Unified diff.
    pub diff: String,
}

/// Outcome of connecting to one MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerRecord {
    pub name: String,
    pub ok: bool,
    pub tools: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Record {
    SessionStart(SessionMeta),
    UserMessage {
        message: Message,
        source: UserSource,
    },
    AssistantMessage {
        message: Message,
        request_id: u64,
        model: String,
        stop_reason: StopReason,
        usage: Usage,
        metrics: Metrics,
    },
    ToolCall {
        tool_use_id: String,
        name: String,
        input: Value,
        title: String,
        kind: String,
    },
    ToolResult {
        tool_use_id: String,
        name: String,
        output: String,
        is_error: bool,
        permission: PermissionOutcome,
        duration_ms: u64,
        edits: Vec<EditRecord>,
    },
    HookRun {
        event: String,
        command: String,
        success: bool,
        exit_code: Option<i32>,
        output: String,
        duration_ms: u64,
    },
    ModeChange {
        from: String,
        to: String,
        reason: String,
        initiated_by: Initiator,
    },
    /// The user pinned (or cleared) the session model with `/model`.
    ModelChange {
        from: Option<String>,
        to: Option<String>,
    },
    Compaction {
        summary: String,
        replaced_messages: usize,
    },
    /// `/clear`: the history before this record is not sent to the model again.
    HistoryCleared {
        cleared_messages: usize,
    },
    TurnEnd {
        stop_reason: String,
        duration_ms: u64,
        model_requests: u32,
        tool_calls: u32,
        input_tokens: u64,
        output_tokens: u64,
    },
    SessionResumed {},
    /// The user chose not to resume this session; it stops being offered as unfinished until new
    /// work is recorded.
    SessionDismissed {},
    McpServers {
        servers: Vec<McpServerRecord>,
    },
}

/// A record with its timestamp, as stored on one line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stamped {
    pub ts: String,
    #[serde(flatten)]
    pub record: Record,
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `YYYYMMDD-HHMMSS-<6 hex>` in UTC, sortable by creation time.
pub fn new_session_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"),
        &hex[..6]
    )
}

pub fn sessions_dir(project: &Path) -> PathBuf {
    project.join(".ruckcode/sessions")
}

pub fn session_path(project: &Path, id: &str) -> PathBuf {
    sessions_dir(project).join(format!("{id}.jsonl"))
}

/// The user message that replaces history after compaction.
pub fn compaction_message(summary: &str) -> Message {
    Message::user_text(format!(
        "This session is being continued from a previous conversation. Summary:\n{summary}"
    ))
}

pub struct TranscriptWriter {
    file: File,
    path: PathBuf,
}

impl TranscriptWriter {
    /// Create `<project>/.ruckcode/sessions/<id>.jsonl` and write `session_start`.
    pub fn create(project: &Path, meta: SessionMeta) -> Result<Self> {
        let dir = sessions_dir(project);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating the session folder {}", dir.display()))?;
        let path = session_path(project, &meta.id);
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        let mut w = Self { file, path };
        w.append(&Record::SessionStart(meta))?;
        Ok(w)
    }

    /// Reopen an existing transcript for appending.
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        // A crash may have left a partial last line; start on a fresh one.
        let text = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if text.last().is_some_and(|b| *b != b'\n') {
            file.write_all(b"\n")?;
        }
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one record and flush.
    pub fn append(&mut self, record: &Record) -> Result<()> {
        let stamped = Stamped {
            ts: now_rfc3339(),
            record: record.clone(),
        };
        let mut line = serde_json::to_vec(&stamped)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LoadedSession {
    pub meta: SessionMeta,
    /// Conversation history rebuilt from user/assistant/compaction records.
    pub messages: Vec<Message>,
    /// Mode at the end of the transcript.
    pub mode: String,
    /// Model pinned for the session by the last `model_change` record, if any.
    pub model: Option<String>,
    pub records: Vec<Stamped>,
}

impl LoadedSession {
    /// Tool uses in the final assistant message that never received results.
    pub fn dangling_tool_uses(&self) -> Vec<(String, String)> {
        match self.messages.last() {
            Some(m) if m.role == Role::Assistant => m
                .tool_uses()
                .map(|(id, name, _)| (id.to_string(), name.to_string()))
                .collect(),
            _ => vec![],
        }
    }
}

pub struct TranscriptReader;

impl TranscriptReader {
    /// Parse a transcript, ignoring unparseable lines (such as a truncated final line).
    pub fn load(path: &Path) -> Result<LoadedSession> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut records = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Stamped>(line) {
                Ok(r) => records.push(r),
                Err(e) => tracing::warn!("{}:{}: skipping bad record: {e}", path.display(), n + 1),
            }
        }
        let meta = match records.first().map(|r| &r.record) {
            Some(Record::SessionStart(m)) => m.clone(),
            _ => anyhow::bail!("{} does not start with session_start", path.display()),
        };
        let mut messages = Vec::new();
        let mut mode = meta.mode.clone();
        let mut model = None;
        for r in &records {
            match &r.record {
                Record::UserMessage { message, .. } | Record::AssistantMessage { message, .. } => {
                    messages.push(message.clone())
                }
                Record::Compaction { summary, .. } => messages = vec![compaction_message(summary)],
                Record::HistoryCleared { .. } => messages.clear(),
                Record::ModeChange { to, .. } => mode = to.clone(),
                Record::ModelChange { to, .. } => model = to.clone(),
                _ => {}
            }
        }
        Ok(LoadedSession {
            meta,
            messages,
            mode,
            model,
            records,
        })
    }
}

/// A synthetic user message answering every dangling tool use with an error.
pub fn interrupted_results(tool_use_ids: &[String]) -> Message {
    Message {
        role: Role::User,
        content: tool_use_ids
            .iter()
            .map(|id| ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: "interrupted before completion".into(),
                is_error: true,
            })
            .collect(),
    }
}

/// What a transcript says was left undone. See [`assess_unfinished`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unfinished {
    /// The last turn never completed (killed, crashed, provider error) or work was recorded
    /// after the last turn ended.
    pub interrupted: bool,
    /// Content of the last `TodoWrite` plan's items that are not completed.
    pub open_tasks: Vec<String>,
    /// The plan item that was in progress, if any.
    pub in_progress: Option<String>,
}

impl Unfinished {
    pub fn is_unfinished(&self) -> bool {
        self.interrupted || !self.open_tasks.is_empty()
    }
}

/// Scan the records (cheaply, front to back) for unfinished work.
///
/// A session is interrupted when conversation records (user/assistant message, tool call or
/// result) follow the last `turn_end`, or the last turn ended in an error. A turn the user
/// cancelled on purpose is not interrupted. Independently, the last `TodoWrite` plan with items
/// that are not `completed` counts as open tasks. `session_dismissed` clears both until newer
/// records appear.
pub fn assess_unfinished(records: &[Stamped]) -> Unfinished {
    let mut work_after_end = false;
    let mut bad_end = false;
    let mut open: Vec<String> = vec![];
    let mut in_progress = None;
    for r in records {
        match &r.record {
            Record::UserMessage { .. }
            | Record::AssistantMessage { .. }
            | Record::ToolCall { .. }
            | Record::ToolResult { .. } => work_after_end = true,
            Record::TurnEnd { stop_reason, .. } => {
                work_after_end = false;
                bad_end = matches!(stop_reason.as_str(), "error" | "interrupted");
            }
            Record::SessionDismissed {} => {
                work_after_end = false;
                bad_end = false;
                open.clear();
                in_progress = None;
            }
            Record::HistoryCleared { .. } => {
                open.clear();
                in_progress = None;
            }
            _ => {}
        }
        if let Record::ToolCall { name, input, .. } = &r.record {
            if name == "TodoWrite" {
                open.clear();
                in_progress = None;
                for t in input["todos"].as_array().into_iter().flatten() {
                    let content = t["content"].as_str().unwrap_or_default().to_string();
                    match t["status"].as_str() {
                        Some("completed") => {}
                        Some("in_progress") => {
                            in_progress = Some(content.clone());
                            open.push(content);
                        }
                        _ => open.push(content),
                    }
                }
            }
        }
    }
    Unfinished {
        interrupted: work_after_end || bad_end,
        open_tasks: open,
        in_progress,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub created_at: String,
    pub first_prompt: String,
    pub mode: String,
    pub updated_at: String,
}

/// Sessions stored in the project, newest first.
pub fn list_sessions(project: &Path) -> Vec<SessionSummary> {
    let Ok(rd) = std::fs::read_dir(sessions_dir(project)) else {
        return vec![];
    };
    let mut out: Vec<SessionSummary> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let s = TranscriptReader::load(&e.path()).ok()?;
            let first_prompt = s
                .records
                .iter()
                .find_map(|r| match &r.record {
                    Record::UserMessage {
                        message,
                        source: UserSource::User,
                    } => Some(message.text()),
                    _ => None,
                })
                .unwrap_or_default();
            Some(SessionSummary {
                id: s.meta.id.clone(),
                created_at: s.meta.created_at.clone(),
                first_prompt,
                mode: s.mode,
                updated_at: s.records.last().map(|r| r.ts.clone()).unwrap_or_default(),
            })
        })
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str) -> SessionMeta {
        SessionMeta {
            id: id.into(),
            cwd: "/x".into(),
            created_at: now_rfc3339(),
            ruckcode_version: "0".into(),
            profile: "p".into(),
            model: "m".into(),
            mode: "default".into(),
        }
    }

    #[test]
    fn id_format() {
        let id = new_session_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 6);
        assert_eq!(parts[2].len(), 6);
    }

    #[test]
    fn roundtrip_and_truncated_tail() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = TranscriptWriter::create(dir.path(), meta("20250101-000000-abcdef")).unwrap();
        w.append(&Record::UserMessage {
            message: Message::user_text("hello"),
            source: UserSource::User,
        })
        .unwrap();
        w.append(&Record::AssistantMessage {
            message: Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "hmm".into(),
                        signature: Some("sig".into()),
                    },
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "Read".into(),
                        input: serde_json::json!({}),
                    },
                ],
            },
            request_id: 1,
            model: "m".into(),
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            metrics: Metrics {
                duration_ms: 1,
                time_to_first_token_ms: None,
                output_tokens_per_sec: None,
            },
        })
        .unwrap();
        let path = w.path().to_path_buf();
        use std::io::Write;
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"ts\":\"x\",\"type\":\"user_mess")
            .unwrap();
        let l = TranscriptReader::load(&path).unwrap();
        assert_eq!(l.messages.len(), 2);
        assert_eq!(l.dangling_tool_uses(), [("t1".into(), "Read".into())]);
        assert_eq!(list_sessions(dir.path())[0].first_prompt, "hello");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw
            .lines()
            .next()
            .unwrap()
            .contains("\"type\":\"session_start\""));
        assert!(raw.contains("\"signature\":\"sig\""));
    }

    fn stamped(record: Record) -> Stamped {
        Stamped {
            ts: now_rfc3339(),
            record,
        }
    }

    fn user(t: &str) -> Stamped {
        stamped(Record::UserMessage {
            message: Message::user_text(t),
            source: UserSource::User,
        })
    }

    fn turn_end(reason: &str) -> Stamped {
        stamped(Record::TurnEnd {
            stop_reason: reason.into(),
            duration_ms: 1,
            model_requests: 1,
            tool_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
        })
    }

    fn todo(items: serde_json::Value) -> Stamped {
        stamped(Record::ToolCall {
            tool_use_id: "t".into(),
            name: "TodoWrite".into(),
            input: serde_json::json!({ "todos": items }),
            title: String::new(),
            kind: "think".into(),
        })
    }

    #[test]
    fn unfinished_detection() {
        let start = stamped(Record::SessionStart(meta("a")));
        let a = |rs: Vec<Stamped>| {
            let mut v = vec![start.clone()];
            v.extend(rs);
            assess_unfinished(&v)
        };
        // Completed turn, cancelled turn: settled.
        assert!(!a(vec![user("x"), turn_end("end_turn")]).is_unfinished());
        assert!(!a(vec![user("x"), turn_end("cancelled")]).is_unfinished());
        // Killed mid-turn (no turn_end), or ended in an error: interrupted.
        assert!(a(vec![user("x")]).interrupted);
        assert!(a(vec![user("x"), turn_end("end_turn"), user("y")]).interrupted);
        assert!(a(vec![user("x"), turn_end("error")]).interrupted);
        // Open plan items count even after a clean turn end; completed plans do not.
        let open = a(vec![
            user("x"),
            todo(serde_json::json!([
                {"content": "a", "status": "completed"},
                {"content": "b", "status": "in_progress"},
                {"content": "c", "status": "pending"}
            ])),
            turn_end("end_turn"),
        ]);
        assert!(!open.interrupted);
        assert_eq!(open.open_tasks, ["b", "c"]);
        assert_eq!(open.in_progress.as_deref(), Some("b"));
        let done = a(vec![
            todo(serde_json::json!([{"content": "a", "status": "completed"}])),
            turn_end("end_turn"),
        ]);
        assert!(!done.is_unfinished());
        // Dismissal settles it until new work appears.
        let d = vec![user("x"), stamped(Record::SessionDismissed {})];
        assert!(!a(d.clone()).is_unfinished());
        let mut again = d;
        again.push(user("more"));
        assert!(a(again).interrupted);
    }

    #[test]
    fn compaction_resets_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = TranscriptWriter::create(dir.path(), meta("a")).unwrap();
        for _ in 0..3 {
            w.append(&Record::UserMessage {
                message: Message::user_text("x"),
                source: UserSource::User,
            })
            .unwrap();
        }
        w.append(&Record::Compaction {
            summary: "S".into(),
            replaced_messages: 3,
        })
        .unwrap();
        let l = TranscriptReader::load(w.path()).unwrap();
        assert_eq!(l.messages, vec![compaction_message("S")]);
    }
}
