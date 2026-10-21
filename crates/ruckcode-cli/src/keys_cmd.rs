//! `rc keys`: manage named API keys in credentials.toml. Secrets are never printed.

use std::io::{IsTerminal, Read};

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use ruckcode_core::config::credentials_path;
use ruckcode_core::keys::{self, KeyScope};

use crate::Cli;

#[derive(Subcommand, Debug)]
pub enum KeysAction {
    /// List named keys (masked) and which profiles reference them.
    List,
    /// Add or replace a named key; the secret is read from stdin or a masked prompt.
    Add {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove a named key.
    Remove { name: String },
    /// Point a profile at a named key (in your config, or this project's with --project).
    Use {
        name: String,
        /// Profile to change (default: the default profile).
        #[arg(long)]
        profile: Option<String>,
        /// Save in .ruckcode/config.toml (this project only).
        #[arg(long)]
        project: bool,
    },
}

/// Read a secret without echo when stdin is a terminal, else from the piped stdin.
fn read_secret(prompt: &str) -> Result<String> {
    use crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    if !std::io::stdin().is_terminal() {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        return Ok(s.trim().to_string());
    }
    eprint!("{prompt}");
    enable_raw_mode()?;
    let mut secret = String::new();
    let result = loop {
        match read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(anyhow::anyhow!("aborted"))
                }
                KeyCode::Char(c) => secret.push(c),
                KeyCode::Backspace => {
                    secret.pop();
                }
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(e.into()),
        }
    };
    let _ = disable_raw_mode();
    eprintln!();
    result?;
    Ok(secret.trim().to_string())
}

pub async fn run(cli: &Cli, action: &KeysAction) -> Result<()> {
    let project = cli.project_dir()?;
    match action {
        KeysAction::List => {
            let list = keys::list_keys(None)?;
            let refs = keys::key_references(None, &project);
            if list.is_empty() {
                println!(
                    "no named keys ({}); add one with `rc keys add <name>`",
                    credentials_path(None)
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
            }
            for k in &list {
                let used: Vec<&str> = refs
                    .iter()
                    .filter(|(n, _)| *n == k.name)
                    .map(|(_, w)| w.as_str())
                    .collect();
                println!(
                    "{:<20} {:<8} {}{}",
                    k.name,
                    k.masked,
                    k.description.as_deref().unwrap_or(""),
                    if used.is_empty() {
                        "  [unused]".to_string()
                    } else {
                        format!("  [used by {}]", used.join(", "))
                    }
                );
            }
            for (n, w) in &refs {
                if !list.iter().any(|k| k.name == *n) {
                    println!("warning: {w} references missing key `{n}` (run `rc keys add {n}`)");
                }
            }
        }
        KeysAction::Add { name, description } => {
            keys::validate_key_name(name)?;
            let secret = read_secret(&format!("API key for `{name}` (hidden): "))?;
            if secret.is_empty() {
                bail!("no key entered");
            }
            let p = keys::add_key(None, name, &secret, description.as_deref())?;
            println!("saved key `{name}` to {}", p.display());
        }
        KeysAction::Remove { name } => {
            let refs: Vec<String> = keys::key_references(None, &project)
                .into_iter()
                .filter(|(n, _)| n == name)
                .map(|(_, w)| w)
                .collect();
            if !keys::remove_key(None, name)? {
                bail!("no key named `{name}`");
            }
            println!("removed key `{name}`");
            if !refs.is_empty() {
                println!("warning: still referenced by {}", refs.join(", "));
            }
        }
        KeysAction::Use {
            name,
            profile,
            project: in_project,
        } => {
            if !keys::list_keys(None)?.iter().any(|k| k.name == *name) {
                bail!("no key named `{name}`; add it with `rc keys add {name}`");
            }
            let profile = match profile {
                Some(p) => p.clone(),
                None => {
                    ruckcode_core::Config::load(&project, None)
                        .context("loading config")?
                        .default_profile
                }
            };
            let scope = if *in_project {
                KeyScope::Project
            } else {
                KeyScope::User
            };
            let p = keys::use_key(name, &profile, scope, None, &project)?;
            println!(
                "profile `{profile}` now uses key `{name}` ({})",
                p.display()
            );
        }
    }
    Ok(())
}
