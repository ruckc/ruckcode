//! The session-level sandbox: capability detection, the proxy, and command preparation.

use crate::caps::{self, Capabilities, Plan};
use crate::config::{NetworkMode, SandboxConfig};
use crate::policy::{build_policy, BuildInput, Policy};
use crate::proxy::{self, DomainPrompter, DomainStore, Gate, Proxy};
use crate::spawn::{launcher_command, Fd, LauncherExe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct SandboxOptions {
    pub project_root: PathBuf,
    pub exe: LauncherExe,
    pub prompter: Option<Arc<dyn DomainPrompter>>,
    pub store: Option<DomainStore>,
    /// ruckcode's user config dir (its credentials are never readable).
    pub config_dir: Option<PathBuf>,
    /// Home directory override (default: the user's home).
    pub home: Option<PathBuf>,
    /// Skip detection and use these capabilities (tests).
    pub caps: Option<Capabilities>,
}

pub struct Sandbox {
    exe: LauncherExe,
    caps: Capabilities,
    gate: Arc<Gate>,
    proxy: Option<Proxy>,
    stage: PathBuf,
    project_root: PathBuf,
    config_dir: Option<PathBuf>,
    home: Option<PathBuf>,
    warned: AtomicBool,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.stage);
    }
}

/// A command ready to spawn, plus the bookkeeping to explain what the sandbox did to it.
pub struct Prepared {
    pub command: tokio::process::Command,
    fds: Vec<Fd>,
    /// Warnings to show once per session (degraded enforcement).
    pub notice: Option<String>,
    snapshot: ConfigSnapshot,
    gate: Arc<Gate>,
    cfg: SandboxConfig,
    layers_note: String,
    roots: Vec<PathBuf>,
}

pub enum Prep {
    Run(Box<Prepared>),
    /// Platform without sandbox support; run the command plainly (after showing the notice).
    Unsandboxed {
        notice: Option<String>,
    },
}

impl Prepared {
    pub fn spawn(&mut self) -> std::io::Result<tokio::process::Child> {
        let c = self.command.spawn();
        self.fds.clear();
        c
    }

    /// After the command finished: restore tampered ruckcode config and explain any blocking.
    pub fn finish(&self, output: &str, success: bool) -> String {
        let mut notes: Vec<String> = Vec::new();
        for p in self.snapshot.restore() {
            notes.push(format!(
                "[sandbox] The command modified ruckcode configuration ({}); the change was reverted. Sandboxed commands cannot change ruckcode's config or modes.",
                p.display()
            ));
        }
        for p in self.snapshot.restore_git() {
            notes.push(format!(
                "[sandbox] The command modified {}; the change was reverted. Sandboxed commands cannot change the git remote configuration, hooks or remote-tracking refs (`.git/config`, `.git/hooks`, `.git/info`, `.git/refs/remotes`, `.git/packed-refs`, `.git/FETCH_HEAD`): they would run outside the sandbox or forge the source of the sandbox policy.",
                p.display()
            ));
        }
        let denied = self.gate.take_denials();
        if !denied.is_empty() {
            notes.push(format!(
                "[sandbox] Network access was blocked for: {}. These domains are not in sandbox.allowed_domains and were not approved. The user can add them to `sandbox.allowed_domains` in config.toml (or approve them when prompted); you cannot change the sandbox policy.",
                denied.join(", ")
            ));
        }
        if !success {
            notes.extend(fs_denial_note(
                output,
                &self.cfg,
                &self.layers_note,
                &self.roots,
            ));
            if denied.is_empty() && self.cfg.network == NetworkMode::Off && network_failure(output)
            {
                notes.push("[sandbox] Network access is disabled (`sandbox.network = \"off\"`). The user can change this in config.toml or the mode's `[modes.<id>.sandbox]` in modes.toml.".into());
            }
        }
        notes.join("\n")
    }
}

fn network_failure(out: &str) -> bool {
    let l = out.to_lowercase();
    [
        "could not resolve",
        "network is unreachable",
        "name or service not known",
        "temporary failure in name resolution",
        "connection refused",
        "failed to connect",
    ]
    .iter()
    .any(|p| l.contains(p))
}

