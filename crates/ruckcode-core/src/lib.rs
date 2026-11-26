//! The ruckcode product on the agent harness: its extension (prompt, tools, guard hints), its
//! configuration sections, the setup and mode-editing APIs.
//!
//! The generic engine lives in `ruck-harness-core`. The modules below re-export it so that the
//! CLI, ACP server and app keep their import paths; they go away when this crate becomes
//! `ruckcode-agent`.

pub mod config;
pub mod extension;
pub mod guard_ext;
pub mod identity;
pub mod init;
pub mod keys;
pub mod mode_ext;
pub mod modes_api;
mod modes_edit;
pub mod project_policy;
pub mod prompt_ext;
pub mod search_pattern;
pub mod setup;
pub mod setup_api;
pub mod transcript;
pub mod transcript_api;

// Shims: the generic engine, under its old paths.
pub use ruck_harness_core::{
    agent, agent_ext, context, modes, permissions, prompt, retry, session_policy, thinking_guard,
    tool_loop_guard,
};

pub use config::{Config, Pricing, Profile, ProviderKind};
pub use extension::RuckcodeExtension;
pub use ruck_harness_core::{
    Agent, AgentExtension, AgentOptions, Client, Command, ModeDef, ModeInfo, ModeSchema, ModeSet,
    Permission, PermissionDecision, PermissionRequest, RateLimitEvent, SessionEvent, SessionExt,
    SessionInfo, StopReason, ToolStatus, MAX_CLIENT_INSTRUCTIONS,
};
pub use ruck_harness_llm::Usage as TokenUsage;
pub use ruck_harness_mcp::McpServerConfig;
pub use ruck_harness_sandbox;
