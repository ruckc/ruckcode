//! ACP v1 wire types (subset).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const INITIALIZE: &str = "initialize";
pub const AUTHENTICATE: &str = "authenticate";
pub const SESSION_NEW: &str = "session/new";
pub const SESSION_LOAD: &str = "session/load";
pub const SESSION_PROMPT: &str = "session/prompt";
pub const SESSION_CANCEL: &str = "session/cancel";
pub const SESSION_SET_MODE: &str = "session/set_mode";
pub const SESSION_UPDATE: &str = "session/update";
pub const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
pub const FS_READ_TEXT_FILE: &str = "fs/read_text_file";
pub const FS_WRITE_TEXT_FILE: &str = "fs/write_text_file";
pub const ASK_USER_QUESTION: &str = "_ruckcode/ask_user_question";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FsCapabilities {
    pub read_text_file: bool,
    pub write_text_file: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClientCapabilities {
    pub fs: FsCapabilities,
    pub terminal: bool,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

impl ClientCapabilities {
    pub fn supports_ask_user_question(&self) -> bool {
        self.meta
            .as_ref()
            .and_then(|m| m.get("ruckcode"))
            .and_then(|r| r.get("askUserQuestion"))
            .and_then(Value::as_bool)
            == Some(true)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitializeRequest {
    pub protocol_version: u16,
    pub client_capabilities: ClientCapabilities,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptCapabilities {
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentCapabilities {
    pub load_session: bool,
    pub prompt_capabilities: PromptCapabilities,
    pub mcp_capabilities: McpCapabilities,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpCapabilities {
    pub http: bool,
    pub sse: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitializeResponse {
    pub protocol_version: u16,
    pub agent_capabilities: AgentCapabilities,
    pub auth_methods: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_info: Option<Implementation>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewSessionRequest {
    pub cwd: PathBuf,
    pub mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionMode {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionModeState {
    pub current_mode_id: String,
    pub available_modes: Vec<SessionMode>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewSessionResponse {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modes: Option<SessionModeState>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoadSessionRequest {
    pub session_id: String,
    pub cwd: PathBuf,
    pub mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoadSessionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modes: Option<SessionModeState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedResource {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Image {
        data: String,
        mime_type: String,
    },
    ResourceLink {
        uri: String,
        #[serde(default)]
        name: String,
    },
    Resource {
        resource: EmbeddedResource,
    },
    /// Any content type we do not understand (audio, future types).
    #[serde(other)]
    Unknown,
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }
}

pub fn prompt_to_text(blocks: &[ContentBlock]) -> String {
    let parts: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            ContentBlock::ResourceLink { uri, .. } => Some(format!("@{uri}")),
            ContentBlock::Resource { resource } => resource
                .text
                .as_ref()
                .map(|t| format!("<file uri={}>\n{}\n</file>", resource.uri, t)),
            ContentBlock::Image { .. } | ContentBlock::Unknown => None,
        })
        .collect();
    parts.join("\n")
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptRequest {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResponse {
    pub stop_reason: StopReason,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CancelNotification {
    pub session_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SetSessionModeRequest {
    pub session_id: String,
    pub mode_id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    #[default]
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    Content {
        content: ContentBlock,
    },
    #[serde(rename_all = "camelCase")]
    Diff {
        path: PathBuf,
        #[serde(default)]
        old_text: Option<String>,
        new_text: String,
    },
    #[serde(rename_all = "camelCase")]
    Terminal {
        terminal_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallLocation {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolCallStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<ToolCallContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<ToolCallLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCallUpdate {
    pub tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ToolKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolCallStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ToolCallContent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<ToolCallLocation>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub priority: PlanPriority,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum SessionUpdate {
    UserMessageChunk {
        content: ContentBlock,
    },
    AgentMessageChunk {
        content: ContentBlock,
    },
    AgentThoughtChunk {
        content: ContentBlock,
    },
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallUpdate),
    Plan {
        entries: Vec<PlanEntry>,
    },
    #[serde(rename_all = "camelCase")]
    CurrentModeUpdate {
        current_mode_id: String,
    },
    #[serde(rename_all = "camelCase")]
    AvailableCommandsUpdate {
        available_commands: Vec<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    pub session_id: String,
    pub update: SessionUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionRequest {
    pub session_id: String,
    pub tool_call: ToolCallUpdate,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RequestPermissionOutcome {
    Cancelled,
    #[serde(rename_all = "camelCase")]
    Selected {
        option_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestPermissionResponse {
    pub outcome: RequestPermissionOutcome,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReadTextFileRequest {
    pub session_id: String,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadTextFileResponse {
    pub content: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WriteTextFileRequest {
    pub session_id: String,
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AskUserQuestionRequest {
    pub session_id: String,
    pub questions: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AskUserQuestionResponse {
    pub answers: Vec<Value>,
    pub cancelled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_call_notification_shape() {
        let n = SessionNotification {
            session_id: "s1".into(),
            update: SessionUpdate::ToolCall(ToolCall {
                tool_call_id: "c1".into(),
                title: "Read foo".into(),
                kind: ToolKind::Read,
                status: ToolCallStatus::InProgress,
                locations: vec![ToolCallLocation {
                    path: "/a".into(),
                    line: Some(3),
                }],
                ..Default::default()
            }),
        };
        let expected = json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": "c1",
                "title": "Read foo",
                "kind": "read",
                "status": "in_progress",
                "locations": [{"path": "/a", "line": 3}]
            }
        });
        assert_eq!(serde_json::to_value(&n).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<SessionNotification>(expected).unwrap(),
            n
        );
    }

    #[test]
    fn update_variants() {
        let u: SessionUpdate = serde_json::from_value(json!({
            "sessionUpdate":"tool_call_update","toolCallId":"c","status":"completed",
            "content":[{"type":"diff","path":"/f","oldText":null,"newText":"x"}]
        }))
        .unwrap();
        let SessionUpdate::ToolCallUpdate(t) = u else {
            panic!()
        };
        assert_eq!(t.status, Some(ToolCallStatus::Completed));
        assert!(t.title.is_none());
        let m = serde_json::to_value(SessionUpdate::CurrentModeUpdate {
            current_mode_id: "plan".into(),
        })
        .unwrap();
        assert_eq!(
            m,
            json!({"sessionUpdate":"current_mode_update","currentModeId":"plan"})
        );
        let c = serde_json::to_value(SessionUpdate::AgentMessageChunk {
            content: ContentBlock::text("hi"),
        })
        .unwrap();
        assert_eq!(
            c,
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}})
        );
    }

    #[test]
    fn permission_shapes() {
        let r: RequestPermissionResponse =
            serde_json::from_value(json!({"outcome":{"outcome":"selected","optionId":"allow"}}))
                .unwrap();
        assert_eq!(
            r.outcome,
            RequestPermissionOutcome::Selected {
                option_id: "allow".into()
            }
        );
        let c = serde_json::to_value(RequestPermissionResponse {
            outcome: RequestPermissionOutcome::Cancelled,
        })
        .unwrap();
        assert_eq!(c, json!({"outcome":{"outcome":"cancelled"}}));
        let req = RequestPermissionRequest {
            session_id: "s".into(),
            tool_call: ToolCallUpdate {
                tool_call_id: "c".into(),
                ..Default::default()
            },
            options: vec![PermissionOption {
                option_id: "o".into(),
                name: "Allow".into(),
                kind: PermissionOptionKind::AllowOnce,
            }],
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["options"][0]["kind"], "allow_once");
        assert_eq!(v["toolCall"], json!({"toolCallId":"c"}));
    }

    #[test]
    fn initialize_and_prompt() {
        let i: InitializeRequest = serde_json::from_value(json!({
            "protocolVersion":1,
            "clientCapabilities":{"fs":{"readTextFile":true},"_meta":{"ruckcode":{"askUserQuestion":true}}}
        }))
        .unwrap();
        assert!(i.client_capabilities.fs.read_text_file);
        assert!(!i.client_capabilities.fs.write_text_file);
        assert!(i.client_capabilities.supports_ask_user_question());
        assert!(!ClientCapabilities::default().supports_ask_user_question());

        let p: PromptRequest = serde_json::from_value(json!({
            "sessionId":"s",
            "prompt":[
                {"type":"text","text":"a"},
                {"type":"resource_link","uri":"file:///x","name":"x"},
                {"type":"resource","resource":{"uri":"file:///y","text":"body","mimeType":"text/plain"}},
                {"type":"audio","data":"..","mimeType":"audio/wav"}
            ]
        }))
        .unwrap();
        assert_eq!(p.prompt[3], ContentBlock::Unknown);
        assert_eq!(
            prompt_to_text(&p.prompt),
            "a\n@file:///x\n<file uri=file:///y>\nbody\n</file>"
        );
        let r = serde_json::to_value(PromptResponse {
            stop_reason: StopReason::MaxTurnRequests,
        })
        .unwrap();
        assert_eq!(r, json!({"stopReason":"max_turn_requests"}));
    }
}
