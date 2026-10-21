//! Onboarding backend: provider presets, and writing profiles, credentials and per-mode models.
//!
//! Everything here is plain file manipulation (via `toml_edit`, so existing comments and
//! formatting survive) so the setup wizard and `rc setup --non-interactive` share it and it can
//! be tested without a terminal.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::config::{credentials_path, user_config_dir, Profile, ProviderKind};
use crate::keys::{self, KeyScope};

/// A provider choice offered by the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPreset {
    /// Short id (also the default profile name), e.g. `ollama`.
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ProviderKind,
    /// `None` means the user must enter a URL (vLLM, LM Studio, other).
    pub base_url: Option<&'static str>,
    /// Conventional key env var; `None` means no key is needed.
    pub api_key_env: Option<&'static str>,
}

pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "anthropic",
        label: "Anthropic",
        kind: ProviderKind::Anthropic,
        base_url: Some("https://api.anthropic.com"),
        api_key_env: Some("ANTHROPIC_API_KEY"),
    },
    ProviderPreset {
        id: "openai",
        label: "OpenAI",
        kind: ProviderKind::Openai,
        base_url: Some("https://api.openai.com/v1"),
        api_key_env: Some("OPENAI_API_KEY"),
    },
    ProviderPreset {
        id: "ollama",
        label: "Ollama (local)",
        kind: ProviderKind::Openai,
        base_url: Some("http://localhost:11434/v1"),
        api_key_env: None,
    },
    ProviderPreset {
        id: "openrouter",
        label: "OpenRouter",
        kind: ProviderKind::Openai,
        base_url: Some("https://openrouter.ai/api/v1"),
        api_key_env: Some("OPENROUTER_API_KEY"),
    },
    ProviderPreset {
        id: "custom",
        label: "vLLM / LM Studio / other OpenAI-compatible",
        kind: ProviderKind::Openai,
        base_url: None,
        api_key_env: None,
    },
];

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// The endpoint a provider kind uses when `base_url` is unset.
pub fn default_base_url(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => ANTHROPIC_BASE_URL,
        _ => "https://api.openai.com/v1",
    }
}

/// `base_url` as stored in config: `None` when it equals the provider's built-in default.
pub fn stored_base_url(kind: ProviderKind, url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() || url == default_base_url(kind) {
        None
    } else {
        Some(url.to_string())
    }
}

