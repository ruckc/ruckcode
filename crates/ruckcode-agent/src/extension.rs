//! ruckcode's [`AgentExtension`]: the coding tools, mise tasks, prompt, guard hints and sandbox
//! policy that turn the generic agent loop into the `rc` coding agent.

use anyhow::Result;
use async_trait::async_trait;
use ruck_harness_ident::AppIdentity;
use ruck_harness_llm::Provider;
use ruck_harness_tools::{PlanItem, Tool};
use ruckcode_mise::{MiseTask, TaskSelector};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{make_provider_with, mise_from_table, HarnessConfig, MiseConfig, Profile};
use crate::guard_ext::ruckcode_guard_hints;
use crate::identity::RUCKCODE;
use crate::mode_ext::{RuckcodeModeExt, RUCKCODE_MODE_SCHEMA};
use crate::prompt_ext::RuckcodePrompt;
use ruck_harness_core::agent::{Agent, AgentOptions};
use ruck_harness_core::agent_ext::{AgentExtension, SessionExt};
use ruck_harness_core::modes::{ModeDef, ModeSchema, ModeSet};
use ruck_harness_core::prompt::PromptProvider;
use ruck_harness_core::session_policy::{RemoteSandboxPolicy, SessionPolicy};
use ruck_harness_core::tool_loop_guard::GuardHints;
use ruck_harness_core::transcript::{self, Stamped, Unfinished};
use ruck_harness_core::transcript_api::{self, UnfinishedSession};

/// The `rc` coding agent: coding, web, plan and ask tools, mise tasks as tools, ruckcode's
/// prompt and loop-guard hints, and the remote sandbox policy.
#[derive(Debug, Clone, Copy, Default)]
pub struct RuckcodeExtension;

#[async_trait]
impl AgentExtension for RuckcodeExtension {
    fn identity(&self) -> &AppIdentity {
        &RUCKCODE
    }

    fn prompt(&self) -> Arc<dyn PromptProvider> {
        Arc::new(RuckcodePrompt)
    }

    fn guard_hints(&self) -> GuardHints {
        ruckcode_guard_hints()
    }

    fn session_policy(&self) -> Option<Arc<dyn SessionPolicy>> {
        Some(Arc::new(RemoteSandboxPolicy::new(RUCKCODE)))
    }

    fn mode_schema(&self) -> &ModeSchema {
        &RUCKCODE_MODE_SCHEMA
    }

    fn make_provider(
        &self,
        profile: &Profile,
        user_dir: Option<&Path>,
    ) -> Result<Arc<dyn Provider>> {
        make_provider_with(profile, user_dir)
    }

    async fn session_init(
        &self,
        cwd: &Path,
        _cfg: &HarnessConfig,
        raw: &toml::Table,
        _modes: &ModeSet,
    ) -> Result<Box<dyn SessionExt>> {
        let mise = mise_from_table(raw)?;
        Ok(Box::new(RuckcodeSession {
            mise,
            tasks: ruckcode_mise::discover(cwd).await,
        }))
    }

    fn decode_plan(&self, tool: &str, input: &Value) -> Option<Vec<PlanItem>> {
        ruckcode_tools::decode_builtin_plan(tool, input)
    }

    fn builtin_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        ruckcode_tools::builtin_tool(name).cloned()
    }
}

/// The mise tasks discovered in the working directory, and the `[mise]` settings.
struct RuckcodeSession {
    mise: MiseConfig,
    tasks: Vec<MiseTask>,
}

#[async_trait]
impl SessionExt for RuckcodeSession {
    fn builtin_tools(
        &self,
        _mode: &ModeDef,
        cfg: &HarnessConfig,
        user_dir: Option<&Path>,
    ) -> Vec<Arc<dyn Tool>> {
        ruckcode_tools::builtin_tools_with(&cfg.web_resolved(&RUCKCODE, user_dir))
    }

    fn dynamic_tools(
        &self,
        mode_id: &str,
        mode: &ModeDef,
        _cfg: &HarnessConfig,
    ) -> Vec<Arc<dyn Tool>> {
        match TaskSelector::new(&mode.mise_tasks()) {
            Ok(sel) => {
                let selected = sel.select(&self.tasks).into_iter().cloned().collect();
                ruckcode_mise::tools_for(selected, self.mise.lazy_threshold)
            }
            Err(e) => {
                tracing::warn!("invalid mise_tasks selector in mode `{mode_id}`: {e}");
                vec![]
            }
        }
    }

    async fn refresh(&mut self, cwd: &Path) {
        self.tasks = ruckcode_mise::discover(cwd).await;
    }
}

/// `Agent::ruckcode(..)`: an agent running the ruckcode coding extension (`Agent` is a harness
/// type, so this is a trait; import it to call the constructor).
pub trait RuckcodeAgent {
    fn ruckcode(options: AgentOptions) -> Self;
}

impl RuckcodeAgent for Agent {
    fn ruckcode(options: AgentOptions) -> Self {
        Self::new(options, Arc::new(RuckcodeExtension))
    }
}

/// The user data directory of ruckcode (`RUCKCODE_DATA_DIR`, else `<data dir>/ruckcode`).
pub fn data_dir() -> Option<PathBuf> {
    ruck_harness_core::project_policy::data_dir_for(&RUCKCODE)
}

/// [`transcript::assess_unfinished_with`] decoding ruckcode's plan tool.
pub fn assess_unfinished(records: &[Stamped]) -> Unfinished {
    transcript::assess_unfinished_with(records, &ruckcode_tools::decode_builtin_plan)
}

/// [`transcript_api::find_unfinished_with`] decoding ruckcode's plan tool.
pub fn find_unfinished(project: &Path, limit: Option<usize>) -> Vec<UnfinishedSession> {
    transcript_api::find_unfinished_with(
        &RUCKCODE,
        project,
        limit,
        &ruckcode_tools::decode_builtin_plan,
    )
}
