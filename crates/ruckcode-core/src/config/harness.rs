//! The generic half of the configuration: everything the harness reads from `config.toml` that
//! is not specific to the ruckcode product, and the loader that layers and secures the files.
//!
//! This module must not import anything product-specific (no `ruckcode_tools`, no
//! `crate::identity::RUCKCODE`, no `crate::setup`): the product passes its [`AppIdentity`] in.
//! That keeps it movable into the harness core crate as one unit.

use anyhow::{bail, Context, Result};
use ruck_harness_hooks::HooksConfig;
use ruck_harness_ident::AppIdentity;
use ruck_harness_mcp::McpServerConfig;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::context::{ContextConfig, ContextOverride};
use crate::permissions::Permission;
use crate::retry::RateLimitConfig;
use crate::thinking_guard::{OllamaOptions, ThinkingGuardConfig, ThinkingGuardOverride};
use crate::tool_loop_guard::{ToolLoopGuardConfig, ToolLoopGuardOverride};

const DEFAULTS: &str = r#"
default_profile = "anthropic"

[profiles.anthropic]
provider = "anthropic"
model = "claude-opus-5-5"
max_tokens = 32000
context_window = 200000

[profiles.openai]
provider = "openai"
model = "gpt-5"
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    Openai,
    Mock,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Openai => "openai",
            ProviderKind::Mock => "mock",
        }
    }
}

fn default_max_tokens() -> u32 {
    32_000
}

fn default_context_window() -> u64 {
    200_000
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Profile {
    /// The profile's key in `[profiles.<name>]`; filled in when the config is loaded.
    #[serde(skip)]
    pub name: String,
    pub provider: ProviderKind,
    pub model: String,
    /// Models offered by `/model` and the setup wizard (the default `model` need not be listed).
    #[serde(default)]
    pub models: Vec<String>,
    /// Reference to a named key in `credentials.toml` (`[keys.<name>]`). Not a secret.
    #[serde(default)]
    pub api_key_name: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_context_window")]
    pub context_window: u64,
    /// Passed through as `Request.thinking`.
    #[serde(default)]
    pub thinking: Option<Value>,
    /// Prices of the profile's models; when set, `usage_update` carries a running `cost`.
    #[serde(default)]
    pub pricing: Option<Pricing>,
    /// Per-model prices (`[profiles.<name>.model_pricing."<model>"]`) that replace `pricing` for
    /// that model, for profiles whose `models` differ in price.
    #[serde(default)]
    pub model_pricing: BTreeMap<String, Pricing>,
    /// Overrides of the global `[thinking_guard]` for this profile.
    #[serde(default)]
    pub thinking_guard: Option<ThinkingGuardOverride>,
    /// Overrides of the global `[context]` for this profile.
    #[serde(default)]
    pub context: Option<ContextOverride>,
    /// Overrides of the global `[tool_loop_guard]` for this profile.
    #[serde(default)]
    pub tool_loop_guard: Option<ToolLoopGuardOverride>,
    /// Ollama sampling options (`[profiles.<name>.ollama]`); sent only when this section is set.
    #[serde(default)]
    pub ollama: Option<OllamaOptions>,
    /// Sampling temperature (0.0 to 2.0); sent only when set. Not sent to Anthropic while
    /// extended thinking is enabled.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Nucleus sampling `top_p` (0.0 to 1.0); sent only when set.
    #[serde(default)]
    pub top_p: Option<f64>,
    /// Allowlist of built-in tool names offered to the model with this profile (MCP and mise
    /// tools are unaffected). Unset offers every built-in tool the mode allows.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Lean tool set for small local models: leave out the built-in tools whose nested
    /// array-of-object arguments some servers fail to parse (`TodoWrite`, `AskUserQuestion`), and
    /// the prompt lines about them. Off by default.
    #[serde(default)]
    pub lean_tools: bool,
}

impl Profile {
    /// Whether `tool` is offered with this profile. A tool's
    /// [`LeanClass`](ruck_harness_tools::LeanClass) says how `lean_tools` treats it.
    pub fn allows_tool(&self, tool: &dyn ruck_harness_tools::Tool) -> bool {
        self.allows_class(tool.name(), tool.lean_class())
    }

    pub(crate) fn allows_class(&self, name: &str, class: ruck_harness_tools::LeanClass) -> bool {
        use ruck_harness_tools::LeanClass;
        if self.lean_tools && class == LeanClass::DropWhenLean {
            return false;
        }
        let listed = self
            .tools
            .as_ref()
            .map(|l| l.iter().any(|n| n.eq_ignore_ascii_case(name)));
        if self.lean_tools && class == LeanClass::OptInWhenLean {
            return listed == Some(true);
        }
        listed.unwrap_or(true)
    }

    /// The prices for `model`, if any are configured.
    pub fn pricing_for(&self, model: &str) -> Option<&Pricing> {
        self.model_pricing.get(model).or(self.pricing.as_ref())
    }
}

/// `cache_write` default to the `input` rate when omitted, so cached tokens are never free by
/// accident.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

impl Pricing {
    /// Cost in USD of one request's usage; `None` when the provider reported no usage.
    pub fn cost(&self, u: &ruck_harness_llm::Usage) -> Option<f64> {
        if !u.is_reported() {
            return None;
        }
        let read = u.cache_read_tokens.unwrap_or(0) as f64;
        let write = u.cache_write_tokens.unwrap_or(0) as f64;
        let usd = u.uncached_input_tokens() as f64 * self.input
            + u.output_tokens as f64 * self.output
            + read * self.cache_read.unwrap_or(self.input)
            + write * self.cache_write.unwrap_or(self.input);
        Some(usd / 1_000_000.0)
    }

    fn valid(&self) -> bool {
        [
            Some(self.input),
            Some(self.output),
            self.cache_read,
            self.cache_write,
        ]
        .into_iter()
        .flatten()
        .all(|p| p.is_finite() && p >= 0.0)
    }
}

fn default_mcp_lazy_threshold() -> usize {
    ruck_harness_mcp::DEFAULT_LAZY_THRESHOLD
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct McpConfig {
    /// Above this many selected MCP tools in a mode, expose `McpListTools`/`McpCall` instead.
    #[serde(default = "default_mcp_lazy_threshold")]
    pub lazy_threshold: usize,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            lazy_threshold: default_mcp_lazy_threshold(),
        }
    }
}

