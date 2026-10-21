//! `rc setup`: the interactive wizard, or `--non-interactive` for scripts.

use anyhow::{bail, Context, Result};
use clap::Args;
use ruckcode_core::config::Profile;
use ruckcode_core::keys::{self, KeyScope};
use ruckcode_core::setup::{self, NewKey, SetupPlan, PRESETS};

use crate::Cli;

#[derive(Args, Debug, Default)]
pub struct SetupArgs {
    /// Skip the wizard and apply the flags below.
    #[arg(long)]
    pub non_interactive: bool,
    /// anthropic, openai, or a preset: ollama, openrouter.
    #[arg(long)]
    pub provider: Option<String>,
    /// Endpoint (default: the provider's usual URL).
    #[arg(long)]
    pub base_url: Option<String>,
    /// Read the key from this environment variable at run time.
    #[arg(long)]
    pub api_key_env: Option<String>,
    /// Use (or, with --api-key-stdin, create) this named key in credentials.toml.
    #[arg(long)]
    pub api_key_name: Option<String>,
    /// Read the secret for --api-key-name from stdin and save it to credentials.toml.
    #[arg(long)]
    pub api_key_stdin: bool,
    /// Save the key reference in this project's .ruckcode/config.toml instead of your profile.
    #[arg(long)]
    pub project_key: bool,
    /// Profile name (default: the provider).
    #[arg(long)]
    pub profile: Option<String>,
    /// Comma separated models to make available (required).
    #[arg(long, value_delimiter = ',')]
    pub models: Vec<String>,
    /// Default model of the profile (default: the first of --models).
    #[arg(long)]
    pub default_model: Option<String>,
    /// `MODE=MODEL` to use MODEL (of this profile) in a mode, or `MODE=default`. Repeatable.
    #[arg(long = "mode-model", value_name = "MODE=MODEL")]
    pub mode_model: Vec<String>,
    /// Do not make this the default profile.
    #[arg(long)]
    pub no_default: bool,
}

pub async fn run(cli: &Cli, args: &SetupArgs) -> Result<()> {
    if !args.non_interactive {
        return crate::tui::setup::runner::standalone(cli).await;
    }
    let project = cli.project_dir()?;
    let plan = build_plan(args, || {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        Ok(s)
    })?;
    let report = setup::apply(&plan, None, &project)?;
    println!("{}", crate::tui::setup::runner::summary(&report));
    Ok(())
}

/// Build the plan for `--non-interactive`; `stdin` supplies the secret for `--api-key-stdin`.
pub fn build_plan(args: &SetupArgs, stdin: impl FnOnce() -> Result<String>) -> Result<SetupPlan> {
    let id = args
        .provider
        .as_deref()
        .context("--provider is required with --non-interactive")?;
    let preset = PRESETS
        .iter()
        .find(|p| p.id == id && p.id != "custom")
        .with_context(|| {
            format!("unknown provider `{id}`: use anthropic, openai, ollama or openrouter")
        })?;
    if args.models.is_empty() {
        bail!("--models is required with --non-interactive");
    }
    let name = args
        .profile
        .clone()
        .unwrap_or_else(|| preset.id.to_string());
    let base = args
        .base_url
        .clone()
        .or_else(|| preset.base_url.map(String::from))
        .unwrap_or_default();
    let default_model = args
        .default_model
        .clone()
        .unwrap_or_else(|| args.models[0].clone());
    let mut models = args.models.clone();
    if !models.contains(&default_model) {
        models.insert(0, default_model.clone());
    }

    let (mut api_key_env, mut api_key_name, mut new_key) = (None, None, None);
    if let Some(n) = &args.api_key_name {
        keys::validate_key_name(n)?;
        api_key_name = Some(n.clone());
        if args.api_key_stdin {
            let secret = stdin()?.trim().to_string();
            if secret.is_empty() {
                bail!("--api-key-stdin: no key on stdin");
            }
            new_key = Some(NewKey {
                name: n.clone(),
                secret,
                description: Some(format!("{} key", preset.label)),
            });
        } else if !keys::list_keys(None)?.iter().any(|k| k.name == *n) {
            bail!("named key `{n}` not found in credentials.toml; add it with `rc keys add {n}` or pass --api-key-stdin");
        }
    } else if args.api_key_stdin {
        bail!("--api-key-stdin needs --api-key-name");
    } else if let Some(e) = &args.api_key_env {
        api_key_env = Some(e.clone());
    } else {
        api_key_env = preset.api_key_env.map(String::from);
    }

    let mut mode_models = vec![];
    for spec in &args.mode_model {
        let (mode, model) = spec
            .split_once('=')
            .with_context(|| format!("--mode-model `{spec}`: expected MODE=MODEL"))?;
        let choice = (model != "default").then(|| (name.clone(), model.to_string()));
        mode_models.push((mode.to_string(), choice));
    }

    Ok(SetupPlan {
        profile_name: name.clone(),
        profile: Profile {
            name,
            provider: preset.kind,
            model: default_model,
            models,
            api_key_name,
            api_key_env,
            base_url: setup::stored_base_url(preset.kind, &base),
            max_tokens: 32_000,
            context_window: 200_000,
            thinking: None,
        },
        set_default: !args.no_default,
        new_key,
        key_scope: if args.project_key {
            KeyScope::Project
        } else {
            KeyScope::User
        },
        mode_models,
    })
}
