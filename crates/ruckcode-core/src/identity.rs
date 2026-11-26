//! The ruckcode product identity: the one place its names, paths and environment prefix are
//! spelled out. Every `ruck-harness-*` crate receives it as a parameter instead of hardcoding it.
//!
//! This lives in ruckcode-core because the other crates that need it (cli, app, acp) already
//! depend on core, and core is the future ruckcode-agent crate where the product identity
//! belongs. `AppIdentity` is a foreign type, so the constructor is a `const` here rather than an
//! inherent `AppIdentity::ruckcode()`.

use ruck_harness_ident::AppIdentity;

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
        assert_eq!(RUCKCODE.extra_protected_project_paths, [".mcp.json"]);
    }

    /// The fixture the harness crates test against must not drift from the real identity.
    #[test]
    fn the_harness_test_fixture_matches() {
        assert_eq!(ruck_harness_ident::testing::RUCKCODE, RUCKCODE);
    }
}