fn default_compaction_threshold() -> f64 {
    0.8
}

fn default_max_turn_requests() -> u32 {
    100
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HarnessConfig {
    pub default_profile: String,
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: f64,
    #[serde(default = "default_max_turn_requests")]
    pub max_turn_requests: u32,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub mcp: McpConfig,
    /// MCP servers by name (`[mcp_servers.<name>]`), merged from user config, project
    /// `.mcp.json` and project config.
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    /// Tool name or glob (`mise__*`) to permission.
    #[serde(default)]
    pub permissions: BTreeMap<String, Permission>,
    #[serde(default)]
    pub hooks: HooksConfig,
    /// Retry and wait behaviour for 429 / overloaded / transient API failures (`[rate_limit]`).
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
    /// Detection of a model looping in its reasoning (`[thinking_guard]`).
    #[serde(default)]
    pub thinking_guard: ThinkingGuardConfig,
    /// Detection of repeated no-progress tool calls (`[tool_loop_guard]`).
    #[serde(default)]
    pub tool_loop_guard: ToolLoopGuardConfig,
    /// Context-window management: compaction, tool result caps, output reserve (`[context]`).
    #[serde(default)]
    pub context: ContextConfig,
    /// Command sandbox (`[sandbox]`). `enabled` is only honoured from the user's config.
    #[serde(default)]
    pub sandbox: ruck_harness_sandbox::SandboxConfig,
    /// The `WebFetch` and `WebSearch` tools (`[web]`). A project's own config may only set
    /// `enabled`.
    #[serde(default)]
    pub web: ruck_harness_web::WebConfig,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self::from_toml_str("").expect("built-in defaults parse")
    }
}

impl HarnessConfig {
    /// Parse a single TOML document on top of the built-in defaults.
    pub fn from_toml_str(text: &str) -> Result<Self> {
        Self::from_table(ConfigLoader::table_from_str(text)?)
    }

