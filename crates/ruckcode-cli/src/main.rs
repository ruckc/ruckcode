//! `rc`: the ruckcode command line. An ACP client: the agent is only ever reached over ACP.

mod client;
mod headless;
mod init_cmd;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use ruckcode_core::{Agent, AgentOptions};

#[derive(Parser, Debug)]
#[command(name = "rc", version, about = "ruckcode: a mode-driven coding agent")]
pub struct Cli {
    /// Initial prompt (interactive) or the prompt to run (with --print).
    pub prompt: Option<String>,

    /// Headless mode: print the response and exit.
    #[arg(short = 'p', long)]
    pub print: bool,

    /// Output format for --print.
    #[arg(long, value_enum, default_value = "text")]
    pub output_format: OutputFormat,

    /// Tools to auto-allow in --print mode (comma separated, e.g. Bash,Edit).
    #[arg(long, value_delimiter = ',')]
    pub allow: Vec<String>,

    /// Allow every permission request in --print mode.
    #[arg(long)]
    pub yolo: bool,

    /// Initial mode id.
    #[arg(long)]
    pub mode: Option<String>,

    /// Model profile from config.
    #[arg(long)]
    pub profile: Option<String>,

    /// Override the model of the profile.
    #[arg(long)]
    pub model: Option<String>,

    /// Project directory (default: current directory).
    #[arg(long, global = true)]
    pub cwd: Option<PathBuf>,

    /// Resume a session by id; without an id, pick one interactively.
    #[arg(short = 'r', long, num_args = 0..=1, default_missing_value = "", value_name = "ID")]
    pub resume: Option<String>,

    /// Continue the most recent session.
    #[arg(short = 'c', long = "continue")]
    pub continue_last: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    StreamJson,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Serve the agent over ACP on stdio (for editors such as Zed).
    Acp,
    /// Generate .ruckcode/modes.toml for this project.
    Init {
        /// Use the minimal plan/build/verify modes.
        #[arg(long)]
        minimal: bool,
        /// Comma separated subset of modes to generate.
        #[arg(long, value_delimiter = ',')]
        modes: Vec<String>,
        /// Print a diff between the existing file and what would be generated.
        #[arg(long)]
        diff: bool,
        /// Overwrite an existing modes.toml.
        #[arg(long)]
        force: bool,
    },
    /// List sessions of this project.
    Sessions,
}

impl Cli {
    pub fn project_dir(&self) -> Result<PathBuf> {
        let dir = match &self.cwd {
            Some(d) => d.clone(),
            None => std::env::current_dir()?,
        };
        dir.canonicalize()
            .with_context(|| format!("cannot access {}", dir.display()))
    }

    pub fn build_agent(&self) -> Arc<Agent> {
        Arc::new(Agent::new(AgentOptions {
            profile_override: self.profile.clone(),
            model_override: self.model.clone(),
            ..Default::default()
        }))
    }
}

fn init_logging(to_file: bool) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("RUCKCODE_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    if to_file {
        let path = dirs::cache_dir().map(|d| d.join("ruckcode").join("rc.log"));
        if let Some(path) = path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .try_init();
                return;
            }
        }
        // No usable log file: stay silent rather than corrupt the screen.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("off"))
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let interactive = cli.command.is_none() && !cli.print;
    init_logging(interactive);
    let result: Result<u8> = match &cli.command {
        Some(Command::Acp) => run_acp(&cli).await.map(|_| 0),
        Some(Command::Init {
            minimal,
            modes,
            diff,
            force,
        }) => init_cmd::run(&cli, *minimal, modes, *diff, *force)
            .await
            .map(|_| 0),
        Some(Command::Sessions) => run_sessions(&cli).await.map(|_| 0),
        None if cli.print => headless::run(&cli).await,
        None => tui::run(&cli).await.map(|_| 0),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("rc: {e:#}");
            ExitCode::from(1)
        }
    }
}

async fn run_acp(cli: &Cli) -> Result<()> {
    ruckcode_acp::agent::serve_stdio(cli.build_agent()).await
}

/// Connect an in-process agent with a client handler and run the ACP handshake.
pub async fn connect(
    cli: &Cli,
    handler: Arc<dyn ruckcode_acp::Handler>,
) -> Result<ruckcode_acp::Connection> {
    let conn = ruckcode_acp::agent::spawn_in_process(cli.build_agent(), handler);
    client::initialize(&conn).await?;
    Ok(conn)
}

struct NoHandler;

#[async_trait::async_trait]
impl ruckcode_acp::Handler for NoHandler {
    async fn request(
        &self,
        method: &str,
        _params: serde_json::Value,
    ) -> Result<serde_json::Value, ruckcode_acp::RpcError> {
        Err(ruckcode_acp::RpcError::method_not_found(method))
    }
    async fn notification(&self, _method: &str, _params: serde_json::Value) {}
}

async fn run_sessions(cli: &Cli) -> Result<()> {
    let cwd = cli.project_dir()?;
    let conn = connect(cli, Arc::new(NoHandler)).await?;
    let sessions = client::list_sessions(&conn, &cwd).await?;
    if sessions.is_empty() {
        println!("no sessions in {}", cwd.display());
        return Ok(());
    }
    for s in sessions {
        println!(
            "{}  {}  {:<10} {}",
            s.id,
            s.updated_at,
            s.mode,
            one_line(&s.first_prompt, 60)
        );
    }
    Ok(())
}

pub fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        let t: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    } else {
        flat
    }
}