fn fs_denial_note(
    output: &str,
    cfg: &SandboxConfig,
    layers: &str,
    roots: &[PathBuf],
) -> Option<String> {
    use std::sync::OnceLock;
    static KW: OnceLock<regex::Regex> = OnceLock::new();
    static PATH: OnceLock<regex::Regex> = OnceLock::new();
    let kw = KW.get_or_init(|| {
        regex::Regex::new(
            r"(?i)permission denied|operation not permitted|read-only file system|EACCES|EPERM|no such file or directory|ENOENT|nonexistent|does not exist|not found|cannot (open|create|access|stat)|failed to (open|create)",
        )
        .unwrap()
    });
    let path = PATH.get_or_init(|| regex::Regex::new(r#"(/[A-Za-z0-9_.@+~%=,\-/]+)"#).unwrap());
    let covered = |p: &Path| roots.iter().any(|r| p.starts_with(r));
    // Paths named on error lines that exist on the host but are outside the sandbox's view.
    let mut outside: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut perm_hit = false;
    for line in output.lines().filter(|l| kw.is_match(l)) {
        let l = line.to_lowercase();
        let missing = [
            "no such file",
            "enoent",
            "nonexistent",
            "does not exist",
            "not found",
        ]
        .iter()
        .any(|k| l.contains(k));
        perm_hit |= !missing;
        for m in path.find_iter(line) {
            let t = m.as_str().trim_end_matches([':', ',', '.']);
            let pb = Path::new(t);
            if t.len() < 2 {
                continue;
            }
            let on_host = pb.exists() || pb.parent().is_some_and(|d| d.exists());
            let list = if !covered(pb) && on_host {
                &mut outside
            } else {
                &mut other
            };
            if !list.iter().any(|x| x == t) && list.len() < 6 {
                list.push(t.to_string());
            }
        }
    }
    if outside.is_empty() && !perm_hit {
        return None;
    }
    let what = if !outside.is_empty() {
        format!(
            "access to {} (outside the paths the sandbox exposes)",
            outside.join(", ")
        )
    } else if other.is_empty() {
        "access to a path".to_string()
    } else {
        format!("access to {}", other.join(", "))
    };
    Some(format!(
        "[sandbox] The command failed and the sandbox may have blocked {what}. Sandboxed commands can read most of the system but write only to the project directory, temp directories and tool caches ({} cache dirs configured); secrets such as ~/.ssh are unreadable; ruckcode's own config is read-only ({layers}). Do not try to work around the sandbox. If access is genuinely needed, tell the user they can widen the policy with `sandbox.extra_write_paths`, `sandbox.extra_read_paths` or `sandbox.deny_read_paths` in config.toml, or per mode under `[modes.<id>.sandbox]` in modes.toml.",
        cfg.cache_paths.len()
    ))
}

/// Contents of ruckcode's own config files (and the protected parts of `.git`) before a command
/// ran.
struct ConfigSnapshot {
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
    git: crate::git::Snapshot,
}

impl ConfigSnapshot {
    fn take(project: &Path, config_dir: Option<&Path>) -> Self {
        let mut paths = vec![
            project.join(".ruckcode/config.toml"),
            project.join(".ruckcode/modes.toml"),
            project.join(".mcp.json"),
        ];
        if let Some(d) = config_dir {
            paths.push(d.join("config.toml"));
            paths.push(d.join("modes.toml"));
        }
        Self {
            files: paths
                .into_iter()
                .map(|p| {
                    let c = std::fs::read(&p).ok();
                    (p, c)
                })
                .collect(),
            git: crate::git::Snapshot::take(project),
        }
    }

    /// Undo changes to the git paths a command must not touch.
    fn restore_git(&self) -> Vec<PathBuf> {
        self.git.restore()
    }

    /// Undo changes; returns the paths that had been changed.
    fn restore(&self) -> Vec<PathBuf> {
        let mut changed = vec![];
        for (p, saved) in &self.files {
            let now = std::fs::read(p).ok();
            if &now == saved {
                continue;
            }
            match saved {
                Some(c) => {
                    let _ = std::fs::write(p, c);
                }
                None => {
                    let _ = std::fs::remove_file(p);
                }
            }
            changed.push(p.clone());
        }
        changed
    }
}

/// The proxy endpoint handed to commands. A literal IPv4 address, never `localhost`: the proxy
/// listens on 127.0.0.1 only, and `localhost` may resolve to `::1` first, which neither the proxy
/// nor the Seatbelt/Landlock rules (which name 127.0.0.1's port) cover.
pub(crate) fn proxy_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Environment that points a command's HTTP clients at the proxy. `loopback_is_local` is true
/// when loopback is only "local" inside a network namespace (or for explicitly allowed ports).
pub(crate) fn proxy_env(port: u16, loopback_is_local: bool) -> Vec<(&'static str, String)> {
    let url = proxy_url(port);
    let mut v: Vec<(&'static str, String)> = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ]
    .into_iter()
    .map(|k| (k, url.clone()))
    .collect();
    if loopback_is_local {
        for k in ["NO_PROXY", "no_proxy"] {
            v.push((k, "localhost,127.0.0.1,::1".to_string()));
        }
    }
    for k in [
        "npm_config_proxy",
        "npm_config_https_proxy",
        "CARGO_HTTP_PROXY",
    ] {
        v.push((k, url.clone()));
    }
    v
}

impl Sandbox {
    pub async fn new(opts: SandboxOptions) -> Arc<Self> {
        let caps = opts.caps.clone().unwrap_or_else(|| caps::detect(&opts.exe));
        let gate = Arc::new(Gate::new(opts.store, opts.prompter));
        // Unique per Sandbox: a securely created (O_EXCL, 0700) directory, not a name derived
        // from the pid and clock, which two Sandboxes in one process could share. Removed in Drop.
        let stage = tempfile::Builder::new()
            .prefix("rc-sbx-")
            .tempdir()
            .map(tempfile::TempDir::keep)
            .unwrap_or_else(|e| {
                tracing::warn!("cannot create the sandbox stage dir ({e}); using a fallback");
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                std::env::temp_dir().join(format!(
                    "rc-sbx-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ))
            });
        let _ = std::fs::create_dir_all(stage.join("tmp"));
        let _ = std::fs::create_dir_all(stage.join("vartmp"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o700));
            for d in ["tmp", "vartmp"] {
                let _ = std::fs::set_permissions(
                    stage.join(d),
                    std::fs::Permissions::from_mode(0o1777),
                );
            }
        }
        let sock = stage.join("proxy.sock");
        let proxy = match proxy::start(gate.clone(), Some(sock)).await {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("sandbox proxy failed to start: {e}");
                None
            }
        };
        Arc::new(Self {
            exe: opts.exe,
            caps,
            gate,
            proxy,
            stage,
            project_root: opts.project_root,
            config_dir: opts.config_dir,
            home: opts.home,
            warned: AtomicBool::new(false),
        })
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    pub fn plan(&self, cfg: &SandboxConfig) -> Plan {
        caps::plan(&self.caps, cfg)
    }

    pub fn report(&self, cfg: &SandboxConfig) -> String {
        caps::report(&self.caps, cfg)
    }

    /// Domains the proxy allows without asking (from the effective config).
    pub fn set_allowed_domains(&self, domains: Vec<String>) {
        self.gate.set_allowed(domains);
    }

    pub fn set_current_call(&self, tool_call_id: &str) {
        self.gate.set_current_call(tool_call_id);
    }

    pub fn proxy_port(&self) -> Option<u16> {
        self.proxy.as_ref().map(|p| p.port)
    }

    /// The policy that would be applied for `cfg` in `cwd`.
    pub fn policy_for(&self, cfg: &SandboxConfig, cwd: &Path, plan: &Plan) -> Policy {
        build_policy(&BuildInput {
            cfg,
            cwd,
            project_root: &self.project_root,
            home: self.home.clone().or_else(dirs::home_dir),
            config_dir: self.config_dir.clone(),
            layers: plan.layers,
            proxy_port: self.proxy_port(),
            proxy_socket: self.proxy.as_ref().and_then(|p| p.socket.clone()),
            stage_dir: Some(self.stage.clone()),
            private_tmp: Some(self.stage.clone()),
        })
    }

    /// Prepare `argv` to run in `cwd` under `cfg`. `Err` is a refusal message for the model.
    pub fn prepare(
        &self,
        cfg: &SandboxConfig,
        cwd: &Path,
        argv: &[String],
        tool_call_id: &str,
    ) -> Result<Prep, String> {
        let plan = self.plan(cfg);
        if let Some(r) = &plan.refusal {
            return Err(format!(
                "the command was not run: {r}. The user can fix the environment, relax `sandbox.strict`, or disable the sandbox with `sandbox.enabled = false` in their config.toml."
            ));
        }
        self.gate.set_allowed(cfg.allowed_domains.clone());
        self.gate.set_current_call(tool_call_id);
        let notice = if !plan.warnings.is_empty() && !self.warned.swap(true, Ordering::SeqCst) {
            Some(format!("[sandbox] {}", plan.warnings.join("\n[sandbox] ")))
        } else {
            None
        };
        if plan.unsandboxed {
            return Ok(Prep::Unsandboxed { notice });
        }
        let policy = self.policy_for(cfg, cwd, &plan);
        let (mut std_cmd, fds) = launcher_command(&self.exe, &policy, argv).map_err(|e| {
            format!("the command was not run: cannot start the sandbox launcher: {e}")
        })?;
        std_cmd.current_dir(cwd);
        std_cmd.env("RC_SANDBOX", "1");
        for k in [
            "SSH_AUTH_SOCK",
            "GPG_AGENT_INFO",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            std_cmd.env_remove(k);
        }
        if policy.layers.namespaces && policy.private_tmp.is_some() {
            std_cmd.env("TMPDIR", "/tmp");
        }
        if let (NetworkMode::Allowlist, Some(port)) = (cfg.network, policy.proxy_port) {
            for (k, v) in proxy_env(port, policy.layers.netns || !cfg.allowed_ports.is_empty()) {
                std_cmd.env(k, v);
            }
        }
        let l = policy.layers;
        let layers_note = format!(
            "enforced by: {}",
            [
                (l.namespaces, "namespaces"),
                (l.landlock, "landlock"),
                (l.seccomp, "seccomp"),
                (l.seatbelt, "seatbelt"),
            ]
            .iter()
            .filter(|(f, _)| *f)
            .map(|(_, n)| *n)
            .collect::<Vec<_>>()
            .join("+")
        );
        Ok(Prep::Run(Box::new(Prepared {
            command: tokio::process::Command::from(std_cmd),
            fds,
            notice,
            snapshot: ConfigSnapshot::take(&self.project_root, self.config_dir.as_deref()),
            gate: self.gate.clone(),
            cfg: cfg.clone(),
            layers_note,
            roots: policy
                .read_paths
                .iter()
                .chain(policy.write_paths.iter())
                .cloned()
                .collect(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_note_extracts_paths() {
        let cfg = SandboxConfig::default();
        let n = fs_denial_note(
            "touch: cannot touch '/etc/foo': Permission denied\nother",
            &cfg,
            "enforced by: landlock",
            &[],
        )
        .unwrap();
        assert!(n.contains("/etc/foo") && n.contains("extra_write_paths"));
        assert!(fs_denial_note("all fine", &cfg, "", &[]).is_none());
    }

    #[test]
    fn proxy_env_uses_the_literal_ipv4_loopback() {
        let env = proxy_env(4242, false);
        for k in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "CARGO_HTTP_PROXY",
        ] {
            let v = env
                .iter()
                .find(|(n, _)| *n == k)
                .unwrap_or_else(|| panic!("{k} unset"));
            assert_eq!(v.1, "http://127.0.0.1:4242", "{k}");
        }
        assert!(!env.iter().any(|(k, _)| k.eq_ignore_ascii_case("no_proxy")));
        let env = proxy_env(4242, true);
        assert!(env.iter().any(|(k, _)| *k == "NO_PROXY"));
    }

    #[tokio::test]
    async fn proxy_listens_on_ipv4_loopback() {
        let gate = Arc::new(Gate::new(None, None));
        let p = proxy::start(gate, None).await.unwrap();
        // The port in the Seatbelt rule and in HTTP_PROXY is reachable at exactly 127.0.0.1.
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", p.port))
            .await
            .is_ok());
    }

    #[test]
    fn config_snapshot_restores() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".ruckcode")).unwrap();
        let m = d.path().join(".ruckcode/modes.toml");
        std::fs::write(&m, "orig").unwrap();
        let snap = ConfigSnapshot::take(d.path(), None);
        std::fs::write(&m, "evil").unwrap();
        std::fs::write(d.path().join(".ruckcode/config.toml"), "[sandbox]").unwrap();
        let ch = snap.restore();
        assert_eq!(ch.len(), 2);
        assert_eq!(std::fs::read_to_string(&m).unwrap(), "orig");
        assert!(!d.path().join(".ruckcode/config.toml").exists());
    }
}
