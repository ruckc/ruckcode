//! Session policy: adjustments applied to the loaded configuration and modes when a session
//! starts, before anything is built from them.
//!
//! The harness runs whatever [`SessionPolicy`] it is given, or none. [`RemoteSandboxPolicy`] is
//! the one ruckcode uses: it lays the verified remote copy of the project's sandbox policy over
//! the user's config (see [`crate::project_policy`]).

use async_trait::async_trait;
use ruck_harness_ident::AppIdentity;
use std::path::Path;

use crate::config::HarnessConfig;
use crate::modes::ModeSet;

#[async_trait]
pub trait SessionPolicy: Send + Sync {
    /// Adjust `cfg` and `modes` for a session in `cwd`. Returns notices for the user.
    async fn apply(&self, cwd: &Path, cfg: &mut HarnessConfig, modes: &mut ModeSet) -> Vec<String>;
}

/// Lays the project's sandbox policy, as committed on the remote's default branch, over the
/// session's sandbox settings and modes. The working tree's own sandbox settings were already
/// dropped by the config loaders.
#[derive(Debug, Clone, Copy)]
pub struct RemoteSandboxPolicy {
    identity: AppIdentity,
}

impl RemoteSandboxPolicy {
    /// The policy of the product `identity`: its data directory holds the pins and caches.
    pub fn new(identity: AppIdentity) -> Self {
        Self { identity }
    }
}

#[async_trait]
impl SessionPolicy for RemoteSandboxPolicy {
    async fn apply(&self, cwd: &Path, cfg: &mut HarnessConfig, modes: &mut ModeSet) -> Vec<String> {
        use ruck_harness_sandbox::ProjectPolicy;
        if !cfg.sandbox.enabled || cfg.sandbox.project_policy == ProjectPolicy::Off {
            return vec![];
        }
        let Some(data) = crate::project_policy::data_dir_for(&self.identity) else {
            return vec![];
        };
        let (project, mode) = (cwd.to_path_buf(), cfg.sandbox.project_policy);
        let Ok(res) = tokio::task::spawn_blocking(move || {
            crate::project_policy::resolve(&project, &data, mode)
        })
        .await
        else {
            return vec![];
        };
        let mut notices = res.notices.clone();
        notices.extend(crate::project_policy::apply(&res, &mut cfg.sandbox, modes));
        notices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Toy;

    #[async_trait]
    impl SessionPolicy for Toy {
        async fn apply(&self, _: &Path, cfg: &mut HarnessConfig, _: &mut ModeSet) -> Vec<String> {
            cfg.max_turn_requests = 3;
            vec!["toy".into()]
        }
    }

    #[tokio::test]
    async fn a_policy_adjusts_the_config_and_reports_notices() {
        let mut cfg = HarnessConfig::default();
        let mut modes = ModeSet::implicit();
        let notices = Toy.apply(Path::new("/w"), &mut cfg, &mut modes).await;
        assert_eq!(
            (cfg.max_turn_requests, notices),
            (3, vec!["toy".to_string()])
        );
    }

    /// With the sandbox off the remote policy does nothing and reads no data directory.
    #[tokio::test]
    async fn the_remote_policy_is_inert_when_the_sandbox_is_off() {
        let mut cfg = HarnessConfig::default();
        cfg.sandbox.enabled = false;
        let mut modes = ModeSet::implicit();
        let p = RemoteSandboxPolicy::new(crate::identity::RUCKCODE);
        assert!(p
            .apply(Path::new("/w"), &mut cfg, &mut modes)
            .await
            .is_empty());
    }
}