/// What the wizard (or `--non-interactive`) decided.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupPlan {
    pub profile_name: String,
    pub profile: Profile,
    /// Make this the `default_profile` (always done when the user config has none yet).
    pub set_default: bool,
    /// A new named key to store in `credentials.toml` (never in a config file). The profile
    /// should reference it through `profile.api_key_name`.
    pub new_key: Option<NewKey>,
    /// Where `profile.api_key_name` is saved: the user's profile or this project's config.
    pub key_scope: KeyScope,
    /// Per mode: `Some((profile, model))` pins it, `None` means "profile default".
    pub mode_models: Vec<(String, Option<(String, String)>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewKey {
    pub name: String,
    pub secret: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetupReport {
    pub config_path: PathBuf,
    /// Set when the key reference went to the project's `.ruckcode/config.toml`.
    pub project_config_path: Option<PathBuf>,
    pub credentials_path: Option<PathBuf>,
    pub modes_path: Option<PathBuf>,
    /// Modes whose `profile`/`model` keys were changed.
    pub modes_updated: usize,
}

/// True when there is no user config file and no provider key in the environment.
pub fn needs_onboarding(user_dir: Option<&Path>, env: &dyn Fn(&str) -> Option<String>) -> bool {
    let has_config = user_config_dir(user_dir)
        .map(|d| d.join("config.toml").is_file())
        .unwrap_or(false);
    let has_key = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"]
        .iter()
        .any(|k| env(k).is_some_and(|v| !v.is_empty()));
    !has_config && !has_key
}

/// Reads the process environment, for [`needs_onboarding`].
pub fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Profiles defined in the user or project config files (not the built-in defaults).
pub fn configured_profiles(project: &Path, user_dir: Option<&Path>) -> Vec<Profile> {
    let Ok(cfg) = crate::Config::load(project, user_dir) else {
        return vec![];
    };
    let mut names = std::collections::BTreeSet::new();
    let files = [
        user_config_dir(user_dir).map(|d| d.join("config.toml")),
        Some(project_config_file(project)),
    ];
    for f in files.into_iter().flatten() {
        if let Ok(text) = std::fs::read_to_string(f) {
            if let Ok(t) = text.parse::<toml::Table>() {
                if let Some(toml::Value::Table(p)) = t.get("profiles") {
                    names.extend(p.keys().cloned());
                }
            }
        }
    }
    cfg.profiles
        .into_values()
        .filter(|p| names.contains(&p.name))
        .collect()
}

fn config_file(user_dir: Option<&Path>) -> Result<PathBuf> {
    Ok(user_config_dir(user_dir)
        .context("cannot determine the user config directory")?
        .join("config.toml"))
}

fn read_or_empty(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn set_str(t: &mut Table, key: &str, val: &str) {
    if t.get(key).and_then(Item::as_str) != Some(val) {
        t[key] = value(val);
    }
}

fn sub_table<'a>(parent: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    if !parent.contains_key(key) {
        let mut t = Table::new();
        t.set_implicit(true);
        parent.insert(key, Item::Table(t));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .with_context(|| format!("`{key}` must be a table"))
}

/// `config.toml` text after applying the plan to `existing`.
pub fn render_config(existing: &str, plan: &SetupPlan) -> Result<String> {
    let mut doc: DocumentMut = existing.parse().context("parsing config.toml")?;
    let profiles = sub_table(doc.as_table_mut(), "profiles")?;
    let t = sub_table(profiles, &plan.profile_name)?;
    t.set_implicit(false);
    let p = &plan.profile;
    set_str(t, "provider", p.provider.as_str());
    set_str(t, "model", &p.model);
    match &p.base_url {
        Some(u) => set_str(t, "base_url", u),
        None => {
            t.remove("base_url");
        }
    }
    match &p.api_key_env {
        Some(e) => set_str(t, "api_key_env", e),
        None => {
            t.remove("api_key_env");
        }
    }
    if plan.key_scope == KeyScope::User {
        match &p.api_key_name {
            Some(n) => set_str(t, "api_key_name", n),
            None => {
                t.remove("api_key_name");
            }
        }
    }
    let current: Vec<String> = t
        .get("models")
        .and_then(Item::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if current != p.models {
        if p.models.is_empty() {
            t.remove("models");
        } else {
            let mut arr = Array::new();
            for m in &p.models {
                arr.push(m.as_str());
            }
            if arr.len() > 3 {
                arr.set_trailing_comma(true);
                arr.set_trailing("\n");
                for v in arr.iter_mut() {
                    v.decor_mut().set_prefix("\n    ");
                }
            }
            t["models"] = value(arr);
        }
    }
    if plan.set_default || doc.get("default_profile").is_none() {
        set_str(doc.as_table_mut(), "default_profile", &plan.profile_name);
    }
    Ok(doc.to_string())
}

/// Project `.ruckcode/config.toml` text after applying the plan's key reference, if it has one.
pub fn render_project_config(existing: &str, plan: &SetupPlan) -> Result<Option<String>> {
    match (&plan.key_scope, &plan.profile.api_key_name) {
        (KeyScope::Project, Some(n)) => Ok(Some(keys::render_key_ref(
            existing,
            &plan.profile_name,
            Some(n),
        )?)),
        _ => Ok(None),
    }
}

fn project_config_file(project: &Path) -> PathBuf {
    project.join(".ruckcode").join("config.toml")
}

/// `modes.toml` text after applying the per-mode choices; `None` if there is nothing to write.
pub fn render_modes(existing: Option<&str>, plan: &SetupPlan) -> Result<Option<String>> {
    if plan.mode_models.is_empty() {
        return Ok(None);
    }
    let Some(existing) = existing else {
        if plan.mode_models.iter().any(|(_, c)| c.is_some()) {
            bail!(".ruckcode/modes.toml does not exist; run `rc init` first");
        }
        return Ok(None);
    };
    let mut doc: DocumentMut = existing.parse().context("parsing modes.toml")?;
    let modes = doc
        .get_mut("modes")
        .and_then(Item::as_table_mut)
        .context("modes.toml has no [modes.*] tables")?;
    for (id, choice) in &plan.mode_models {
        let Some(t) = modes.get_mut(id).and_then(Item::as_table_mut) else {
            bail!("mode `{id}` is not defined in modes.toml");
        };
        match choice {
            Some((profile, model)) => {
                set_str(t, "profile", profile);
                set_str(t, "model", model);
            }
            None => {
                t.remove("profile");
                t.remove("model");
            }
        }
    }
    Ok(Some(doc.to_string()))
}

fn modes_file(project: &Path) -> PathBuf {
    project.join(".ruckcode").join("modes.toml")
}

/// Apply a plan: user `config.toml`, `credentials.toml` and the project's `modes.toml`.
/// Nothing is written if any of the documents fails to render.
pub fn apply(plan: &SetupPlan, user_dir: Option<&Path>, project: &Path) -> Result<SetupReport> {
    let cfg_path = config_file(user_dir)?;
    let new_cfg = render_config(&read_or_empty(&cfg_path)?, plan)?;
    let creds = match &plan.new_key {
        Some(k) => {
            let path = credentials_path(user_dir).context("no credentials path")?;
            let text = keys::render_key(
                &read_or_empty(&path)?,
                &k.name,
                &k.secret,
                k.description.as_deref(),
            )?;
            Some((path, text))
        }
        None => None,
    };
    let proj_cfg_path = project_config_file(project);
    let proj_cfg = render_project_config(&read_or_empty(&proj_cfg_path)?, plan)?;
    let modes_path = modes_file(project);
    let existing_modes = std::fs::read_to_string(&modes_path).ok();
    let new_modes = render_modes(existing_modes.as_deref(), plan)?;

    let mut report = SetupReport {
        config_path: cfg_path.clone(),
        ..Default::default()
    };
    if let Some(dir) = cfg_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&cfg_path, new_cfg)
        .with_context(|| format!("writing {}", cfg_path.display()))?;
    if let Some((path, text)) = creds {
        keys::write_private(&path, &text)?;
        report.credentials_path = Some(path);
    }
    if let Some(text) = proj_cfg {
        if let Some(d) = proj_cfg_path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&proj_cfg_path, text)
            .with_context(|| format!("writing {}", proj_cfg_path.display()))?;
        report.project_config_path = Some(proj_cfg_path);
    }
    if let Some(text) = new_modes {
        if existing_modes.as_deref() != Some(text.as_str()) {
            std::fs::write(&modes_path, text)
                .with_context(|| format!("writing {}", modes_path.display()))?;
            report.modes_path = Some(modes_path);
            report.modes_updated = plan.mode_models.len();
        }
    }
    Ok(report)
}

fn unified(name: &str, old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(2)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}

/// Human-readable account of what [`apply`] would write. Never contains the API key.
pub fn preview(plan: &SetupPlan, user_dir: Option<&Path>, project: &Path) -> Result<String> {
    let cfg_path = config_file(user_dir)?;
    let old_cfg = read_or_empty(&cfg_path)?;
    let new_cfg = render_config(&old_cfg, plan)?;
    let mut out = String::new();
    out.push_str(&format!("# {}\n", cfg_path.display()));
    if old_cfg == new_cfg {
        out.push_str("(no changes)\n");
    } else {
        out.push_str(&unified("config.toml", &old_cfg, &new_cfg));
    }
    if let Some(k) = &plan.new_key {
        let path = credentials_path(user_dir).context("no credentials path")?;
        out.push_str(&format!(
            "\nkey `{}` will be saved to {} (mode 0600)\n",
            k.name,
            path.display()
        ));
    }
    let pc_path = project_config_file(project);
    let old_pc = read_or_empty(&pc_path)?;
    if let Some(new_pc) = render_project_config(&old_pc, plan)? {
        out.push_str(&format!("\n# {} (this project only)\n", pc_path.display()));
        out.push_str(&unified("config.toml", &old_pc, &new_pc));
    }
    let modes_path = modes_file(project);
    let old_modes = std::fs::read_to_string(&modes_path).ok();
    if let Some(new_modes) = render_modes(old_modes.as_deref(), plan)? {
        out.push_str(&format!("\n# {}\n", modes_path.display()));
        let old = old_modes.unwrap_or_default();
        if old == new_modes {
            out.push_str("(no changes)\n");
        } else {
            out.push_str(&unified("modes.toml", &old, &new_modes));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, ModeSet};

    fn profile(name: &str, models: &[&str]) -> Profile {
        Profile {
            name: name.into(),
            provider: ProviderKind::Openai,
            model: models.first().copied().unwrap_or("m").into(),
            models: models.iter().map(|s| s.to_string()).collect(),
            api_key_name: None,
            api_key_env: Some("OPENROUTER_API_KEY".into()),
            base_url: Some("https://openrouter.ai/api/v1".into()),
            max_tokens: 32_000,
            context_window: 200_000,
            thinking: None,
        }
    }

    fn plan(models: &[&str]) -> SetupPlan {
        SetupPlan {
            profile_name: "openrouter".into(),
            profile: profile("openrouter", models),
            set_default: false,
            new_key: None,
            key_scope: KeyScope::User,
            mode_models: vec![],
        }
    }

    #[test]
    fn onboarding_detection() {
        let d = tempfile::tempdir().unwrap();
        let none = |_: &str| None;
        assert!(needs_onboarding(Some(d.path()), &none));
        let key = |k: &str| (k == "OPENAI_API_KEY").then(|| "sk".to_string());
        assert!(!needs_onboarding(Some(d.path()), &key));
        let empty = |_: &str| Some(String::new());
        assert!(needs_onboarding(Some(d.path()), &empty));
        std::fs::write(d.path().join("config.toml"), "").unwrap();
        assert!(!needs_onboarding(Some(d.path()), &none));
    }

    #[test]
    fn config_edit_preserves_comments_and_loads_back() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let original = "# my settings\nmax_turn_requests = 10 # keep\n\n[profiles.mine]\n# my own\nprovider = \"mock\"\nmodel = \"x\"\n";
        std::fs::write(user.path().join("config.toml"), original).unwrap();
        let mut p = plan(&["a/one", "b/two"]);
        p.set_default = true;
        let r = apply(&p, Some(user.path()), proj.path()).unwrap();
        assert_eq!(r.config_path, user.path().join("config.toml"));
        let text = std::fs::read_to_string(user.path().join("config.toml")).unwrap();
        assert!(text.contains("# my settings"), "{text}");
        assert!(text.contains("max_turn_requests = 10 # keep"), "{text}");
        assert!(text.contains("# my own"), "{text}");
        assert!(!text.contains("sk-"), "{text}");
        let cfg = Config::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(cfg.default_profile, "openrouter");
        let o = &cfg.profiles["openrouter"];
        assert_eq!(o.name, "openrouter");
        assert_eq!(o.models, ["a/one", "b/two"]);
        assert_eq!(o.model, "a/one");
        assert_eq!(o.provider, ProviderKind::Openai);
        assert_eq!(o.api_key_env.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(cfg.profiles["mine"].model, "x");
        assert_eq!(cfg.max_turn_requests, 10);
        // Re-applying is idempotent.
        apply(&p, Some(user.path()), proj.path()).unwrap();
        let again = std::fs::read_to_string(user.path().join("config.toml")).unwrap();
        assert_eq!(text, again);
    }

    #[test]
    fn existing_profile_is_updated_in_place() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("config.toml"),
            "default_profile = \"openrouter\"\n[profiles.openrouter]\nprovider = \"openai\"\nmodel = \"old\"\napi_key_env = \"OLD\" # old var\nthinking = \"high\"\n",
        )
        .unwrap();
        let mut p = plan(&["new"]);
        p.profile.api_key_env = None;
        apply(&p, Some(user.path()), proj.path()).unwrap();
        let cfg = Config::load(proj.path(), Some(user.path())).unwrap();
        let o = &cfg.profiles["openrouter"];
        assert_eq!(o.model, "new");
        assert_eq!(o.api_key_env, None);
        assert_eq!(o.thinking.as_ref().and_then(|v| v.as_str()), Some("high"));
    }

    #[test]
    fn first_profile_becomes_default_without_set_default() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        apply(&plan(&["m"]), Some(user.path()), proj.path()).unwrap();
        let cfg = Config::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(cfg.default_profile, "openrouter");
        // A later profile does not steal the default.
        let mut p2 = plan(&["z"]);
        p2.profile_name = "other".into();
        p2.profile.name = "other".into();
        apply(&p2, Some(user.path()), proj.path()).unwrap();
        let cfg = Config::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(cfg.default_profile, "openrouter");
        assert!(cfg.profiles.contains_key("other"));
    }

    #[cfg(unix)]
    #[test]
    fn named_keys_are_private_and_never_in_config() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let user = base.path().join("nested/ruckcode");
        let proj = tempfile::tempdir().unwrap();
        let mut p = plan(&["m"]);
        p.new_key = Some(NewKey {
            name: "work".into(),
            secret: "sk-secret-123".into(),
            description: Some("team".into()),
        });
        p.profile.api_key_env = None;
        p.profile.api_key_name = Some("work".into());
        let r = apply(&p, Some(&user), proj.path()).unwrap();
        let cred = user.join("credentials.toml");
        assert_eq!(r.credentials_path.as_deref(), Some(cred.as_path()));
        assert_eq!(
            std::fs::metadata(&cred).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let cfg = std::fs::read_to_string(user.join("config.toml")).unwrap();
        assert!(
            !cfg.contains("sk-secret") && cfg.contains("api_key_name = \"work\""),
            "{cfg}"
        );
        assert_eq!(
            crate::config::named_key(Some(&user), "work").as_deref(),
            Some("sk-secret-123")
        );
        // Shared by a second profile; file stays private.
        let mut p2 = plan(&["m"]);
        p2.profile_name = "second".into();
        p2.profile.name = "second".into();
        p2.profile.api_key_name = Some("work".into());
        apply(&p2, Some(&user), proj.path()).unwrap();
        let c = crate::Config::load(proj.path(), Some(&user)).unwrap();
        assert_eq!(c.profiles["second"].api_key_name.as_deref(), Some("work"));
        assert_eq!(
            std::fs::metadata(&cred).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let pv = preview(&p, Some(&user), proj.path()).unwrap();
        assert!(
            pv.contains("credentials.toml") && !pv.contains("sk-secret"),
            "{pv}"
        );
    }

    #[test]
    fn project_scope_writes_reference_to_project_config_only() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let mut p = plan(&["m"]);
        p.key_scope = KeyScope::Project;
        p.profile.api_key_name = Some("client-acme".into());
        p.new_key = Some(NewKey {
            name: "client-acme".into(),
            secret: "sk-acme-0000".into(),
            description: None,
        });
        let r = apply(&p, Some(user.path()), proj.path()).unwrap();
        assert!(r.project_config_path.is_some());
        let u = std::fs::read_to_string(user.path().join("config.toml")).unwrap();
        assert!(!u.contains("api_key_name"), "{u}");
        let pc = std::fs::read_to_string(proj.path().join(".ruckcode/config.toml")).unwrap();
        assert!(
            pc.contains("api_key_name = \"client-acme\"") && !pc.contains("sk-acme"),
            "{pc}"
        );
        let c = crate::Config::load(proj.path(), Some(user.path())).unwrap();
        let o = &c.profiles["openrouter"];
        // Project overrides only the key reference; the rest comes from the user profile.
        assert_eq!(o.api_key_name.as_deref(), Some("client-acme"));
        assert_eq!(o.model, "m");
        assert_eq!(o.provider, ProviderKind::Openai);
    }

    #[test]
    fn key_resolution_order_and_missing_key_error() {
        let user = tempfile::tempdir().unwrap();
        keys::add_key(Some(user.path()), "k1", "from-file", None).unwrap();
        let mut prof = profile("p", &[]);
        prof.provider = ProviderKind::Anthropic;
        prof.api_key_env = Some("RUCKCODE_TEST_SURELY_UNSET_VAR".into());
        // No name, env unset -> none -> anthropic errors.
        assert_eq!(
            crate::config::resolve_api_key(&prof, Some(user.path())).unwrap(),
            None
        );
        let e = crate::config::make_provider_with(&prof, Some(user.path()))
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("rc setup"), "{e}");
        // Named key wins over env.
        std::env::set_var("RUCKCODE_TEST_SET_VAR", "from-env");
        prof.api_key_env = Some("RUCKCODE_TEST_SET_VAR".into());
        assert_eq!(
            crate::config::resolve_api_key(&prof, Some(user.path()))
                .unwrap()
                .as_deref(),
            Some("from-env")
        );
        prof.api_key_name = Some("k1".into());
        assert_eq!(
            crate::config::resolve_api_key(&prof, Some(user.path()))
                .unwrap()
                .as_deref(),
            Some("from-file")
        );
        assert!(crate::config::make_provider_with(&prof, Some(user.path())).is_ok());
        // Missing named key is an explicit error (no silent fall back to the env var).
        prof.api_key_name = Some("gone".into());
        let e = crate::config::make_provider_with(&prof, Some(user.path()))
            .err()
            .unwrap()
            .to_string();
        assert!(
            e.contains("named key `gone`") && e.contains("rc keys add gone"),
            "{e}"
        );
    }

    #[test]
    fn project_profile_override_is_field_wise() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("config.toml"),
            "[profiles.anthropic]\nmodel = \"u-model\"\nmax_tokens = 123\napi_key_name = \"personal\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(
            proj.path().join(".ruckcode/config.toml"),
            "[profiles.anthropic]\napi_key_name = \"client-acme\"\n",
        )
        .unwrap();
        let c = crate::Config::load(proj.path(), Some(user.path())).unwrap();
        let a = &c.profiles["anthropic"];
        assert_eq!(a.api_key_name.as_deref(), Some("client-acme"));
        assert_eq!((a.model.as_str(), a.max_tokens), ("u-model", 123));
        assert_eq!(a.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    const MODES: &str = "# team modes\ndefault_mode = \"explore\"\n\n[modes.explore]\n# model = \"claude-haiku-5-5\"\ndescription = \"look\"\nmodel = \"legacy\"\n\n[modes.explore.permissions]\nBash = \"ask\"\n\n[modes.test]\ndescription = \"test\"\nprofile = \"gone\"\nmodel = \"x\"\n";

    #[test]
    fn mode_models_set_and_remove() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        let mp = proj.path().join(".ruckcode/modes.toml");
        std::fs::write(&mp, MODES).unwrap();
        let mut p = plan(&["fast", "big"]);
        p.mode_models = vec![
            ("explore".into(), Some(("openrouter".into(), "fast".into()))),
            ("test".into(), None),
        ];
        let r = apply(&p, Some(user.path()), proj.path()).unwrap();
        assert_eq!(r.modes_path.as_deref(), Some(mp.as_path()));
        let text = std::fs::read_to_string(&mp).unwrap();
        assert!(text.contains("# team modes"), "{text}");
        assert!(text.contains("# model = \"claude-haiku-5-5\""), "{text}");
        assert!(text.contains("[modes.explore.permissions]"), "{text}");
        let ms = ModeSet::load(proj.path(), Some(user.path())).unwrap();
        let e = ms.get("explore").unwrap();
        assert_eq!(e.profile.as_deref(), Some("openrouter"));
        assert_eq!(e.model.as_deref(), Some("fast"));
        let t = ms.get("test").unwrap();
        assert_eq!((t.profile.clone(), t.model.clone()), (None, None));
        // Unknown mode is an error and nothing is written.
        let mut bad = plan(&["m"]);
        bad.mode_models = vec![("nope".into(), None)];
        let before = std::fs::read_to_string(user.path().join("config.toml")).unwrap();
        bad.profile_name = "zzz".into();
        assert!(apply(&bad, Some(user.path()), proj.path()).is_err());
        assert_eq!(
            before,
            std::fs::read_to_string(user.path().join("config.toml")).unwrap()
        );
        // Missing modes.toml: pinning is an error, clearing is a no-op.
        let empty = tempfile::tempdir().unwrap();
        assert!(apply(&p, Some(user.path()), empty.path()).is_err());
    }

    #[test]
    fn preview_shows_diffs() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        std::fs::write(proj.path().join(".ruckcode/modes.toml"), MODES).unwrap();
        let mut p = plan(&["fast"]);
        p.mode_models = vec![("explore".into(), Some(("openrouter".into(), "fast".into())))];
        let pv = preview(&p, Some(user.path()), proj.path()).unwrap();
        assert!(
            pv.contains("+[profiles.openrouter]") || pv.contains("+provider"),
            "{pv}"
        );
        assert!(pv.contains("+profile = \"openrouter\""), "{pv}");
        assert!(pv.contains("modes.toml"), "{pv}");
        // Nothing was written by previewing.
        assert!(!user.path().join("config.toml").exists());
    }

    #[test]
    fn base_url_normalisation() {
        assert_eq!(
            stored_base_url(ProviderKind::Anthropic, "https://api.anthropic.com/"),
            None
        );
        assert_eq!(
            stored_base_url(ProviderKind::Openai, "http://localhost:11434/v1/").as_deref(),
            Some("http://localhost:11434/v1")
        );
    }
}
