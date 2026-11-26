//! ACP wire types.
//!
//! The protocol types are the official [`agent_client_protocol_schema`] crate (v1, stable
//! surface plus `unstable_end_turn_token_usage` for `PromptResponse.usage`), re-exported here so that `ruck_harness_acp::schema::*`
//! is the one import for everything on the wire. On top of that this module holds:
//!
//! - method-name constants usable in `match` arms (the crate only exposes them as struct fields;
//!   a unit test keeps the two in sync),
//! - the `_{ns}/*` extension types,
//! - small helpers: capability checks and lenient inbound parsing.
//!
//! Note: the official types are `#[non_exhaustive]`; build them with `Type::new(..)` plus the
//! builder methods and always keep a `_ =>` arm when matching enums.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Round a USD amount to 1e-8 so float sums do not leak noise like `0.30000000000000004`.
pub fn round_usd(amount: f64) -> f64 {
    (amount * 1e8).round() / 1e8
}

/// The ACP `usage` of a prompt turn from the summed provider usage.
///
/// `inputTokens` counts only the prompt tokens that were NOT served from or written to the
/// cache (the convention of Claude Code's ACP adapter and of Anthropic's `input_tokens`), so
/// `totalTokens = inputTokens + outputTokens + cachedReadTokens + cachedWriteTokens` and cost can
/// be computed per bucket without double counting. The cache fields are omitted when the provider
/// does not report them.
pub fn prompt_usage(u: &ruck_harness_llm::Usage) -> Usage {
    let read = u.cache_read_tokens;
    let write = u.cache_write_tokens;
    let input = u.uncached_input_tokens();
    Usage::new(
        input + u.output_tokens + read.unwrap_or(0) + write.unwrap_or(0),
        input,
        u.output_tokens,
    )
    .cached_read_tokens(read)
    .cached_write_tokens(write)
}

pub use agent_client_protocol_schema::v1::*;
pub use agent_client_protocol_schema::ProtocolVersion;

// ---- standard method names (mirrors of AGENT_METHOD_NAMES / CLIENT_METHOD_NAMES) -------------

pub const INITIALIZE: &str = "initialize";
pub const AUTHENTICATE: &str = "authenticate";
pub const SESSION_NEW: &str = "session/new";
pub const SESSION_LOAD: &str = "session/load";
pub const SESSION_LIST: &str = "session/list";
pub const SESSION_RESUME: &str = "session/resume";
pub const SESSION_CLOSE: &str = "session/close";
pub const SESSION_PROMPT: &str = "session/prompt";
pub const SESSION_CANCEL: &str = "session/cancel";
pub const SESSION_SET_MODE: &str = "session/set_mode";
pub const SESSION_SET_CONFIG_OPTION: &str = "session/set_config_option";
pub const SESSION_UPDATE: &str = "session/update";
pub const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
pub const FS_READ_TEXT_FILE: &str = "fs/read_text_file";
pub const FS_WRITE_TEXT_FILE: &str = "fs/write_text_file";

// ---- extension method names -------------------------------------------------------------------

/// The names of the product's extension methods: `_{ns}/ask_user_question` and so on, where
/// `ns` is the identity's `acp_namespace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtMethods {
    /// Agent -> client request: ask the user a question.
    pub ask_user_question: String,
    /// Client -> agent request. Deprecated alias of the standard `session/list`.
    pub list_sessions: String,
    /// Client -> agent request. Pins the model for a session; also reachable as
    /// `session/set_config_option` with `model`.
    pub set_model: String,
    /// Notification (agent -> client) about a model request that is waiting out a rate limit or
    /// transient failure, or was given up on. See `rate_limit_params`.
    pub rate_limit: String,
}

impl ExtMethods {
    pub fn new(ns: &str) -> Self {
        Self {
            ask_user_question: format!("_{ns}/ask_user_question"),
            list_sessions: format!("_{ns}/list_sessions"),
            set_model: format!("_{ns}/set_model"),
            rate_limit: format!("_{ns}/rate_limit"),
        }
    }
}

