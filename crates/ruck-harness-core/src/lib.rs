//! Generic agent harness: the agent loop, sessions, modes, permissions, layered config,
//! transcripts and the loop guards. Product behaviour (prompt text, tools, guard hints, project
//! policy) is supplied through [`AgentExtension`]; this crate knows nothing about any product.

pub mod agent;
pub mod agent_ext;
pub mod config;
pub mod context;
pub mod managed;
pub mod modes;
pub mod permissions;
pub mod project_policy;
pub mod prompt;
pub mod retry;
mod sandbox;
pub mod session_policy;
pub mod thinking_guard;
pub mod tool_loop_guard;
pub mod transcript;
pub mod transcript_api;

pub use agent::{
    Agent, AgentOptions, Client, Command, ModeInfo, PermissionDecision, PermissionRequest,
    RateLimitEvent, SessionEvent, SessionInfo, StopReason, ToolStatus,
};
pub use agent_ext::{AgentExtension, SessionExt};
pub use config::{HarnessConfig, Pricing, Profile, ProviderKind};
pub use modes::{ModeDef, ModeSchema, ModeSet};
pub use permissions::Permission;
pub use prompt::MAX_CLIENT_INSTRUCTIONS;
