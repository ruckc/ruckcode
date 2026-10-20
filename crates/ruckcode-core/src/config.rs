//! Layered configuration: built-in defaults, user config, then project config.

use anyhow::{bail, Context, Result};
use ruckcode_hooks::HooksConfig;
use ruckcode_llm::Provider;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::permissions::Permission;

const DEFAULTS: &str = r#"
default_profile = "anthropic"

[profiles.anthropic]
provider = "anthropic"
model = "claude-opus-5-5"
api_key_env = "ANTHROPIC_API_KEY"
max_tokens = 32000
context_window = 200000

[profiles.openai]
provider = "openai"
model = "gpt-5"
api_key_env = "OPENAI_API_KEY"
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    Openai,
    Mock,
}

fn default_max_tokens() -> u32 {
    32_000
}

fn default_context_window() -> u64 {
    200_000
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Profile {
    pub provider: ProviderKind,
    pub model: String,
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
}

fn default_lazy_threshold() -> usize {
    12
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

fn default_compaction_threshold() -> f64 {
    0.8
}

fn default_max_turn_requests() -> u32 {
    100
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Config {
    pub default_profile: String,
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: f64,
    #[serde(default = "default_max_turn_requests")]
    pub max_turn_requests: u32,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub mise: MiseConfig,
    /// Tool name or glob (`mise__*`) to permission.
    #[serde(default)]
    pub permissions: BTreeMap<String, Permission>,
    #[serde(default)]
    pub hooks: HooksConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_toml_str("").expect("built-in defaults parse")
    }
}

/// `~/.config/ruckcode` unless overridden.
pub fn user_config_dir(over: Option<&Path>) -> Option<PathBuf> {
    match over {
        Some(p) => Some(p.to_path_buf()),
        None => dirs::config_dir().map(|d| d.join("ruckcode")),
    }
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

fn from_tables(layers: Vec<toml::Table>) -> Result<Config> {
    let mut merged = parse_table(DEFAULTS, "built-in defaults")?;
    for l in layers {
        merge(&mut merged, l);
    }
    let cfg: Config = toml::Value::Table(merged)
        .try_into()
        .context("invalid configuration")?;
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
        let files = [
            user_config_dir(user_dir).map(|d| d.join("config.toml")),
            Some(project_dir.join(".ruckcode/config.toml")),
        ];
        for path in files.into_iter().flatten() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                layers.push(parse_table(&text, &path.display().to_string())?);
            }
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

/// Instantiate the provider client for a profile.
pub fn make_provider(profile: &Profile) -> Result<Arc<dyn Provider>> {
    let key = || {
        profile
            .api_key_env
            .as_deref()
            .map(|var| std::env::var(var).with_context(|| format!("{var} is not set")))
    };
    Ok(match profile.provider {
        ProviderKind::Mock => Arc::new(ruckcode_llm::mock::MockProvider::default()),
        ProviderKind::Anthropic => {
            let key = key().context("anthropic profile needs api_key_env")??;
            Arc::new(ruckcode_llm::anthropic::Anthropic::new(
                key,
                profile.base_url.clone(),
            ))
        }
        ProviderKind::Openai => Arc::new(ruckcode_llm::openai::OpenAi::new(
            key().transpose()?,
            profile.base_url.clone(),
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(c.default_profile, "anthropic");
        assert_eq!(c.profiles["anthropic"].model, "claude-opus-5-5");
        assert_eq!(c.profiles["openai"].provider, ProviderKind::Openai);
        assert_eq!(c.max_turn_requests, 100);
        assert_eq!(c.mise.lazy_threshold, 12);
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
    fn unknown_default_profile_is_error() {
        assert!(Config::from_toml_str("default_profile = \"nope\"").is_err());
    }

    #[test]
    fn mock_provider() {
        let c = Config::from_toml_str("[profiles.m]\nprovider='mock'\nmodel='x'").unwrap();
        assert_eq!(make_provider(&c.profiles["m"]).unwrap().name(), "mock");
    }
}
