//! Layered configuration: built-in defaults, user config, then project config.

use anyhow::{bail, Context, Result};
use ruckcode_hooks::HooksConfig;
use ruckcode_llm::Provider;
use ruckcode_mcp::McpServerConfig;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// Built-in tools that `lean_tools` leaves out.
pub const LEAN_DROPPED_TOOLS: [&str; 2] = ["TodoWrite", "AskUserQuestion"];

impl Profile {
    /// Whether the built-in tool `name` is offered with this profile.
    pub fn allows_builtin(&self, name: &str) -> bool {
        if self.lean_tools && LEAN_DROPPED_TOOLS.contains(&name) {
            return false;
        }
        self.tools
            .as_ref()
            .is_none_or(|l| l.iter().any(|n| n.eq_ignore_ascii_case(name)))
    }

    /// The prices for `model`, if any are configured.
    pub fn pricing_for(&self, model: &str) -> Option<&Pricing> {
        self.model_pricing.get(model).or(self.pricing.as_ref())
    }
}

/// USD per million tokens. `input` is the rate for uncached prompt tokens. `cache_read` and
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
    pub fn cost(&self, u: &ruckcode_llm::Usage) -> Option<f64> {
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

fn default_mcp_lazy_threshold() -> usize {
    ruckcode_mcp::DEFAULT_LAZY_THRESHOLD
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

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Config {
    pub default_profile: String,
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: f64,
    #[serde(default = "default_max_turn_requests")]
    pub max_turn_requests: u32,
    /// Show a once-a-day "update available" hint (`rc` TUI and headless). Never contacts
    /// anything but the release site; `RUCKCODE_NO_UPDATE_CHECK=1` also disables it.
    #[serde(default = "default_true")]
    pub check_updates: bool,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub update: UpdateConfig,
    #[serde(default)]
    pub mise: MiseConfig,
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
    pub sandbox: ruckcode_sandbox::SandboxConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_toml_str("").expect("built-in defaults parse")
    }
}

/// Environment variable overriding the user config directory (default `~/.config/ruckcode`).
pub const CONFIG_DIR_ENV: &str = "RUCKCODE_CONFIG_DIR";

/// The user config directory: the explicit override, else `$RUCKCODE_CONFIG_DIR`, else
/// `~/.config/ruckcode`.
pub fn user_config_dir(over: Option<&Path>) -> Option<PathBuf> {
    match over {
        Some(p) => Some(p.to_path_buf()),
        None => match std::env::var_os(CONFIG_DIR_ENV).filter(|v| !v.is_empty()) {
            Some(d) => Some(PathBuf::from(d)),
            None => dirs::config_dir().map(|d| d.join("ruckcode")),
        },
    }
}

/// Path of the credentials file (`credentials.toml`) in the user config directory.
pub fn credentials_path(user_dir: Option<&Path>) -> Option<PathBuf> {
    user_config_dir(user_dir).map(|d| d.join("credentials.toml"))
}

/// The secret of named key `name` in `credentials.toml` (`[keys.<name>] api_key`).
pub fn named_key(user_dir: Option<&Path>, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(credentials_path(user_dir)?).ok()?;
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
pub fn resolve_api_key(profile: &Profile, user_dir: Option<&Path>) -> Result<Option<String>> {
    if let Some(name) = profile.api_key_name.as_deref() {
        return match named_key(user_dir, name) {
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
    let servers = ruckcode_mcp::parse_mcp_json(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    let mut t = toml::Table::new();
    t.insert(
        "mcp_servers".into(),
        toml::Value::try_from(servers).context("converting .mcp.json servers")?,
    );
    Ok(Some(t))
}

fn from_tables(layers: Vec<toml::Table>) -> Result<Config> {
    let mut merged = parse_table(DEFAULTS, "built-in defaults")?;
    for mut l in layers {
        merge_mcp_servers(&mut merged, &mut l);
        merge(&mut merged, l);
    }
    let mut cfg: Config = toml::Value::Table(merged)
        .try_into()
        .context("invalid configuration")?;
    for (name, p) in cfg.profiles.iter_mut() {
        p.name = name.clone();
    }
    for p in cfg.profiles.values() {
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
    for p in cfg.profiles.values() {
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
    if !cfg.profiles.contains_key(&cfg.default_profile) {
        bail!("default_profile `{}` is not defined", cfg.default_profile);
    }
    Ok(cfg)
}

impl Config {
    /// Parse a single TOML document on top of the built-in defaults.
    pub fn from_toml_str(text: &str) -> Result<Self> {
        from_tables(vec![parse_table(text, "config")?])
    }

    /// Load user then project config; `RUCKCODE_PROFILE` overrides `default_profile`.
    pub fn load(project_dir: &Path, user_dir: Option<&Path>) -> Result<Self> {
        let mut layers = Vec::new();
        let read = |path: PathBuf| -> Result<Option<toml::Table>> {
            match std::fs::read_to_string(&path) {
                Ok(text) => Ok(Some(parse_table(&text, &path.display().to_string())?)),
                Err(_) => Ok(None),
            }
        };
        if let Some(p) = user_config_dir(user_dir).map(|d| d.join("config.toml")) {
            layers.extend(read(p)?);
        }
        // `.mcp.json` sits between user config and project config.toml.
        layers.extend(mcp_json_layer(project_dir)?);
        if let Some(mut t) = read(project_dir.join(".ruckcode/config.toml"))? {
            // A project is writable by sandboxed commands, so nothing in its working tree may
            // shape the sandbox: project `[sandbox]` settings are only honoured as committed on
            // the remote (see `project_policy`), never from this file.
            if t.remove("sandbox").is_some() {
                tracing::debug!(
                    "ignoring [sandbox] in the project config.toml (only the verified remote copy is honoured)"
                );
            }
            layers.push(t);
        }
        if let Ok(p) = std::env::var("RUCKCODE_PROFILE") {
            if !p.is_empty() {
                let mut t = toml::Table::new();
                t.insert("default_profile".into(), toml::Value::String(p));
                layers.push(t);
            }
        }
        from_tables(layers)
    }

    pub fn profile(&self, name: &str) -> Result<&Profile> {
        self.profiles
            .get(name)
            .with_context(|| format!("unknown profile `{name}`"))
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
        ProviderKind::Mock => Arc::new(ruckcode_llm::mock::MockProvider::demo()),
        ProviderKind::Anthropic => {
            let key = key.ok_or_else(missing)?;
            Arc::new(ruckcode_llm::anthropic::Anthropic::new(
                key,
                profile.base_url.clone(),
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
            let mut p = ruckcode_llm::openai::OpenAi::new(key, profile.base_url.clone());
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

    fn cfg(text: &str) -> Result<Config> {
        from_tables(vec![parse_table(text, "test")?])
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
        let u = ruckcode_llm::Usage {
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
                .cost(&ruckcode_llm::Usage::default()),
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
        assert_eq!(c.sandbox.network, ruckcode_sandbox::NetworkMode::Allowlist);
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
