//! `[web]` configuration.

use serde::Deserialize;

/// A secret that never appears in `Debug` output (and so never in logs).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// `[web]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebConfig {
    /// Turns `WebFetch` and `WebSearch` off (both) when false.
    pub enabled: bool,
    /// Let `WebFetch` reach private, loopback and link-local addresses (including cloud
    /// metadata services). Off by default; honoured from the user's config only.
    pub allow_private: bool,
    /// `[web.search]`.
    pub search: SearchConfig,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_private: false,
            search: SearchConfig::default(),
        }
    }
}

/// Settings of one search provider. `url` is the endpoint (required for `searxng`, an override
/// for the others). The key comes from `api_key`, else the named key `api_key_name` in
/// `credentials.toml`, else the environment variable `api_key_env` (default `BRAVE_API_KEY` /
/// `TAVILY_API_KEY`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderConf {
    pub url: Option<String>,
    pub api_key: Option<Secret>,
    pub api_key_env: Option<String>,
    pub api_key_name: Option<String>,
}

/// `[web.search]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    /// `duckduckgo` (default, keyless), `searxng`, `brave` or `tavily`.
    pub provider: String,
    /// Providers tried in order when `provider` fails. Empty: no other provider is ever used.
    pub fallback: Vec<String>,
    /// Minimum seconds between DuckDuckGo requests.
    pub ddg_min_interval_secs: f64,
    /// Settings for `provider` itself (shorthand for `[web.search.<provider>]`).
    pub url: Option<String>,
    pub api_key: Option<Secret>,
    pub api_key_env: Option<String>,
    pub api_key_name: Option<String>,
    pub duckduckgo: ProviderConf,
    pub searxng: ProviderConf,
    pub brave: ProviderConf,
    pub tavily: ProviderConf,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            provider: "duckduckgo".into(),
            fallback: vec![],
            ddg_min_interval_secs: 2.0,
            url: None,
            api_key: None,
            api_key_env: None,
            api_key_name: None,
            duckduckgo: ProviderConf::default(),
            searxng: ProviderConf::default(),
            brave: ProviderConf::default(),
            tavily: ProviderConf::default(),
        }
    }
}

pub const PROVIDERS: [&str; 4] = ["duckduckgo", "searxng", "brave", "tavily"];

impl SearchConfig {
    /// The effective settings of provider `name`: its own table, with the top-level shorthand
    /// filling the gaps when `name` is the selected `provider`.
    pub fn conf(&self, name: &str) -> ProviderConf {
        let mut c = match name {
            "duckduckgo" => self.duckduckgo.clone(),
            "searxng" => self.searxng.clone(),
            "brave" => self.brave.clone(),
            "tavily" => self.tavily.clone(),
            _ => ProviderConf::default(),
        };
        if name == self.provider {
            c.url = c.url.or_else(|| self.url.clone());
            c.api_key = c.api_key.or_else(|| self.api_key.clone());
            c.api_key_env = c.api_key_env.or_else(|| self.api_key_env.clone());
            c.api_key_name = c.api_key_name.or_else(|| self.api_key_name.clone());
        }
        c
    }

    /// Fill `api_key` from the named key (`credentials.toml`) for every provider that
    /// references one.
    pub fn resolve_named_keys(&mut self, lookup: impl Fn(&str) -> Option<String>) {
        let fill = |key: &mut Option<Secret>, name: &Option<String>| {
            if key.is_none() {
                if let Some(k) = name.as_deref().and_then(&lookup) {
                    *key = Some(Secret::new(k));
                }
            }
        };
        fill(&mut self.api_key, &self.api_key_name);
        for c in [
            &mut self.duckduckgo,
            &mut self.searxng,
            &mut self.brave,
            &mut self.tavily,
        ] {
            fill(&mut c.api_key, &c.api_key_name);
        }
    }
}
