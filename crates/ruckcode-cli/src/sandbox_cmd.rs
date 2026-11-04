//! `rc sandbox status` / `rc sandbox trust-remote`.

use crate::Cli;
use anyhow::{bail, Result};
use clap::Subcommand;
use ruckcode_core::project_policy::{self, Resolution, Status};
use ruckcode_core::{Config, ModeSet};
use ruckcode_sandbox::{LauncherExe, ProjectPolicy, SandboxConfig};
use std::io::IsTerminal;

#[derive(Subcommand, Debug)]
pub enum SandboxAction {
    /// Show the detected capabilities (Landlock ABI, seccomp, namespaces, Seatbelt), the layers
    /// that will be active, the effective policy and where the project's sandbox policy comes
    /// from.
    Status {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Pin the project's current `origin` URL as the source of its sandbox policy (the policy is
    /// only honoured as committed on that remote's default branch). Run it after you have checked
    /// that a changed `origin` is legitimate. Only a person can run this: it is refused inside
    /// the sandbox.
    TrustRemote {
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

pub struct Effective {
    pub cfg: SandboxConfig,
    pub mode: String,
    pub policy: Option<Resolution>,
}

pub fn effective_config(cli: &Cli) -> Result<Effective> {
    let cwd = cli.project_dir()?;
    let mut config = Config::load(&cwd, None)?;
    let mut modes = ModeSet::load(&cwd, None)?;
    let mut policy = None;
    if config.sandbox.project_policy == ProjectPolicy::Remote {
        if let Some(data) = project_policy::data_dir() {
            let res = project_policy::resolve(&cwd, &data, config.sandbox.project_policy);
            project_policy::apply(&res, &mut config.sandbox, &mut modes);
            policy = Some(res);
        }
    }
    let mode = cli
        .mode
        .clone()
        .unwrap_or_else(|| modes.default_mode.clone());
    let over = modes.get(&mode).and_then(|m| m.sandbox.clone());
    Ok(Effective {
        cfg: config.sandbox.with_override(over.as_ref()),
        mode,
        policy,
    })
}

fn policy_lines(cfg: &SandboxConfig, res: Option<&Resolution>) -> Vec<String> {
    let mut v = Vec::new();
    if cfg.project_policy == ProjectPolicy::Off {
        v.push("project policy: off (sandbox.project_policy = \"off\"; the project's sandbox settings are ignored)".into());
        return v;
    }
    let Some(r) = res else {
        v.push("project policy: none".into());
        return v;
    };
    match (&r.status, &r.verified) {
        (Status::Active, Some(ver)) => {
            let what = if ver.docs.is_empty() {
                "no sandbox settings committed"
            } else {
                "active"
            };
            v.push(format!(
                "project policy: {what}; source: {} (branch {}, commit {}, {} {})",
                ver.source.url,
                ver.source.branch,
                ver.source.sha,
                if ver.source.from_cache {
                    "cached copy, last fetched"
                } else {
                    "fetched"
                },
                fmt_ts(ver.source.fetched_at)
            ));
        }
        (Status::NotGit, _) => v.push("project policy: none (not a git repository)".into()),
        (Status::NoRemote, _) => v.push("project policy: none (no `origin` remote)".into()),
        (Status::PinMismatch { pinned, current }, _) => v.push(format!(
            "project policy: IGNORED (origin is {current}, pinned to {pinned}); run `rc sandbox trust-remote` if the change is legitimate"
        )),
        (Status::Unavailable { reason }, _) => {
            v.push(format!("project policy: none ({reason})"))
        }
        (Status::Off, _) | (Status::Active, None) => v.push("project policy: none".into()),
    }
    if let Some(p) = &r.pinned_url {
        v.push(format!("pinned remote: {p}"));
    }
    if let Some(d) = &r.drift {
        v.push(format!("warning: {d}"));
    }
    v
}

fn fmt_ts(secs: u64) -> String {
    project_policy::fmt_time(secs)
}

pub fn run(cli: &Cli, action: &SandboxAction) -> Result<()> {
    match action {
        SandboxAction::Status { json } => status(cli, *json),
        SandboxAction::TrustRemote { yes } => trust_remote(cli, *yes),
    }
}

fn trust_remote(cli: &Cli, yes: bool) -> Result<()> {
    if std::env::var_os("RC_SANDBOX").is_some() {
        bail!("`rc sandbox trust-remote` cannot be run from inside the sandbox; run it yourself in a terminal");
    }
    let cwd = cli.project_dir()?;
    let Some(root) = project_policy::repo_root(&cwd) else {
        bail!("not a git repository");
    };
    let Some(url) = project_policy::origin_url(&root) else {
        bail!("no `origin` remote is configured");
    };
    let Some(data) = project_policy::data_dir() else {
        bail!("cannot locate the user data directory");
    };
    if let Some(old) = project_policy::pinned_url(&cwd, &data) {
        if old == url {
            println!("already pinned to {url}");
            return Ok(());
        }
        println!("currently pinned: {old}");
    }
    println!("origin: {url}");
    println!("The project's sandbox policy will be taken from the default branch of this remote.");
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("not a terminal: pass --yes to confirm");
        }
        eprint!("Trust this remote? [y/N] ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            bail!("aborted");
        }
    }
    let pinned = project_policy::trust_remote(&cwd, &data)?;
    println!("pinned {pinned}");
    Ok(())
}

fn status(cli: &Cli, json: bool) -> Result<()> {
    let Effective { cfg, mode, policy } = effective_config(cli)?;
    let caps = ruckcode_sandbox::detect(&LauncherExe::SelfExe);
    if json {
        let plan = ruckcode_sandbox::plan(&caps, &cfg);
        let pol = policy.as_ref().map(|r| {
            serde_json::json!({
                "status": r.status,
                "pinned_url": r.pinned_url,
                "source": r.verified.as_ref().map(|v| &v.source),
                "drift": r.drift,
            })
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": mode,
                "capabilities": caps,
                "plan": plan,
                "config": cfg,
                "project_policy": pol,
            }))?
        );
        return Ok(());
    }
    println!("mode: {mode}");
    print!("{}", ruckcode_sandbox::report(&caps, &cfg));
    for l in policy_lines(&cfg, policy.as_ref()) {
        println!("{l}");
    }
    if cfg!(target_os = "linux") && caps.platform == "linux" && !caps.namespaces && cfg.enabled {
        println!(
            "hint: on Ubuntu 23.10+ an AppArmor profile that grants `userns` to the rc binary enables the namespace layer (example in docs/SANDBOX.md); nothing is installed automatically."
        );
    }
    Ok(())
}
