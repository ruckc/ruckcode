//! Layered configuration: built-in defaults, user config, then project config.
//!
//! The generic half lives in [`harness`] ([`HarnessConfig`], [`ConfigLoader`], [`Profile`], ...)
//! and knows nothing about the ruckcode product. [`Config`] adds the product sections
//! (`[update]`, `[mise]`, `check_updates`) and derefs to the [`HarnessConfig`], so
//! `config.sandbox`, `config.profiles` and the rest keep working unchanged.

use anyhow::{Context, Result};
use ruck_harness_llm::Provider;
use serde::Deserialize;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::identity::RUCKCODE;
use crate::thinking_guard::OllamaOptions;

mod harness;
pub use harness::user_config_dir as user_config_dir_for;
pub use harness::{ConfigLoader, HarnessConfig, McpConfig, Pricing, Profile, ProviderKind};

impl Profile {
    /// Whether the built-in tool `name` is offered with this profile. A tool's
    /// [`LeanClass`](ruck_harness_tools::LeanClass) says how `lean_tools` treats it; a name that is
    /// not a built-in tool is treated as [`LeanClass::Keep`](ruck_harness_tools::LeanClass::Keep).
    pub fn allows_builtin(&self, name: &str) -> bool {
        let class = ruckcode_tools::builtin_tool(name)
            .map(|t| t.lean_class())
            .unwrap_or_default();
        self.allows_class(name, class)
    }
}

fn default_lazy_threshold() -> usize {
    12
}

