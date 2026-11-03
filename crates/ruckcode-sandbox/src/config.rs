//! `[sandbox]` configuration and the per-mode override.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    /// No network at all.
    Off,
    /// Only allowlisted domains, through the in-process proxy (unknown domains prompt).
    Allowlist,
    /// Unrestricted network.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NamespaceMode {
    /// Use user/mount/pid/net namespaces when the system allows them.
    Auto,
    /// Refuse to run commands unless namespaces work.
    Required,
    /// Never use namespaces (Landlock + seccomp only).
    Off,
}

/// Where project-level sandbox policy may come from (user config only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectPolicy {
    /// Honour the project's `[sandbox]` / `[modes.<id>.sandbox]` only as committed on the remote's
    /// default branch (verified by a fetch ruckcode runs itself).
    #[default]
    Remote,
    /// Ignore project sandbox policy entirely.
    Off,
}

pub const DEFAULT_ALLOWED_DOMAINS: &[&str] = &[
    "github.com",
    "*.githubusercontent.com",
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "registry.npmjs.org",
    "pypi.org",
    "files.pythonhosted.org",
    "proxy.golang.org",
    "sum.golang.org",
];

/// Directories tools write to (relative to `~`); writable by default when they exist.
pub const DEFAULT_CACHE_PATHS: &[&str] = &[
    "~/.cargo",
    "~/.rustup",
    "~/.npm",
    "~/.cache",
    "~/.local/share/pnpm",
    "~/.local/share/mise",
    "~/.local/state/mise",
    "~/.yarn",
    "~/go",
    "~/.gradle",
    "~/.m2",
    "~/Library/Caches",
];

/// Paths whose contents are never readable by sandboxed commands (relative to `~`).
pub const DEFAULT_DENY_READ: &[&str] = &[
    "~/.ssh",
    "~/.aws",
    "~/.gnupg",
    "~/.config/gh",
    "~/.docker/config.json",
    "~/.netrc",
    "~/.git-credentials",
    "~/.kube",
    "~/.azure",
    "~/.config/gcloud",
    "~/.npmrc",
    "~/.pypirc",
    "~/.cargo/credentials",
    "~/.cargo/credentials.toml",
    "~/.password-store",
    "~/.local/share/keyrings",
];

fn t() -> bool {
    true
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// The only off switch. Honoured from the user's config only, never from project files.
    pub enabled: bool,
    /// Whether project sandbox policy is honoured (`remote`, the default) or ignored (`off`).
    /// Honoured from the user's config only.
    pub project_policy: ProjectPolicy,
    /// Refuse to run commands unless every configured layer is enforced.
    pub strict: bool,
    pub namespaces: NamespaceMode,
    pub network: NetworkMode,
    pub allowed_domains: Vec<String>,
    pub extra_write_paths: Vec<String>,
    pub extra_read_paths: Vec<String>,
    pub deny_read_paths: Vec<String>,
    /// Tool cache directories that are writable (those that exist).
    pub cache_paths: Vec<String>,
    /// Extra local TCP ports commands may connect to (Landlock network rules).
    pub allowed_ports: Vec<u16>,
    /// Mask project `.env*` files (kernel-enforced only with namespaces).
    pub mask_env_files: bool,
    /// In namespace mode, give the command a private `/tmp` that lives as long as the session.
    pub private_tmp: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            project_policy: ProjectPolicy::Remote,
            strict: false,
            namespaces: NamespaceMode::Auto,
            network: NetworkMode::Allowlist,
            allowed_domains: strs(DEFAULT_ALLOWED_DOMAINS),
            extra_write_paths: vec![],
            extra_read_paths: vec![],
            deny_read_paths: vec![],
            cache_paths: strs(DEFAULT_CACHE_PATHS),
            allowed_ports: vec![],
            mask_env_files: t(),
            private_tmp: t(),
        }
    }
}

