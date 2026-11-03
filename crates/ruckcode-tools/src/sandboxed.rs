//! Glue between process-spawning tools and the command sandbox.

use crate::ToolContext;
use ruckcode_sandbox::{Prep, Prepared, Sandbox, SandboxConfig};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a tool call needs to run commands under the sandbox.
#[derive(Clone)]
pub struct SandboxHandle {
    /// Present for process-spawning tools; in-process tools only get the path guards.
    pub sandbox: Option<Arc<Sandbox>>,
    /// The effective config (user config + the active mode's overrides).
    pub cfg: SandboxConfig,
    /// ruckcode's own config (project `.ruckcode`, `.mcp.json`, the user config dir): in-process
    /// tools may not write it.
    pub protected_write: Vec<PathBuf>,
    /// Secrets in-process tools may not read.
    pub deny_read: Vec<PathBuf>,
}

fn normalize(p: &Path) -> PathBuf {
    // Canonicalize the longest existing ancestor so `..` and symlinks cannot dodge the check.
    let mut rest = Vec::new();
    let mut cur = p.to_path_buf();
    loop {
        if let Ok(c) = cur.canonicalize() {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match (cur.file_name().map(|f| f.to_owned()), cur.parent()) {
            (Some(name), Some(par)) => {
                if name == ".." {
                    return p.to_path_buf();
                }
                rest.push(name);
                cur = par.to_path_buf();
            }
            _ => return p.to_path_buf(),
        }
    }
}

impl SandboxHandle {
    pub fn blocks_write(&self, path: &Path) -> bool {
        let n = normalize(path);
        self.protected_write
            .iter()
            .any(|p| n.starts_with(normalize(p)))
    }

    pub fn blocks_read(&self, path: &Path) -> bool {
        let n = normalize(path);
        self.deny_read.iter().any(|p| n.starts_with(normalize(p)))
    }
}

/// Error text for a refused in-process write.
pub const PROTECTED_WRITE_MSG: &str = "refused: this path is ruckcode's own configuration (.ruckcode/, .mcp.json or the user config directory). The sandbox policy can only be changed by the user.";

/// A command about to be spawned, sandboxed when the call has a sandbox.
pub struct Launch {
    plain: Option<tokio::process::Command>,
    prepared: Option<Box<Prepared>>,
    notice: Option<String>,
}

impl Launch {
    /// The command to configure (stdio, env, ...) before spawning.
    pub fn cmd(&mut self) -> &mut tokio::process::Command {
        match (&mut self.prepared, &mut self.plain) {
            (Some(p), _) => &mut p.command,
            (None, Some(c)) => c,
            (None, None) => unreachable!("a launch always has a command"),
        }
    }

    pub fn spawn(&mut self) -> std::io::Result<tokio::process::Child> {
        match &mut self.prepared {
            Some(p) => p.spawn(),
            None => self.cmd().spawn(),
        }
    }

    /// Text to append to the tool result (one-time degradation warning, blocked access).
    pub fn notes(&self, output: &str, success: bool) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(n) = &self.notice {
            parts.push(n.clone());
        }
        if let Some(p) = &self.prepared {
            let n = p.finish(output, success);
            if !n.is_empty() {
                parts.push(n);
            }
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }
}

/// Build the command for `program args...` in `cwd`. `Err` is a refusal message for the model.
pub fn launch(
    ctx: &ToolContext,
    program: &str,
    args: &[String],
    cwd: &Path,
) -> Result<Launch, String> {
    let plain = || {
        let mut c = tokio::process::Command::new(program);
        c.args(args).current_dir(cwd);
        c
    };
    let Some((h, sb)) = ctx
        .sandbox
        .as_ref()
        .and_then(|h| h.sandbox.as_ref().map(|sb| (h, sb)))
    else {
        return Ok(Launch {
            plain: Some(plain()),
            prepared: None,
            notice: None,
        });
    };
    let mut argv = vec![program.to_string()];
    argv.extend(args.iter().cloned());
    match sb.prepare(&h.cfg, cwd, &argv, &ctx.tool_call_id)? {
        Prep::Run(mut p) => {
            let notice = p.notice.take();
            Ok(Launch {
                plain: None,
                prepared: Some(p),
                notice,
            })
        }
        Prep::Unsandboxed { notice } => Ok(Launch {
            plain: Some(plain()),
            prepared: None,
            notice,
        }),
    }
}
