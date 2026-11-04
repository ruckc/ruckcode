//! Wiring of the command sandbox into the agent: the network-domain prompt and the per-call
//! handle given to tools.

use crate::agent::{Client, PermissionDecision, PermissionRequest};
use async_trait::async_trait;
use ruckcode_sandbox::{DomainAnswer, DomainPrompter, SandboxConfig};
use ruckcode_tools::{SandboxHandle, ToolKind};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Asks the user (through the existing permission flow) whether a command may reach a domain.
/// "Allow" is once, "Always allow" remembers it for this project, "Reject" denies.
pub(crate) struct NetPrompter {
    pub client: Arc<dyn Client>,
    pub session_id: String,
}

#[async_trait]
impl DomainPrompter for NetPrompter {
    async fn ask(&self, host: &str, tool_call_id: &str) -> DomainAnswer {
        let req = PermissionRequest {
            tool_call_id: tool_call_id.to_string(),
            title: format!(
                "Allow network access to {host}? (Always allow remembers it for this project)"
            ),
            kind: ToolKind::Fetch,
            input: json!({"sandbox": "network", "host": host}),
            allow_always: true,
        };
        match self.client.request_permission(&self.session_id, req).await {
            PermissionDecision::AllowOnce => DomainAnswer::Once,
            PermissionDecision::AllowAlways => DomainAnswer::Always,
            PermissionDecision::Reject | PermissionDecision::Cancelled => DomainAnswer::Deny,
        }
    }
}

/// Paths in-process tools must not write: ruckcode's own configuration and the parts of `.git`
/// that could redirect the remote or run code outside the sandbox (`.git/config`,
/// `.git/hooks/**`, `.git/info/**`, remote-tracking refs, ...).
pub(crate) fn protected_write(project: &Path, config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut v = vec![project.join(".ruckcode"), project.join(".mcp.json")];
    v.extend(config_dir.map(Path::to_path_buf));
    v.extend(ruckcode_sandbox::git_protected_paths(project));
    v
}

pub(crate) fn handle(
    sandbox: Option<Arc<ruckcode_sandbox::Sandbox>>,
    cfg: SandboxConfig,
    project: &Path,
    config_dir: Option<&Path>,
) -> SandboxHandle {
    let deny_read =
        ruckcode_sandbox::deny_list(&cfg, dirs::home_dir().as_deref(), project, config_dir);
    SandboxHandle {
        sandbox,
        cfg,
        protected_write: protected_write(project, config_dir),
        deny_read,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruckcode_tools::SandboxHandle;

    #[test]
    fn write_guards_cover_git_config_hooks_and_info() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().canonicalize().unwrap();
        std::fs::create_dir_all(p.join(".git/hooks")).unwrap();
        std::fs::write(p.join(".git/config"), "[core]\n").unwrap();
        let h = SandboxHandle {
            sandbox: None,
            cfg: SandboxConfig::default(),
            protected_write: protected_write(&p, None),
            deny_read: vec![],
        };
        for blocked in [
            ".git/config",
            ".git/hooks/post-commit",
            ".git/hooks/sub/x",
            ".git/info/exclude",
            ".git/refs/remotes/origin/main",
            ".git/hooks/../config",
            ".ruckcode/config.toml",
        ] {
            assert!(h.blocks_write(&p.join(blocked)), "{blocked}");
        }
        for ok in [
            ".git/index",
            ".git/HEAD",
            ".git/refs/heads/x",
            "src/a.rs",
            ".gitignore",
        ] {
            assert!(!h.blocks_write(&p.join(ok)), "{ok}");
        }
    }
}
