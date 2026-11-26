//! The seam between the generic agent loop and a product.
//!
//! The loop knows nothing about coding tools, mise or ruckcode's prompt. A product implements
//! [`AgentExtension`] (once per process) and [`SessionExt`] (once per session) and hands the
//! former to [`Agent::new`](crate::Agent::new).

use anyhow::Result;
use async_trait::async_trait;
use ruck_harness_ident::AppIdentity;
use ruck_harness_llm::Provider;
use ruck_harness_tools::{PlanItem, Tool};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

use crate::config::{HarnessConfig, Profile};
use crate::modes::{ModeDef, ModeSchema, ModeSet};
use crate::prompt::PromptProvider;
use crate::session_policy::SessionPolicy;
use crate::tool_loop_guard::GuardHints;

/// Everything product-specific the agent loop needs. Implementations are shared by all sessions.
#[async_trait]
pub trait AgentExtension: Send + Sync {
    /// Names, environment variables, directories and the ACP namespace of the product.
    fn identity(&self) -> &AppIdentity;

    /// The product's part of the system prompt.
    fn prompt(&self) -> Arc<dyn PromptProvider>;

    /// How the tool-loop guard recognises the product's tools (`GuardHints::default()` for none,
    /// which turns off the search and oscillation detectors).
    fn guard_hints(&self) -> GuardHints;

    /// Adjusts the loaded config and modes when a session starts.
    fn session_policy(&self) -> Option<Arc<dyn SessionPolicy>>;

    /// The product's additions to the mode format.
    fn mode_schema(&self) -> &ModeSchema;

    /// The provider client for `profile` (resolving its API key from `user_dir`).
    fn make_provider(
        &self,
        profile: &Profile,
        user_dir: Option<&Path>,
    ) -> Result<Arc<dyn Provider>>;

    /// Per-session product state. `raw` is the merged config table the harness half `cfg` was
    /// deserialised from, so the product reads its own sections from it; an error (an invalid
    /// product section) fails the session start.
    async fn session_init(
        &self,
        cwd: &Path,
        cfg: &HarnessConfig,
        raw: &toml::Table,
        modes: &ModeSet,
    ) -> Result<Box<dyn SessionExt>>;

    /// The plan a call of the tool `tool` with `input` writes, for a tool that is not in the
    /// session's current tool set (replaying a transcript whose tool the mode no longer offers).
    fn decode_plan(&self, tool: &str, input: &Value) -> Option<Vec<PlanItem>>;

    /// The built-in tool called `name`, for profile allow-lists and the prompt's `offers`.
    fn builtin_tool(&self, name: &str) -> Option<Arc<dyn Tool>>;
}

/// The product's state for one session.
#[async_trait]
pub trait SessionExt: Send {
    /// The built-in tools the product offers in `mode`, before the profile's filter and ahead of
    /// `SwitchMode` and the MCP tools. `user_dir` is the user config directory override.
    fn builtin_tools(
        &self,
        mode: &ModeDef,
        cfg: &HarnessConfig,
        user_dir: Option<&Path>,
    ) -> Vec<Arc<dyn Tool>>;

    /// Session-scoped tools (discovered at runtime, e.g. project tasks) appended after the MCP
    /// tools. They are not subject to `mode.tools` or the profile's allow-list.
    fn dynamic_tools(
        &self,
        _mode_id: &str,
        _mode: &ModeDef,
        _cfg: &HarnessConfig,
    ) -> Vec<Arc<dyn Tool>> {
        vec![]
    }

    /// Re-discover what depends on the working tree (on a mode switch).
    async fn refresh(&mut self, cwd: &Path);
}
