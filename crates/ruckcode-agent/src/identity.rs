//! The ruckcode product identity: the one place its names, paths and environment prefix are
//! spelled out. Every `ruck-harness-*` crate receives it as a parameter instead of hardcoding it.
//!
//! This lives in ruckcode-agent because the other crates that need it (cli, app, acp) already
//! depend on core, and core is the future ruckcode-agent crate where the product identity
//! belongs. `AppIdentity` is a foreign type, so the constructor is a `const` here rather than an
//! inherent `AppIdentity::ruckcode()`.

use ruck_harness_ident::AppIdentity;

/// The product's release version, the single source of truth: what `rc --version` prints (minus
/// the `rc ` prefix), what ACP `agentInfo` and MCP `clientInfo` report, the transcript's
/// `ruckcode_version`, the updater's current version and the desktop app's version. Release
/// builds get the `RUCKCODE_VERSION` the release workflow sets (`0.26.1`); local builds get
/// `<package version>-dev+<git sha>` (see build.rs).
pub const VERSION: &str = env!("RUCKCODE_RELEASE_VERSION");

pub const RUCKCODE: AppIdentity = AppIdentity {
    name: "ruckcode",
    env_prefix: "RUCKCODE",
    project_dir: ".ruckcode",
    config_dir_env: "RUCKCODE_CONFIG_DIR",
    user_config_subdir: "ruckcode",
    data_dir_name: "ruckcode",
    version: VERSION,
    // Deliberately the package version, not VERSION: the user agent has always carried it, and
    // keeping it release-independent keeps provider/web request fingerprints stable.
    user_agent: concat!("ruckcode/", env!("CARGO_PKG_VERSION")),
    homepage_url: "https://github.com/ruckc/ruckcode",
    acp_namespace: "ruckcode",
    cli_name: "rc",
    extra_protected_project_paths: &[".mcp.json"],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value is a literal taken from the code before the identity was threaded through.
    #[test]
    fn ruckcode_identity_is_byte_identical_to_the_old_hardcoded_values() {
        assert_eq!(RUCKCODE.name, "ruckcode");
        assert_eq!(RUCKCODE.env_prefix, "RUCKCODE");
        assert_eq!(RUCKCODE.project_dir, ".ruckcode");
        assert_eq!(RUCKCODE.config_dir_env, "RUCKCODE_CONFIG_DIR");
        assert_eq!(RUCKCODE.user_config_subdir, "ruckcode");
        assert_eq!(RUCKCODE.data_dir_name, "ruckcode");
        assert_eq!(
            RUCKCODE.user_agent,
            format!("ruckcode/{}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            RUCKCODE.web_user_agent(),
            format!(
                "ruckcode/{} (+https://github.com/ruckc/ruckcode)",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert_eq!(RUCKCODE.acp_namespace, "ruckcode");
        assert_eq!(RUCKCODE.cli_name, "rc");
        assert_eq!(RUCKCODE.extra_protected_project_paths, [".mcp.json"]);
    }

    /// Release builds (RUCKCODE_VERSION set at compile time) report exactly that, minus a leading
    /// `v`; local builds report `<package version>-dev+<sha>` or the bare package version.
    #[test]
    fn version_follows_the_release_env_var() {
        match option_env!("RUCKCODE_VERSION").map(|v| v.trim().trim_start_matches('v')) {
            Some(v) if !v.is_empty() => assert_eq!(VERSION, v),
            _ => {
                let pkg = env!("CARGO_PKG_VERSION");
                assert!(
                    VERSION == pkg || VERSION.starts_with(&format!("{pkg}-dev+")),
                    "{VERSION}"
                );
            }
        }
        assert_eq!(RUCKCODE.version, VERSION);
    }

    /// The fixture the harness crates test against must not drift from the real identity.
    #[test]
    fn the_harness_test_fixture_matches() {
        // `version` is the build's release version, which the fixture cannot know.
        let fixture = AppIdentity {
            version: VERSION,
            ..ruck_harness_ident::testing::RUCKCODE
        };
        assert_eq!(fixture, RUCKCODE);
    }
}