/// `[update]`: how `rc` keeps itself current (see the README, "Updates").
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UpdateConfig {
    /// Download, verify and install new releases in the background (TUI only); the new version
    /// runs from the next launch.
    #[serde(default = "default_true")]
    pub auto: bool,
    /// Check for new releases at most once a day and mention them. `false` also stops `auto`.
    #[serde(default = "default_true")]
    pub check: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            auto: true,
            check: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MiseConfig {
    #[serde(default = "default_lazy_threshold")]
    pub lazy_threshold: usize,
}

impl Default for MiseConfig {
    fn default() -> Self {
        Self {
            lazy_threshold: default_lazy_threshold(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// The ruckcode product's own top-level keys, read from the same table as the harness's.
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct ProductSections {
    /// Show a once-a-day "update available" hint (`rc` TUI and headless). Never contacts
    /// anything but the release site; `RUCKCODE_NO_UPDATE_CHECK=1` also disables it.
    #[serde(default = "default_true")]
    check_updates: bool,
    #[serde(default)]
    update: UpdateConfig,
    #[serde(default)]
    mise: MiseConfig,
}

/// `config.toml` as `rc` reads it: the harness's sections plus the product's.
///
/// Both halves are deserialised from the same merged table rather than with
/// `#[serde(flatten)]`, which keeps the error messages of the harness half exactly as they were
/// (flatten buffers the input and rewrites type errors). Deref gives `config.sandbox` etc.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub harness: HarnessConfig,
    /// Show a once-a-day "update available" hint (`rc` TUI and headless). Never contacts
    /// anything but the release site; `RUCKCODE_NO_UPDATE_CHECK=1` also disables it.
    pub check_updates: bool,
    pub update: UpdateConfig,
    pub mise: MiseConfig,
}

impl Deref for Config {
    type Target = HarnessConfig;
    fn deref(&self) -> &HarnessConfig {
        &self.harness
    }
}

impl DerefMut for Config {
    fn deref_mut(&mut self) -> &mut HarnessConfig {
        &mut self.harness
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::from_toml_str("").expect("built-in defaults parse")
    }
}

/// Environment variable overriding the user config directory (default `~/.config/ruckcode`).
pub const CONFIG_DIR_ENV: &str = RUCKCODE.config_dir_env;

/// The user config directory: the explicit override, else `$RUCKCODE_CONFIG_DIR`, else
/// `~/.config/ruckcode`.
pub fn user_config_dir(over: Option<&Path>) -> Option<PathBuf> {
    harness::user_config_dir(&RUCKCODE, over)
}

/// Path of the credentials file (`credentials.toml`) in the user config directory.
pub fn credentials_path(user_dir: Option<&Path>) -> Option<PathBuf> {
    harness::credentials_path(&RUCKCODE, user_dir)
}

/// The secret of named key `name` in `credentials.toml` (`[keys.<name>] api_key`).
pub fn named_key(user_dir: Option<&Path>, name: &str) -> Option<String> {
    harness::named_key(&RUCKCODE, user_dir, name)
}

/// Resolve a profile's API key: the named key (`api_key_name`, an error if it is missing from
/// `credentials.toml`), else (legacy, hand-written configs only) the env var named by
/// `api_key_env`, else none.
pub fn resolve_api_key(profile: &Profile, user_dir: Option<&Path>) -> Result<Option<String>> {
    harness::resolve_api_key(&RUCKCODE, profile, user_dir)
}

/// The `[mise]` section of a merged config table (the product's half of [`Config::from_table`]).
pub(crate) fn mise_from_table(table: &toml::Table) -> Result<MiseConfig> {
    let p: ProductSections = toml::Value::Table(table.clone())
        .try_into()
        .context("invalid configuration")?;
    Ok(p.mise)
}

impl Config {
    /// Deserialise a merged config table (see [`ConfigLoader::load_table`]): the harness sections,
    /// then the product's, then validation.
    pub fn from_table(table: toml::Table) -> Result<Self> {
        let harness = HarnessConfig::deserialize_table(&table)?;
        let p: ProductSections = toml::Value::Table(table)
            .try_into()
            .context("invalid configuration")?;
        harness.validate()?;
        Ok(Self {
            harness,
            check_updates: p.check_updates,
            update: p.update,
            mise: p.mise,
        })
    }

    /// Parse a single TOML document on top of the built-in defaults.
    pub fn from_toml_str(text: &str) -> Result<Self> {
        Self::from_table(ConfigLoader::table_from_str(text)?)
    }

    /// Load user then project config; `RUCKCODE_PROFILE` overrides `default_profile`.
    pub fn load(project_dir: &Path, user_dir: Option<&Path>) -> Result<Self> {
        Self::from_table(ConfigLoader::new(RUCKCODE).load_table(project_dir, user_dir)?)
    }

    /// The `[web]` settings with named search keys (`api_key_name`) read from `credentials.toml`.
    pub fn web_resolved(&self, user_dir: Option<&Path>) -> ruck_harness_web::WebConfig {
        self.harness.web_resolved(&RUCKCODE, user_dir)
    }
}

/// Instantiate the provider client for a profile, resolving keys without a config dir override.
pub fn make_provider(profile: &Profile) -> Result<Arc<dyn Provider>> {
    make_provider_with(profile, None)
}

/// Instantiate the provider client for a profile. The API key comes from the profile's named key
/// in the per-user `credentials.toml` (legacy `api_key_env` is still honoured), else none (only
/// local/LAN OpenAI-compatible endpoints may run without a key).
pub fn make_provider_with(profile: &Profile, user_dir: Option<&Path>) -> Result<Arc<dyn Provider>> {
    let key = resolve_api_key(profile, user_dir)?;
    let missing = || {
        anyhow::anyhow!(
            "no API key for profile `{}`: run `rc setup` (or `rc keys add <name>` then `rc keys use <name> --profile {}`)",
            profile.name,
            profile.name
        )
    };
    Ok(match profile.provider {
        ProviderKind::Mock => Arc::new(
            ruck_harness_llm::mock::MockProvider::demo().with_identity(crate::identity::RUCKCODE),
        ),
        ProviderKind::Anthropic => {
            let key = key.ok_or_else(missing)?;
            Arc::new(ruck_harness_llm::anthropic::Anthropic::new(
                key,
                profile.base_url.clone(),
                &crate::identity::RUCKCODE,
            ))
        }
        ProviderKind::Openai => {
            let local = profile
                .base_url
                .as_deref()
                .is_some_and(crate::setup::is_local_endpoint);
            if key.is_none() && !local {
                return Err(missing());
            }
            let mut p = ruck_harness_llm::openai::OpenAi::new(
                key,
                profile.base_url.clone(),
                &crate::identity::RUCKCODE,
            );
            if let Some(extra) = profile.ollama.as_ref().and_then(OllamaOptions::extra_body) {
                p = p.with_extra_body(extra);
            }
            Arc::new(p)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::Permission;

    fn cfg(text: &str) -> Result<Config> {
        Config::from_toml_str(text)
    }

    #[test]
    fn thinking_guard_and_ollama_options_parse() {
        let c = cfg(r#"
default_profile = "local"
[thinking_guard]
repeat_count = 6
max_nudges = 3
[profiles.local]
provider = "openai"
model = "gemma4:26b"
[profiles.local.thinking_guard]
budget_tokens = 8000
[profiles.local.ollama]
repeat_penalty = 1.15
[profiles.cloud]
provider = "openai"
model = "gpt"
"#)
        .unwrap();
        assert_eq!(c.thinking_guard.repeat_count, 6);
        assert_eq!(c.thinking_guard.window_chars, 4000);
        let local = c.profile("local").unwrap();
        let g = c
            .thinking_guard
            .with_override(local.thinking_guard.as_ref());
        assert_eq!((g.budget_tokens, g.max_nudges), (Some(8000), 3));
        assert!(local.ollama.as_ref().unwrap().extra_body().is_some());
        // Other profiles get no Ollama options and the global guard settings.
        let cloud = c.profile("cloud").unwrap();
        assert!(cloud.ollama.is_none() && cloud.thinking_guard.is_none());
    }

    #[test]
    fn tool_loop_guard_parses_with_profile_override() {
        let c = cfg(r#"
default_profile = "local"
[tool_loop_guard]
repeat_count = 6
window = 12
search_repeat_count = 9
[profiles.local]
provider = "openai"
model = "gemma4:26b"
[profiles.local.tool_loop_guard]
max_nudges = 1
enabled = false
[profiles.cloud]
provider = "openai"
model = "gpt"
"#)
        .unwrap();
        assert_eq!(c.tool_loop_guard.repeat_count, 6);
        assert_eq!(c.tool_loop_guard.plan_noop_count, 3);
        assert_eq!(c.tool_loop_guard.search_repeat_count, 9);
        assert_eq!(c.tool_loop_guard.oscillation_count, 2);
        assert!(c.tool_loop_guard.search_repeat_enabled && c.tool_loop_guard.oscillation_enabled);
        let local = c.profile("local").unwrap();
        let g = c
            .tool_loop_guard
            .with_override(local.tool_loop_guard.as_ref());
        assert_eq!((g.enabled, g.max_nudges, g.window), (false, 1, 12));
        let cloud = c.profile("cloud").unwrap();
        assert!(cloud.tool_loop_guard.is_none());
        assert!(
            c.tool_loop_guard
                .with_override(cloud.tool_loop_guard.as_ref())
                .enabled
        );
    }

    #[test]
    fn lean_tools_and_tool_allowlist_filter_builtins() {
        let c = cfg(r#"
[profiles.lean]
provider = "openai"
model = "m"
lean_tools = true
[profiles.pick]
provider = "openai"
model = "m"
tools = ["bash", "Read"]
[profiles.plain]
provider = "openai"
model = "m"
"#)
        .unwrap();
        let (lean, pick, plain) = (
            &c.profiles["lean"],
            &c.profiles["pick"],
            &c.profiles["plain"],
        );
        for t in ["Bash", "Read", "Edit", "TodoWrite", "AskUserQuestion"] {
            assert!(plain.allows_builtin(t), "{t}");
        }
        assert!(lean.allows_builtin("Bash") && lean.allows_builtin("Edit"));
        assert!(!lean.allows_builtin("TodoWrite") && !lean.allows_builtin("AskUserQuestion"));
        assert!(pick.allows_builtin("Bash") && pick.allows_builtin("Read"));
        assert!(!pick.allows_builtin("Edit") && !pick.allows_builtin("TodoWrite"));
    }

    #[test]
    fn web_tools_follow_lean_tools_and_the_allowlist() {
        let c = cfg(r#"
[profiles.lean]
provider = "openai"
model = "m"
lean_tools = true
[profiles.lean_web]
provider = "openai"
model = "m"
lean_tools = true
tools = ["Read", "WebSearch"]
[profiles.pick]
provider = "openai"
model = "m"
tools = ["Read", "webfetch"]
[profiles.plain]
provider = "openai"
model = "m"
"#)
        .unwrap();
        let p = &c.profiles;
        for t in ["WebFetch", "WebSearch"] {
            assert!(p["plain"].allows_builtin(t), "{t}");
            assert!(!p["lean"].allows_builtin(t), "{t}");
        }
        assert!(
            !p["lean_web"].allows_builtin("WebFetch") && p["lean_web"].allows_builtin("WebSearch")
        );
        assert!(p["pick"].allows_builtin("WebFetch") && !p["pick"].allows_builtin("WebSearch"));
    }

    #[test]
    fn web_config_parses_and_projects_cannot_widen_it() {
        let c = cfg(r#"
[web]
allow_private = true
[web.search]
provider = "searxng"
fallback = ["duckduckgo"]
url = "https://searx.example"
[web.search.brave]
api_key = "sekrit"
api_key_env = "MY_BRAVE"
"#)
        .unwrap();
        assert!(c.web.enabled && c.web.allow_private);
        assert_eq!(c.web.search.provider, "searxng");
        assert_eq!(
            c.web.search.conf("searxng").url.as_deref(),
            Some("https://searx.example")
        );
        assert_eq!(
            c.web.search.conf("brave").api_key_env.as_deref(),
            Some("MY_BRAVE")
        );
        assert!(
            !format!("{:?}", c.web).contains("sekrit"),
            "keys are redacted in Debug"
        );
        assert_eq!(Config::default().web.search.provider, "duckduckgo");
        assert!(!cfg("[web]\nenabled = false\n").unwrap().web.enabled);
        assert!(cfg("[web]\nbogus = 1\n").is_err());
        // A project's config.toml keeps only `web.enabled`.
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(
            proj.path().join(".ruckcode/config.toml"),
            "[web]\nenabled = false\nallow_private = true\n[web.search]\nurl = \"http://evil.test\"\n",
        )
        .unwrap();
        let c = Config::load(proj.path(), Some(user.path())).unwrap();
        assert!(!c.web.enabled && !c.web.allow_private && c.web.search.url.is_none());
    }

    #[test]
    fn named_search_keys_come_from_credentials() {
        let user = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("credentials.toml"),
            "[keys.brv]\napi_key = \"from-creds\"\n",
        )
        .unwrap();
        let c = cfg("[web.search]\nprovider = \"brave\"\napi_key_name = \"brv\"\n").unwrap();
        let w = c.web_resolved(Some(user.path()));
        assert_eq!(
            w.search.conf("brave").api_key.unwrap().expose(),
            "from-creds"
        );
    }

    #[test]
    fn sampling_parses_and_validates() {
        let base = "[profiles.local]\nprovider = \"openai\"\nmodel = \"m\"\n";
        let c = cfg(&format!(
            "{base}temperature = 0.5\ntop_p = 0.95\n[profiles.local.ollama]\nrepeat_penalty = 1.1\n"
        ))
        .unwrap();
        let p = &c.profiles["local"];
        assert_eq!((p.temperature, p.top_p), (Some(0.5), Some(0.95)));
        assert!(p.ollama.is_some());
        let plain = cfg(base).unwrap();
        assert_eq!(plain.profiles["local"].temperature, None);
        for (k, v) in [
            ("temperature", "2.5"),
            ("temperature", "-0.1"),
            ("top_p", "1.5"),
            ("top_p", "-1"),
        ] {
            let e = cfg(&format!("{base}{k} = {v}\n")).unwrap_err().to_string();
            assert!(e.contains(k) && e.contains("between"), "{e}");
        }
        assert!(cfg(&format!("{base}temperature = 2.0\ntop_p = 0.0\n")).is_ok());
    }

    #[test]
    fn pricing_parses_and_costs_each_bucket() {
        let c = cfg(r#"
[profiles.anthropic.pricing]
input = 3.0
output = 15.0
cache_read = 0.3
[profiles.anthropic.model_pricing."big"]
input = 15.0
output = 75.0
"#)
        .unwrap();
        let p = &c.profiles["anthropic"];
        let u = ruck_harness_llm::Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_tokens: Some(400_000),
            cache_write_tokens: Some(100_000),
        };
        // 500k uncached * 3 + 400k read * 0.3 + 100k write * (input rate 3) + 100k out * 15
        let usd = p.pricing_for("claude-opus-5-5").unwrap().cost(&u).unwrap();
        assert!((usd - (1.5 + 0.12 + 0.3 + 1.5)).abs() < 1e-9, "{usd}");
        // The per-model table replaces the profile prices.
        assert_eq!(p.pricing_for("big").unwrap().input, 15.0);
        // No usage reported: no cost.
        assert_eq!(
            p.pricing_for("x")
                .unwrap()
                .cost(&ruck_harness_llm::Usage::default()),
            None
        );
        // Unpriced profile.
        assert!(c.profiles["openai"].pricing_for("gpt-5").is_none());
    }

    #[test]
    fn negative_pricing_is_rejected() {
        let e = cfg("[profiles.anthropic.pricing]\ninput = -1.0\noutput = 1.0\n").unwrap_err();
        assert!(e.to_string().contains("pricing"), "{e}");
    }

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(c.default_profile, "anthropic");
        assert_eq!(c.profiles["anthropic"].model, "claude-opus-5-5");
        assert_eq!(c.profiles["openai"].provider, ProviderKind::Openai);
        assert_eq!(c.max_turn_requests, 100);
        assert!(c.check_updates);
        assert!(
            !Config::from_toml_str("check_updates = false")
                .unwrap()
                .check_updates
        );
        assert!(c.update.auto && c.update.check);
        let u = Config::from_toml_str("[update]\nauto = false")
            .unwrap()
            .update;
        assert!(!u.auto && u.check);
        assert_eq!(c.mise.lazy_threshold, 12);
        assert_eq!(c.mcp.lazy_threshold, 20);
        assert!(c.mcp_servers.is_empty());
    }

    #[test]
    fn project_overrides_user_per_key() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("config.toml"),
            r#"
            max_turn_requests = 10
            [profiles.local]
            provider = "mock"
            model = "m"
            [permissions]
            Bash = "ask"
            Write = "deny"
            "#,
        )
        .unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(
            proj.path().join(".ruckcode/config.toml"),
            r#"
            default_profile = "local"
            [profiles.anthropic]
            model = "other"
            thinking = { type = "enabled", budget_tokens = 8000 }
            [permissions]
            Bash = "allow"
            "#,
        )
        .unwrap();
        let c = Config::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(c.default_profile, "local");
        assert_eq!(c.max_turn_requests, 10);
        assert_eq!(c.profiles["local"].provider, ProviderKind::Mock);
        assert_eq!(c.profiles["anthropic"].model, "other");
        assert_eq!(c.profiles["anthropic"].max_tokens, 32000);
        assert_eq!(
            c.profiles["anthropic"].thinking.as_ref().unwrap()["budget_tokens"],
            8000
        );
        assert_eq!(c.permissions["Bash"], Permission::Allow);
        assert_eq!(c.permissions["Write"], Permission::Deny);
    }

    #[test]
    fn sandbox_enabled_only_from_user_config() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(
            proj.path().join(".ruckcode/config.toml"),
            "[sandbox]\nenabled = false\nnetwork = \"off\"\nextra_write_paths = [\"/x\"]\n",
        )
        .unwrap();
        let c = Config::load(proj.path(), Some(user.path())).unwrap();
        assert!(
            c.sandbox.enabled,
            "project config must not disable the sandbox"
        );
        // The working-tree [sandbox] table is not read at all (only the verified remote copy is
        // honoured, see `project_policy`).
        assert_eq!(
            c.sandbox.network,
            ruck_harness_sandbox::NetworkMode::Allowlist
        );
        assert!(c.sandbox.extra_write_paths.is_empty());
        std::fs::write(
            user.path().join("config.toml"),
            "[sandbox]\nenabled = false\n",
        )
        .unwrap();
        let c = Config::load(proj.path(), Some(user.path())).unwrap();
        assert!(!c.sandbox.enabled);
        assert!(Config::default().sandbox.enabled);
    }

    #[test]
    fn unknown_default_profile_is_error() {
        assert!(Config::from_toml_str("default_profile = \"nope\"").is_err());
    }

    #[test]
    fn mock_provider() {
        let c = Config::from_toml_str("[profiles.m]\nprovider='mock'\nmodel='x'").unwrap();
        assert_eq!(make_provider(&c.profiles["m"]).unwrap().name(), "mock");
    }

    #[test]
    fn mcp_servers_merge_by_name() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("config.toml"),
            r#"
            [mcp]
            lazy_threshold = 5
            [mcp_servers.u]
            command = "user-cmd"
            [mcp_servers.both]
            command = "user-both"
            args = ["x"]
            [mcp_servers.clash]
            command = "user-clash"
            "#,
        )
        .unwrap();
        std::fs::write(
            proj.path().join(".mcp.json"),
            r#"{"mcpServers": {
                "both": {"type": "http", "url": "http://json/mcp", "headers": {"A": "b"}},
                "clash": {"command": "json-clash", "env": {"K": "v"}},
                "j": {"command": "json-cmd", "args": ["1"]}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(
            proj.path().join(".ruckcode/config.toml"),
            r#"
            [mcp_servers.clash]
            command = "proj-clash"
            [mcp_servers.p]
            url = "http://p/mcp"
            "#,
        )
        .unwrap();
        let c = Config::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(c.mcp.lazy_threshold, 5);
        let names: Vec<_> = c.mcp_servers.keys().cloned().collect();
        assert_eq!(names, ["both", "clash", "j", "p", "u"]);
        // .mcp.json overrides user config wholesale (no field mixing).
        assert_eq!(
            c.mcp_servers["both"].url.as_deref(),
            Some("http://json/mcp")
        );
        assert_eq!(c.mcp_servers["both"].command, None);
        assert_eq!(c.mcp_servers["both"].headers["A"], "b");
        // Project config.toml wins over .mcp.json and user config.
        assert_eq!(
            c.mcp_servers["clash"].command.as_deref(),
            Some("proj-clash")
        );
        assert!(c.mcp_servers["clash"].env.is_empty());
        assert_eq!(c.mcp_servers["j"].args, ["1"]);
        assert_eq!(c.mcp_servers["u"].command.as_deref(), Some("user-cmd"));
    }

    #[test]
    fn bad_mcp_json_is_error() {
        let proj = tempfile::tempdir().unwrap();
        std::fs::write(proj.path().join(".mcp.json"), "{nope").unwrap();
        assert!(Config::load(proj.path(), None).is_err());
    }
}