    /// Deserialise and validate a merged config table (see [`ConfigLoader::load_table`]).
    /// Keys the harness does not know (the product's own sections) are ignored.
    pub fn from_table(table: toml::Table) -> Result<Self> {
        let cfg = Self::deserialize_table(&table)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// The deserialisation half of [`from_table`](Self::from_table), without validation. A
    /// product that deserialises its own sections from the same table calls this first, then
    /// its own, then [`validate`](Self::validate), so a type error anywhere is reported before a
    /// validation error.
    pub fn deserialize_table(table: &toml::Table) -> Result<Self> {
        let mut cfg: Self = toml::Value::Table(table.clone())
            .try_into()
            .context("invalid configuration")?;
        for (name, p) in cfg.profiles.iter_mut() {
            p.name = name.clone();
        }
        Ok(cfg)
    }

    /// Check the value ranges and that the default profile exists.
    pub fn validate(&self) -> Result<()> {
        for p in self.profiles.values() {
            if p.pricing
                .iter()
                .chain(p.model_pricing.values())
                .any(|x| !x.valid())
            {
                bail!(
                    "profile `{}`: pricing rates must be non-negative numbers",
                    p.name
                );
            }
        }
        for p in self.profiles.values() {
            if let Some(t) = p.temperature {
                if !(0.0..=2.0).contains(&t) {
                    bail!(
                        "profile `{}`: temperature must be between 0.0 and 2.0 (got {t})",
                        p.name
                    );
                }
            }
            if let Some(t) = p.top_p {
                if !(0.0..=1.0).contains(&t) {
                    bail!(
                        "profile `{}`: top_p must be between 0.0 and 1.0 (got {t})",
                        p.name
                    );
                }
            }
        }
        if !self.profiles.contains_key(&self.default_profile) {
            bail!("default_profile `{}` is not defined", self.default_profile);
        }
        Ok(())
    }

    pub fn profile(&self, name: &str) -> Result<&Profile> {
        self.profiles
            .get(name)
            .with_context(|| format!("unknown profile `{name}`"))
    }

    /// The `[web]` settings with named search keys (`api_key_name`) read from `credentials.toml`
    /// in the identity's user config directory.
    pub fn web_resolved(
        &self,
        identity: &AppIdentity,
        user_dir: Option<&Path>,
    ) -> ruck_harness_web::WebConfig {
        let mut w = self.web.clone();
        w.search
            .resolve_named_keys(|n| named_key(identity, user_dir, n));
        w
    }
}

/// The user config directory of `identity`: the explicit override, else the identity's
/// `config_dir_env` variable, else `<platform config dir>/<user_config_subdir>`.
pub fn user_config_dir(identity: &AppIdentity, over: Option<&Path>) -> Option<PathBuf> {
    match over {
        Some(p) => Some(p.to_path_buf()),
        None => match std::env::var_os(identity.config_dir_env).filter(|v| !v.is_empty()) {
            Some(d) => Some(PathBuf::from(d)),
            None => dirs::config_dir().map(|d| d.join(identity.user_config_subdir)),
        },
    }
}

/// Path of the credentials file (`credentials.toml`) in the user config directory.
pub fn credentials_path(identity: &AppIdentity, user_dir: Option<&Path>) -> Option<PathBuf> {
    user_config_dir(identity, user_dir).map(|d| d.join("credentials.toml"))
}

/// The secret of named key `name` in `credentials.toml` (`[keys.<name>] api_key`).
pub fn named_key(identity: &AppIdentity, user_dir: Option<&Path>, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(credentials_path(identity, user_dir)?).ok()?;
    let t: toml::Table = text.parse().ok()?;
    t.get("keys")?
        .get(name)?
        .get("api_key")?
        .as_str()
        .filter(|k| !k.is_empty())
        .map(String::from)
}

/// Resolve a profile's API key: the named key (`api_key_name`, an error if it is missing from
/// `credentials.toml`), else (legacy, hand-written configs only) the env var named by
/// `api_key_env`, else none.
pub fn resolve_api_key(
    identity: &AppIdentity,
    profile: &Profile,
    user_dir: Option<&Path>,
) -> Result<Option<String>> {
    if let Some(name) = profile.api_key_name.as_deref() {
        return match named_key(identity, user_dir, name) {
            Some(k) => Ok(Some(k)),
            None => bail!(
                "named key `{name}` (profile `{}`) not found in credentials.toml: run `rc keys add {name}`",
                profile.name
            ),
        };
    }
    Ok(profile
        .api_key_env
        .as_deref()
        .and_then(|v| std::env::var(v).ok())
        .filter(|k| !k.is_empty()))
}

/// Recursively merge `over` into `base`: tables merge by key, everything else is replaced.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

fn parse_table(text: &str, what: &str) -> Result<toml::Table> {
    text.parse::<toml::Table>()
        .with_context(|| format!("parsing {what}"))
}

/// Merge `[mcp_servers]` entries by server name: a later definition replaces the whole entry.
fn merge_mcp_servers(base: &mut toml::Table, over: &mut toml::Table) {
    let Some(toml::Value::Table(servers)) = over.remove("mcp_servers") else {
        return;
    };
    let entry = base
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(t) = entry {
        for (name, v) in servers {
            t.insert(name, v);
        }
    }
}

/// Servers declared in `<project>/.mcp.json` (Claude Code format), as a config layer.
fn mcp_json_layer(project_dir: &Path) -> Result<Option<toml::Table>> {
    let path = project_dir.join(".mcp.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let servers = ruck_harness_mcp::parse_mcp_json(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    let mut t = toml::Table::new();
    t.insert(
        "mcp_servers".into(),
        toml::Value::try_from(servers).context("converting .mcp.json servers")?,
    );
    Ok(Some(t))
}

/// Layers the config files of a product and applies the security rules, producing one merged
/// table. Paths and environment variable names come from the product's [`AppIdentity`].
///
/// Layers, lowest first: the built-in defaults, the user's `config.toml`, the project's
/// `.mcp.json`, the project's `<project_dir>/config.toml`, then `<ENV_PREFIX>_PROFILE` as
/// `default_profile`. The table is not yet deserialised: [`HarnessConfig::from_table`] reads the
/// harness sections and the product reads its own from the same table.
#[derive(Debug, Clone, Copy)]
pub struct ConfigLoader {
    identity: AppIdentity,
}

impl ConfigLoader {
    pub fn new(identity: AppIdentity) -> Self {
        Self { identity }
    }

    /// The user config directory (see [`user_config_dir`]).
    pub fn user_config_dir(&self, over: Option<&Path>) -> Option<PathBuf> {
        user_config_dir(&self.identity, over)
    }

    /// The built-in defaults with one TOML document merged on top.
    pub fn table_from_str(text: &str) -> Result<toml::Table> {
        Self::merge_layers(vec![parse_table(text, "config")?])
    }

    /// The built-in defaults with `layers` merged on top, lowest first.
    pub fn merge_layers(layers: Vec<toml::Table>) -> Result<toml::Table> {
        let mut merged = parse_table(DEFAULTS, "built-in defaults")?;
        for mut l in layers {
            merge_mcp_servers(&mut merged, &mut l);
            merge(&mut merged, l);
        }
        Ok(merged)
    }

    /// Load and layer the user's and the project's config.
    ///
    /// Security rules: a project is writable by sandboxed commands, so its `[sandbox]` table is
    /// dropped (project sandbox policy is only honoured as committed on the remote, see
    /// `project_policy`), and its `[web]` table keeps only `enabled`. `sandbox.enabled` and
    /// `sandbox.project_policy` therefore only ever come from the user's config.
    pub fn load_table(&self, project_dir: &Path, user_dir: Option<&Path>) -> Result<toml::Table> {
        let mut layers = Vec::new();
        let read = |path: PathBuf| -> Result<Option<toml::Table>> {
            match std::fs::read_to_string(&path) {
                Ok(text) => Ok(Some(parse_table(&text, &path.display().to_string())?)),
                Err(_) => Ok(None),
            }
        };
        if let Some(p) = self
            .user_config_dir(user_dir)
            .map(|d| d.join("config.toml"))
        {
            layers.extend(read(p)?);
        }
        // `.mcp.json` sits between user config and project config.toml.
        layers.extend(mcp_json_layer(project_dir)?);
        if let Some(mut t) = read(self.identity.project_path(project_dir).join("config.toml"))? {
            if t.remove("sandbox").is_some() {
                tracing::debug!(
                    "ignoring [sandbox] in the project config.toml (only the verified remote copy is honoured)"
                );
            }
            // Likewise `[web]`: private-address access and search endpoints and keys are the
            // user's call. A project may only switch the web tools off (or on).
            if let Some(toml::Value::Table(w)) = t.get_mut("web") {
                w.retain(|k, _| k == "enabled");
            }
            layers.push(t);
        }
        if let Ok(p) = std::env::var(self.identity.env_var("PROFILE")) {
            if !p.is_empty() {
                let mut t = toml::Table::new();
                t.insert("default_profile".into(), toml::Value::String(p));
                layers.push(t);
            }
        }
        Self::merge_layers(layers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruck_harness_ident::testing::ACME;

    #[test]
    fn the_loader_takes_its_paths_and_env_names_from_the_identity() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::write(user.path().join("config.toml"), "max_turn_requests = 7\n").unwrap();
        std::fs::create_dir_all(proj.path().join(".acme")).unwrap();
        std::fs::write(
            proj.path().join(".acme/config.toml"),
            "compaction_threshold = 0.5\n[sandbox]\nenabled = false\n",
        )
        .unwrap();
        // A ruckcode-style project directory is not read for another identity.
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(
            proj.path().join(".ruckcode/config.toml"),
            "max_turn_requests = 999\n",
        )
        .unwrap();
        let t = ConfigLoader::new(ACME)
            .load_table(proj.path(), Some(user.path()))
            .unwrap();
        let c = HarnessConfig::from_table(t).unwrap();
        assert_eq!(c.max_turn_requests, 7);
        assert_eq!(c.compaction_threshold, 0.5);
        assert!(c.sandbox.enabled, "the project [sandbox] is dropped");
        assert_eq!(
            ConfigLoader::new(ACME).user_config_dir(Some(user.path())),
            Some(user.path().to_path_buf())
        );
    }

    #[test]
    fn product_sections_are_ignored_by_the_harness() {
        let t = ConfigLoader::table_from_str("check_updates = false\n[mise]\nx = 1\n").unwrap();
        assert!(HarnessConfig::from_table(t).is_ok());
    }
}
