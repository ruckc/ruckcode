//! Agent core: configuration, lifecycle modes, permissions, transcripts and the agent loop.

pub mod agent;
pub mod config;
pub mod init;
pub mod keys;
pub mod modes;
pub mod permissions;
pub mod setup;
pub mod setup_api;
pub mod transcript;

mod prompt;

pub use agent::{
    Agent, AgentOptions, Client, ModeInfo, PermissionDecision, PermissionRequest, SessionEvent,
    SessionInfo, StopReason, ToolStatus,
};
pub use config::{Config, Profile, ProviderKind};
pub use modes::{ModeDef, ModeSet};
pub use permissions::Permission;
pub use ruckcode_mcp::McpServerConfig;