/// Id of the `model` session config option.
pub const MODEL_CONFIG_ID: &str = "model";

// ---- extension types --------------------------------------------------------------------------

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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsExtRequest {
    pub cwd: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsExtEntry {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub first_prompt: String,
    pub mode: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListSessionsExtResponse {
    pub sessions: Vec<ListSessionsExtEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelRequest {
    pub session_id: String,
    /// `None` (or absent) clears the pin.
    #[serde(default)]
    pub model: Option<String>,
}

// ---- helpers ----------------------------------------------------------------------------------

fn meta_flag(caps: &ClientCapabilities, ns: &str, flag: &str) -> bool {
    caps.meta
        .as_ref()
        .and_then(|m| m.get(ns))
        .and_then(|r| r.get(flag))
        .and_then(Value::as_bool)
        == Some(true)
}

/// Client advertised `_meta.{ns}.askUserQuestion = true`.
pub fn supports_ask_user_question(caps: &ClientCapabilities, ns: &str) -> bool {
    meta_flag(caps, ns, "askUserQuestion")
}

/// Client can render the standard `notice` session update: it advertised
/// `clientCapabilities.session.notices`, or `_meta.{ns}.notices = true`.
pub fn supports_notices(caps: &ClientCapabilities, ns: &str) -> bool {
    caps.session.as_ref().is_some_and(|s| s.notices.is_some()) || meta_flag(caps, ns, "notices")
}

/// Client advertised `_meta.{ns}.rateLimit = true`: it renders `_{ns}/rate_limit`
/// notifications (a live countdown). Others get the status as a plain notice.
pub fn supports_rate_limit(caps: &ClientCapabilities, ns: &str) -> bool {
    meta_flag(caps, ns, "rateLimit")
}

/// Capabilities the product's own client advertises: no fs/terminal, notices, and the
/// ask-user and rate-limit extensions under `_meta.{ns}`.
pub fn client_capabilities(ns: &str) -> ClientCapabilities {
    let Value::Object(meta) =
        serde_json::json!({ns: {"askUserQuestion": true, "notices": true, "rateLimit": true}})
    else {
        unreachable!()
    };
    ClientCapabilities::new()
        .terminal(false)
        .session(
            ClientSessionCapabilities::new()
                .notices(NoticeCapabilities::new())
                .config_options(SessionConfigOptionsCapabilities::new()),
        )
        .meta(meta)
}

/// Flatten prompt content blocks into the text sent to the model. Images, audio and binary
/// resources are dropped.
pub fn prompt_to_text(blocks: &[ContentBlock]) -> String {
    let parts: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.text.clone()),
            ContentBlock::ResourceLink(l) => Some(format!("@{}", l.uri)),
            ContentBlock::Resource(r) => match &r.resource {
                EmbeddedResourceResource::TextResourceContents(t) => {
                    Some(format!("<file uri={}>\n{}\n</file>", t.uri, t.text))
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    parts.join("\n")
}

/// Parse the `prompt` array item by item, skipping blocks of unknown type or shape (the official
/// type would fail the whole request on one unknown block).
pub fn parse_prompt_blocks(prompt: &Value) -> Vec<ContentBlock> {
    let Some(items) = prompt.as_array() else {
        return vec![];
    };
    items
        .iter()
        .filter_map(
            |item| match serde_json::from_value::<ContentBlock>(item.clone()) {
                Ok(b) => Some(b),
                Err(e) => {
                    tracing::debug!("skipping unsupported prompt content block: {e}");
                    None
                }
            },
        )
        .collect()
}

/// Make a request's params tolerant before typed parsing: `mcpServers` defaults to `[]`.
pub fn default_mcp_servers(params: &mut Value) {
    if let Some(m) = params.as_object_mut() {
        if m.get("mcpServers").is_none_or(Value::is_null) {
            m.insert("mcpServers".into(), Value::Array(vec![]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn method_constants_match_official_names() {
        let a = &AGENT_METHOD_NAMES;
        let c = &CLIENT_METHOD_NAMES;
        assert_eq!(INITIALIZE, a.initialize);
        assert_eq!(AUTHENTICATE, a.authenticate);
        assert_eq!(SESSION_NEW, a.session_new);
        assert_eq!(SESSION_LOAD, a.session_load);
        assert_eq!(SESSION_LIST, a.session_list);
        assert_eq!(SESSION_RESUME, a.session_resume);
        assert_eq!(SESSION_CLOSE, a.session_close);
        assert_eq!(SESSION_PROMPT, a.session_prompt);
        assert_eq!(SESSION_CANCEL, a.session_cancel);
        assert_eq!(SESSION_SET_MODE, a.session_set_mode);
        assert_eq!(SESSION_SET_CONFIG_OPTION, a.session_set_config_option);
        assert_eq!(SESSION_UPDATE, c.session_update);
        assert_eq!(SESSION_REQUEST_PERMISSION, c.session_request_permission);
        assert_eq!(FS_READ_TEXT_FILE, c.fs_read_text_file);
        assert_eq!(FS_WRITE_TEXT_FILE, c.fs_write_text_file);
    }

    #[test]
    fn capability_helpers() {
        let none = ClientCapabilities::new();
        assert!(!supports_ask_user_question(&none, "acme"));
        assert!(!supports_notices(&none, "acme"));
        let ours = client_capabilities("acme");
        assert!(supports_ask_user_question(&ours, "acme"));
        assert!(supports_notices(&ours, "acme"));
        let i: InitializeRequest = serde_json::from_value(json!({
            "protocolVersion":1,
            "clientCapabilities":{"fs":{"readTextFile":true},"session":{"notices":{}}}
        }))
        .unwrap();
        assert!(i.client_capabilities.fs.read_text_file);
        assert!(supports_notices(&i.client_capabilities, "acme"));
        assert!(!supports_ask_user_question(&i.client_capabilities, "acme"));
        let j: InitializeRequest = serde_json::from_value(json!({
            "protocolVersion":1,
            "clientCapabilities":{"_meta":{"acme":{"notices":true}}}
        }))
        .unwrap();
        assert!(supports_notices(&j.client_capabilities, "acme"));
    }

    #[test]
    fn prompt_parsing_skips_unknown_blocks() {
        let blocks = parse_prompt_blocks(&json!([
            {"type":"text","text":"a"},
            {"type":"resource_link","uri":"file:///x","name":"x"},
            {"type":"resource","resource":{"uri":"file:///y","text":"body","mimeType":"text/plain"}},
            {"type":"audio","data":"..","mimeType":"audio/wav"},
            {"type":"hologram","x":1},
            {"nonsense":true},
            {"type":"resource","resource":{"uri":"file:///z","blob":"AAAA"}}
        ]));
        assert_eq!(blocks.len(), 5);
        assert_eq!(
            prompt_to_text(&blocks),
            "a\n@file:///x\n<file uri=file:///y>\nbody\n</file>"
        );
        assert!(parse_prompt_blocks(&json!(null)).is_empty());
    }

    #[test]
    fn mcp_servers_default() {
        let mut p = json!({"cwd":"/p"});
        default_mcp_servers(&mut p);
        assert_eq!(p["mcpServers"], json!([]));
        let n: NewSessionRequest = serde_json::from_value(p).unwrap();
        assert!(n.mcp_servers.is_empty());
        let mut p = json!({"mcpServers":null});
        default_mcp_servers(&mut p);
        assert_eq!(p["mcpServers"], json!([]));
    }

    #[test]
    fn error_conversion() {
        let e: crate::rpc::RpcError =
            agent_client_protocol::Error::from(Error::method_not_found()).into();
        assert_eq!(e.code, -32601);
    }
}
