//! The identity of the product built on the harness.
//!
//! Everything a harness crate would otherwise hardcode about the product (its name in
//! model-visible messages, its environment variable prefix, its project and user directories,
//! its HTTP user agent) is a field of [`AppIdentity`]. The product defines one value and passes
//! it to the harness crates; the harness crates never name a product themselves.

use std::path::{Path, PathBuf};

/// Names and paths that identify the product built on the harness.
///
/// All fields are `'static` so the identity is `Copy` and can be a `const`. `AppIdentity`
/// deliberately has no `Default`: there is no neutral product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AppIdentity {
    /// Product name as it appears in model-visible and user-visible messages ("ruckcode").
    pub name: &'static str,
    /// Prefix of the environment variables the product reads and exports ("RUCKCODE").
    pub env_prefix: &'static str,
    /// The per-project directory, relative to the project root (".ruckcode").
    pub project_dir: &'static str,
    /// The environment variable overriding the user config directory ("RUCKCODE_CONFIG_DIR").
    pub config_dir_env: &'static str,
    /// The product's subdirectory of the platform user config directory ("ruckcode").
    pub user_config_subdir: &'static str,
    /// The product's subdirectory of the platform user data directory ("ruckcode").
    pub data_dir_name: &'static str,
    /// The HTTP `User-Agent` of provider and model-listing requests ("ruckcode/0.1.0").
    pub user_agent: &'static str,
    /// Where the product is documented; appended to the user agent of web tool requests.
    pub homepage_url: &'static str,
    /// The ACP extension namespace (`_meta.<ns>.*`, `_<ns>/*` methods).
    pub acp_namespace: &'static str,
    /// Project files, besides `project_dir`, that sandboxed commands cannot write
    /// (relative to the project root, e.g. ".mcp.json").
    pub extra_protected_project_paths: &'static [&'static str],
}

impl AppIdentity {
    /// `{env_prefix}_{suffix}`, e.g. `env_var("EVENT")` is `RUCKCODE_EVENT`.
    pub fn env_var(&self, suffix: &str) -> String {
        format!("{}_{}", self.env_prefix, suffix)
    }

    /// `project_dir` under `project`.
    pub fn project_path(&self, project: &Path) -> PathBuf {
        project.join(self.project_dir)
    }

    /// Every project path sandboxed commands may not write: `project_dir` first, then the extras.
    pub fn protected_project_paths(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.project_dir).chain(self.extra_protected_project_paths.iter().copied())
    }

    /// The user agent of web tool requests: the plain user agent plus the homepage (when set).
    pub fn web_user_agent(&self) -> String {
        if self.homepage_url.is_empty() {
            self.user_agent.to_string()
        } else {
            format!("{} (+{})", self.user_agent, self.homepage_url)
        }
    }
}

/// Fixtures for the unit tests of crates that consume an identity. `RUCKCODE` repeats the real
/// product values as literals on purpose: tests compare against them to catch drift.
#[cfg(feature = "testing")]
pub mod testing {
    use super::AppIdentity;

    /// What `ruckcode_core::identity::RUCKCODE` must equal (all workspace crates share a version).
    pub const RUCKCODE: AppIdentity = AppIdentity {
        name: "ruckcode",
        env_prefix: "RUCKCODE",
        project_dir: ".ruckcode",
        config_dir_env: "RUCKCODE_CONFIG_DIR",
        user_config_subdir: "ruckcode",
        data_dir_name: "ruckcode",
        user_agent: concat!("ruckcode/", env!("CARGO_PKG_VERSION")),
        homepage_url: "https://github.com/ruckc/ruckcode",
        acp_namespace: "ruckcode",
        extra_protected_project_paths: &[".mcp.json"],
    };

    /// A second product, to prove nothing is hardcoded.
    pub const ACME: AppIdentity = AppIdentity {
        name: "acme",
        env_prefix: "ACME",
        project_dir: ".acme",
        config_dir_env: "ACME_CONFIG_DIR",
        user_config_subdir: "acme-agent",
        data_dir_name: "acme-data",
        user_agent: "acme/9.9.9",
        homepage_url: "https://example.com/acme",
        acp_namespace: "acme",
        extra_protected_project_paths: &[".acme-tools.json"],
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: AppIdentity = AppIdentity {
        name: "x",
        env_prefix: "X",
        project_dir: ".x",
        config_dir_env: "X_CONFIG_DIR",
        user_config_subdir: "x",
        data_dir_name: "x",
        user_agent: "x/1",
        homepage_url: "https://x.test",
        acp_namespace: "x",
        extra_protected_project_paths: &[".x.json"],
    };

    #[test]
    fn helpers_derive_from_the_fields() {
        assert_eq!(ID.env_var("EVENT"), "X_EVENT");
        assert_eq!(ID.project_path(Path::new("/p")), Path::new("/p/.x"));
        assert_eq!(
            ID.protected_project_paths().collect::<Vec<_>>(),
            [".x", ".x.json"]
        );
        assert_eq!(ID.web_user_agent(), "x/1 (+https://x.test)");
        let bare = AppIdentity {
            homepage_url: "",
            ..ID
        };
        assert_eq!(bare.web_user_agent(), "x/1");
    }
}
