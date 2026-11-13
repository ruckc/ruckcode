//! Provider-neutral LLM types plus Anthropic and OpenAI-compatible clients.

pub mod anthropic;
pub mod mock;
pub mod models;
pub mod openai;
pub mod ratelimit;
mod sse;

pub use models::{list_models, ApiKind, ModelInfo};
pub use ratelimit::{is_transport_error, ApiError, Bucket, LimitKind, RateLimitInfo};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Model reasoning. `signature` must be round-tripped for Anthropic extended thinking.
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// Concatenated text of all text blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
    /// Provider-specific reasoning configuration, passed through as-is
    /// (Anthropic: the `thinking` object; OpenAI-compatible: `reasoning_effort` string).
    pub thinking: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other,
}

/// Token usage of one model request (or a sum of several).
///
/// `input_tokens` is the WHOLE prompt, cached tokens included, for every provider: OpenAI's
/// `prompt_tokens` already includes them, and the Anthropic client adds `cache_read_input_tokens`
/// and `cache_creation_input_tokens` to `input_tokens` (which excludes them). `cache_read_tokens`
/// and `cache_write_tokens` are the parts of `input_tokens` served from / written to the prompt
/// cache; `None` means the provider did not say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

impl Usage {
    /// Whether the provider reported anything. A request cannot really consume zero tokens, so
    /// an all-zero usage means "no usage block" (e.g. a local endpoint that reports none).
    pub fn is_reported(&self) -> bool {
        self.input_tokens > 0
            || self.output_tokens > 0
            || self.cache_read_tokens.is_some_and(|n| n > 0)
            || self.cache_write_tokens.is_some_and(|n| n > 0)
    }

    /// Prompt tokens that were neither read from nor written to the cache.
    pub fn uncached_input_tokens(&self) -> u64 {
        self.input_tokens.saturating_sub(
            self.cache_read_tokens
                .unwrap_or(0)
                .saturating_add(self.cache_write_tokens.unwrap_or(0)),
        )
    }

    /// Add another request's usage; a cache count stays `None` until some request reports it.
    pub fn add(&mut self, other: &Usage) {
        fn opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
            match (a, b) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
            }
        }
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens = opt(self.cache_read_tokens, other.cache_read_tokens);
        self.cache_write_tokens = opt(self.cache_write_tokens, other.cache_write_tokens);
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Usage,
    /// Rate-limit headers the server sent with this response (used for proactive pacing).
    pub rate_limit: Option<RateLimitInfo>,
}

/// Incremental output while a response streams.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolUseStart { id: String, name: String },
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;

    /// Send a request, forwarding streaming events to `events`, and return the final message.
    async fn complete(
        &self,
        request: Request,
        events: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response>;
}

/// Accumulates a partially-streamed tool call.
#[derive(Default)]
pub(crate) struct PartialToolUse {
    pub id: String,
    pub name: String,
    pub json: String,
}

impl PartialToolUse {
    pub fn finish(self) -> ContentBlock {
        let input = if self.json.trim().is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(&self.json)
                .unwrap_or_else(|_| serde_json::json!({ "_invalid_json": self.json }))
        };
        ContentBlock::ToolUse {
            id: self.id,
            name: self.name,
            input,
        }
    }
}

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("ruckcode/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}