/// `[modes.<id>.sandbox]`: per-mode overrides. There is deliberately no `enabled` here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxOverride {
    pub strict: Option<bool>,
    pub namespaces: Option<NamespaceMode>,
    pub network: Option<NetworkMode>,
    pub allowed_domains: Option<Vec<String>>,
    pub extra_write_paths: Option<Vec<String>>,
    pub extra_read_paths: Option<Vec<String>>,
    pub deny_read_paths: Option<Vec<String>>,
    pub cache_paths: Option<Vec<String>>,
    pub allowed_ports: Option<Vec<u16>>,
    pub mask_env_files: Option<bool>,
}

impl SandboxOverride {
    /// `other`'s set fields replace this override's.
    pub fn merged_with(&self, other: &SandboxOverride) -> SandboxOverride {
        macro_rules! pick {
            ($($f:ident),*) => {
                SandboxOverride { $($f: other.$f.clone().or_else(|| self.$f.clone())),* }
            };
        }
        pick!(
            strict,
            namespaces,
            network,
            allowed_domains,
            extra_write_paths,
            extra_read_paths,
            deny_read_paths,
            cache_paths,
            allowed_ports,
            mask_env_files
        )
    }
}

impl SandboxConfig {
    /// The config with a mode's overrides applied (lists are replaced, not merged).
    pub fn with_override(&self, o: Option<&SandboxOverride>) -> Self {
        let mut c = self.clone();
        let Some(o) = o else { return c };
        if let Some(v) = o.strict {
            c.strict = v;
        }
        if let Some(v) = o.namespaces {
            c.namespaces = v;
        }
        if let Some(v) = o.network {
            c.network = v;
        }
        if let Some(v) = &o.allowed_domains {
            c.allowed_domains = v.clone();
        }
        if let Some(v) = &o.extra_write_paths {
            c.extra_write_paths = v.clone();
        }
        if let Some(v) = &o.extra_read_paths {
            c.extra_read_paths = v.clone();
        }
        if let Some(v) = &o.deny_read_paths {
            c.deny_read_paths = v.clone();
        }
        if let Some(v) = &o.cache_paths {
            c.cache_paths = v.clone();
        }
        if let Some(v) = &o.allowed_ports {
            c.allowed_ports = v.clone();
        }
        if let Some(v) = o.mask_env_files {
            c.mask_env_files = v;
        }
        c
    }
}

/// Does `host` match an allowlist `pattern` (`example.com`, `*.example.com`, `*`)?
pub fn domain_matches(pattern: &str, host: &str) -> bool {
    let p = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if p == "*" {
        return true;
    }
    match p.strip_prefix("*.") {
        Some(suffix) => h.len() > suffix.len() + 1 && h.ends_with(&format!(".{suffix}")),
        None => p == h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_override() {
        let c = SandboxConfig::default();
        assert!(c.enabled && !c.strict);
        assert_eq!(c.network, NetworkMode::Allowlist);
        assert!(c.allowed_domains.contains(&"crates.io".to_string()));
        let o = SandboxOverride {
            network: Some(NetworkMode::Off),
            extra_write_paths: Some(vec!["/data".into()]),
            ..Default::default()
        };
        let e = c.with_override(Some(&o));
        assert_eq!(e.network, NetworkMode::Off);
        assert_eq!(e.extra_write_paths, ["/data"]);
        assert!(e.enabled);
    }

    #[test]
    fn override_cannot_disable() {
        let bad: Result<SandboxOverride, _> = toml_like(r#"{"enabled": false}"#);
        assert!(bad.is_err());
    }

    fn toml_like(s: &str) -> Result<SandboxOverride, serde_json::Error> {
        serde_json::from_str(s)
    }

    #[test]
    fn domains() {
        assert!(domain_matches("github.com", "GitHub.com"));
        assert!(domain_matches(
            "*.githubusercontent.com",
            "raw.githubusercontent.com"
        ));
        assert!(!domain_matches(
            "*.githubusercontent.com",
            "githubusercontent.com"
        ));
        assert!(!domain_matches("github.com", "evilgithub.com"));
        assert!(!domain_matches("github.com", "x.github.com"));
        assert!(domain_matches("*", "anything.io"));
    }
}
