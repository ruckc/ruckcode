//! `rc sandbox status`: what the command sandbox can enforce on this machine.

use crate::Cli;
use anyhow::Result;
use clap::Subcommand;
use ruckcode_core::{Config, ModeSet};
use ruckcode_sandbox::{LauncherExe, SandboxConfig};

#[derive(Subcommand, Debug)]
pub enum SandboxAction {
    /// Show the detected capabilities (Landlock ABI, seccomp, namespaces, Seatbelt), the layers
    /// that will be active and the effective policy.
    Status {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

pub fn effective_config(cli: &Cli) -> Result<(SandboxConfig, String)> {
    let cwd = cli.project_dir()?;
    let config = Config::load(&cwd, None)?;
    let modes = ModeSet::load(&cwd, None)?;
    let mode = cli
        .mode
        .clone()
        .unwrap_or_else(|| modes.default_mode.clone());
    let over = modes.get(&mode).and_then(|m| m.sandbox.clone());
    Ok((config.sandbox.with_override(over.as_ref()), mode))
}

pub fn run(cli: &Cli, action: &SandboxAction) -> Result<()> {
    let SandboxAction::Status { json } = action;
    let (cfg, mode) = effective_config(cli)?;
    let caps = ruckcode_sandbox::detect(&LauncherExe::SelfExe);
    if *json {
        let plan = ruckcode_sandbox::plan(&caps, &cfg);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": mode,
                "capabilities": caps,
                "plan": plan,
                "config": cfg,
            }))?
        );
        return Ok(());
    }
    println!("mode: {mode}");
    print!("{}", ruckcode_sandbox::report(&caps, &cfg));
    if cfg!(target_os = "linux") && caps.platform == "linux" && !caps.namespaces && cfg.enabled {
        println!(
            "hint: on Ubuntu 23.10+ an AppArmor profile that grants `userns` to the rc binary enables the namespace layer (example in docs/SANDBOX.md); nothing is installed automatically."
        );
    }
    Ok(())
}
