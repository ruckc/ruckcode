//! Project-level sandbox policy, honoured only as committed on the remote's default branch.
//!
//! A sandboxed command can write the project, including `.git`, so nothing in the working tree,
//! the index, local commits, `refs/remotes/*` or `.git/config` can be trusted as the source of
//! sandbox policy. Instead, at session start ruckcode itself (outside the sandbox):
//!
//! 1. reads the `origin` URL and pins it (trust on first use) in the user data directory, which
//!    sandboxed commands cannot write; a later URL change disables the project policy until the
//!    user runs `rc sandbox trust-remote`;
//! 2. resolves the remote's default branch (`git ls-remote --symref <url> HEAD`) and fetches it
//!    *by URL* (`git fetch --no-tags --depth 1 <url> <branch>`) into a private bare repository
//!    in the user data directory, so no repository-local config, hook or object store takes part;
//! 3. reads `.ruckcode/config.toml` (`[sandbox]`) and `.ruckcode/modes.toml`
//!    (`[modes.<id>.sandbox]`) from the fetched commit (`git show <sha>:<path>`);
//! 4. caches the commit and those two blobs; offline, the cached verified copy is used.

use anyhow::{anyhow, bail, Context, Result};
use ruckcode_sandbox::{ProjectPolicy, SandboxConfig, SandboxOverride};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Environment variable overriding the user data directory (default `<data dir>/ruckcode`).
pub const DATA_DIR_ENV: &str = "RUCKCODE_DATA_DIR";

const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(5);
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const LOCAL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BLOB: usize = 1024 * 1024;

/// The user data directory holding pins, caches and the private fetch repositories. Outside the
/// project and not writable by sandboxed commands.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    dirs::data_dir().map(|d| d.join("ruckcode"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A Unix timestamp as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn fmt_time(secs: u64) -> String {
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| secs.to_string())
}

// ---- persisted state ----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Cache {
    url: String,
    branch: String,
    sha: String,
    fetched_at: u64,
    config_toml: Option<String>,
    modes_toml: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    repo_root: String,
    pinned_url: Option<String>,
    pinned_at: u64,
    /// The one-time "policy will be taken from ..." notice was shown.
    notified: bool,
    cache: Option<Cache>,
}

struct Paths {
    state: PathBuf,
    mirror: PathBuf,
}

fn paths(data_dir: &Path, repo_root: &Path) -> Paths {
    let key = ruckcode_sandbox::project_key(repo_root);
    let base = data_dir.join("project-policy");
    Paths {
        state: base.join(format!("{key}.json")),
        mirror: base.join(format!("{key}.git")),
    }
}

impl State {
    fn load(p: &Path) -> Self {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self, p: &Path) {
        let Some(dir) = p.parent() else { return };
        let _ = std::fs::create_dir_all(dir);
        let tmp = p.with_extension("tmp");
        if let Ok(text) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, p);
            }
        }
    }
}

// ---- running git safely ----

fn null_device() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

/// The user's own global git config (credential helpers, `url.*.insteadOf` for their network),
/// never the repository's.
fn global_git_config() -> PathBuf {
    if let Some(p) = std::env::var_os("GIT_CONFIG_GLOBAL").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(h) = dirs::home_dir() {
        let p = h.join(".gitconfig");
        if p.exists() {
            return p;
        }
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")));
    if let Some(x) = xdg.map(|x| x.join("git/config")) {
        if x.exists() {
            return x;
        }
    }
    PathBuf::from(null_device())
}

/// A git command that cannot run repository-controlled code and never prompts. The environment
/// is rebuilt from scratch (no inherited `GIT_DIR`, `GIT_CONFIG_*`, `GIT_EXEC_PATH`, ...).
fn git_command(git_dir: Option<&Path>, cwd: &Path) -> Command {
    let mut c = Command::new("git");
    c.env_clear();
    for k in [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "LANG",
        "XDG_CONFIG_HOME",
        "XDG_RUNTIME_DIR",
        "SSH_AUTH_SOCK",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
        // Windows
        "USERPROFILE",
        "SystemRoot",
        "SYSTEMROOT",
        "TEMP",
        "TMP",
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramData",
    ] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
        c.env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=5",
        );
    }
    c.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", global_git_config())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .env("SSH_ASKPASS", "true")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_ALLOW_PROTOCOL", "file:git:http:https:ssh")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    if let Some(d) = git_dir {
        c.env("GIT_DIR", d);
    }
    let hooks = format!("core.hooksPath={}", null_device());
    c.args(["-c", "core.fsmonitor=", "-c", &hooks])
        .args(["-c", "protocol.ext.allow=never"])
        .args(["-c", "gc.auto=0", "-c", "maintenance.auto=false"])
        .args(["-c", "fetch.fsckObjects=true"])
        .current_dir(cwd)
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c
}

struct GitOut {
    ok: bool,
    stdout: Vec<u8>,
    stderr: String,
}

fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<GitOut> {
    use std::io::Read;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("cannot run git")?;
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let (so, se) = (
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = out.by_ref().take(MAX_BLOB as u64 + 1).read_to_end(&mut b);
            b
        }),
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = err.by_ref().take(16 * 1024).read_to_end(&mut b);
            b
        }),
    );
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(s) => break s,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // The pipe readers are abandoned: a grandchild (ssh) may still hold the pipes.
                bail!("timed out after {}s", timeout.as_secs());
            }
            None => std::thread::sleep(Duration::from_millis(15)),
        }
    };
    Ok(GitOut {
        ok: status.success(),
        stdout: so.join().unwrap_or_default(),
        stderr: String::from_utf8_lossy(&se.join().unwrap_or_default())
            .trim()
            .to_string(),
    })
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

fn safe_ref(r: &str) -> bool {
    !r.is_empty()
        && !r.starts_with('-')
        && r.len() < 256
        && r.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+@".contains(c))
        && !r.contains("..")
}

fn safe_url(u: &str) -> bool {
    !u.is_empty() && !u.starts_with('-') && !u.contains(['\n', '\0']) && !u.starts_with("ext::")
}

/// The branch `HEAD` points to on the remote.
fn default_branch(url: &str, cwd: &Path) -> Result<String> {
    let mut c = git_command(None, cwd);
    c.args(["ls-remote", "--symref", "--", url, "HEAD"]);
    let o = run_with_timeout(c, LS_REMOTE_TIMEOUT)?;
    if !o.ok {
        bail!("git ls-remote failed: {}", first_line(&o.stderr));
    }
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        if let Some(rest) = line.strip_prefix("ref: ") {
            let r = rest.split_whitespace().next().unwrap_or("");
            if let Some(b) = r.strip_prefix("refs/heads/") {
                if safe_ref(b) {
                    return Ok(b.to_string());
                }
            }
        }
    }
    bail!("the remote did not report a default branch")
}

struct Fetched {
    branch: String,
    sha: String,
    config_toml: Option<String>,
    modes_toml: Option<String>,
}

fn show(mirror: &Path, sha: &str, path: &str) -> Option<String> {
    let mut c = git_command(Some(mirror), mirror);
    c.args(["show", &format!("{sha}:{path}")]);
    let o = run_with_timeout(c, LOCAL_TIMEOUT).ok()?;
    (o.ok && o.stdout.len() <= MAX_BLOB).then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Fetch `url`'s default branch into the private repository and read the policy files.
fn verified_fetch(url: &str, mirror: &Path, prefix: &str) -> Result<Fetched> {
    if !safe_url(url) {
        bail!("refusing the remote URL");
    }
    let parent = mirror.parent().context("no data directory")?;
    std::fs::create_dir_all(parent)?;
    let branch = default_branch(url, parent)?;
    if !mirror.join("HEAD").exists() {
        let mut c = git_command(None, parent);
        c.args(["init", "--bare", "-q"]).arg(mirror);
        let o = run_with_timeout(c, LOCAL_TIMEOUT)?;
        if !o.ok {
            bail!(
                "cannot create the fetch repository: {}",
                first_line(&o.stderr)
            );
        }
    }
    let mut c = git_command(Some(mirror), mirror);
    c.args(["fetch", "--no-tags", "--depth", "1", "--", url])
        .arg(format!("refs/heads/{branch}"));
    let o = run_with_timeout(c, FETCH_TIMEOUT)?;
    if !o.ok {
        bail!("git fetch failed: {}", first_line(&o.stderr));
    }
    let mut c = git_command(Some(mirror), mirror);
    c.args(["rev-parse", "--verify", "FETCH_HEAD^{commit}"]);
    let o = run_with_timeout(c, LOCAL_TIMEOUT)?;
    let sha = first_line(&String::from_utf8_lossy(&o.stdout));
    if !o.ok || sha.len() < 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("cannot resolve FETCH_HEAD");
    }
    Ok(Fetched {
        config_toml: show(mirror, &sha, &format!("{prefix}.ruckcode/config.toml")),
        modes_toml: show(mirror, &sha, &format!("{prefix}.ruckcode/modes.toml")),
        branch,
        sha,
    })
}

/// The raw `remote.origin.url` of the repository at `root`, read without includes and without
/// applying `insteadOf` rewrites.
pub fn origin_url(root: &Path) -> Option<String> {
    // An explicit file: no includes, no `insteadOf`, no repository discovery.
    let cfg = ruckcode_sandbox::git_common_dir(root)?.join("config");
    let mut c = git_command(None, root);
    c.args(["config", "--no-includes", "--file"])
        .arg(&cfg)
        .args(["--get", "remote.origin.url"]);
    let o = run_with_timeout(c, LOCAL_TIMEOUT).ok()?;
    let u = first_line(&String::from_utf8_lossy(&o.stdout));
    (o.ok && !u.is_empty()).then_some(u)
}

/// The directory containing `.git`, searching upwards from `project`.
pub fn repo_root(project: &Path) -> Option<PathBuf> {
    let start = project.canonicalize().ok()?;
    start
        .ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

// ---- policy documents ----

/// `[sandbox]` of a config document, without the keys a project may never set.
fn sandbox_table(config_toml: &str) -> Result<Option<toml::Table>> {
    let t: toml::Table = config_toml.parse().context("parsing config.toml")?;
    let Some(toml::Value::Table(mut sb)) = t.get("sandbox").cloned() else {
        return Ok(None);
    };
    sb.remove("enabled");
    sb.remove("project_policy");
    Ok((!sb.is_empty()).then_some(sb))
}

/// `[modes.<id>.sandbox]` tables of a modes document.
fn mode_tables(modes_toml: &str) -> Result<BTreeMap<String, toml::Table>> {
    let t: toml::Table = modes_toml.parse().context("parsing modes.toml")?;
    let mut out = BTreeMap::new();
    if let Some(toml::Value::Table(modes)) = t.get("modes") {
        for (id, v) in modes {
            if let Some(toml::Value::Table(sb)) = v.get("sandbox") {
                if !sb.is_empty() {
                    out.insert(id.clone(), sb.clone());
                }
            }
        }
    }
    Ok(out)
}

/// A project's sandbox policy as found in a set of files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolicyDocs {
    pub sandbox: Option<toml::Table>,
    pub modes: BTreeMap<String, toml::Table>,
}

impl PolicyDocs {
    fn parse(config: Option<&str>, modes: Option<&str>) -> Result<Self> {
        Ok(Self {
            sandbox: config.map(sandbox_table).transpose()?.flatten(),
            modes: modes.map(mode_tables).transpose()?.unwrap_or_default(),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.sandbox.is_none() && self.modes.is_empty()
    }
}

/// The working-tree copy of the project's sandbox policy (never trusted; used for drift only).
pub fn working_tree_docs(project: &Path) -> PolicyDocs {
    let read = |p: &str| std::fs::read_to_string(project.join(p)).ok();
    PolicyDocs::parse(
        read(".ruckcode/config.toml").as_deref(),
        read(".ruckcode/modes.toml").as_deref(),
    )
    .unwrap_or_default()
}

/// Where a verified policy came from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Source {
    pub url: String,
    pub branch: String,
    pub sha: String,
    pub fetched_at: u64,
    /// The fetch failed (offline, ...) and this is the cached verified copy.
    pub from_cache: bool,
}

impl Source {
    pub fn describe(&self) -> String {
        format!(
            "{} {} @ {} ({} {})",
            self.url,
            self.branch,
            &self.sha[..self.sha.len().min(12)],
            if self.from_cache {
                "cached, fetched"
            } else {
                "fetched"
            },
            fmt_time(self.fetched_at)
        )
    }
}

/// The policy read from the verified copy.
#[derive(Debug, Clone)]
pub struct Verified {
    pub source: Source,
    pub docs: PolicyDocs,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    /// `sandbox.project_policy = "off"`.
    Off,
    NotGit,
    NoRemote,
    /// The `origin` URL differs from the pinned one: project policy ignored.
    PinMismatch {
        pinned: String,
        current: String,
    },
    /// No verified copy could be fetched and none is cached.
    Unavailable {
        reason: String,
    },
    Active,
}

/// The outcome of resolving the project policy at session start.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub status: Status,
    pub verified: Option<Verified>,
    /// One-time messages for the session (pinning, cache use, pin mismatch, ...).
    pub notices: Vec<String>,
    /// Set when the working tree's sandbox config differs from the verified copy.
    pub drift: Option<String>,
    pub pinned_url: Option<String>,
    /// Name the local tree's policy files are compared against ("<branch> on <url>").
    pub tracking: Option<String>,
}

impl Resolution {
    fn plain(status: Status) -> Self {
        Self {
            status,
            verified: None,
            notices: vec![],
            drift: None,
            pinned_url: None,
            tracking: None,
        }
    }
}

/// Resolve the verified project sandbox policy for `project`. Blocking (runs git, with
/// timeouts); call from `spawn_blocking` in async code.
pub fn resolve(project: &Path, data_dir: &Path, mode: ProjectPolicy) -> Resolution {
    if mode == ProjectPolicy::Off {
        return Resolution::plain(Status::Off);
    }
    let Some(root) = repo_root(project) else {
        return with_local_drift(Resolution::plain(Status::NotGit), project);
    };
    let p = paths(data_dir, &root);
    let mut state = State::load(&p.state);
    state.repo_root = root.display().to_string();
    let current = origin_url(&root);
    let mut res = Resolution::plain(Status::NoRemote);
    let mut first_pin = false;
    let url = match (&state.pinned_url, current) {
        (None, None) => return with_local_drift(res, project),
        (Some(pinned), None) => {
            res.status = Status::PinMismatch {
                pinned: pinned.clone(),
                current: "(no origin remote)".into(),
            };
            res.pinned_url = Some(pinned.clone());
            res.notices
                .push(mismatch_notice(pinned, "(no origin remote)"));
            return with_local_drift(res, project);
        }
        (None, Some(u)) => {
            // Trust on first use: the pin is made at the first session in the project, before
            // any sandboxed command could have changed `origin`.
            state.pinned_url = Some(u.clone());
            state.pinned_at = now();
            state.cache = None;
            first_pin = true;
            u
        }
        (Some(pinned), Some(u)) if *pinned != u => {
            res.status = Status::PinMismatch {
                pinned: pinned.clone(),
                current: u.clone(),
            };
            res.pinned_url = Some(pinned.clone());
            res.notices.push(mismatch_notice(pinned, &u));
            return with_local_drift(res, project);
        }
        (Some(pinned), Some(_)) => pinned.clone(),
    };
    res.pinned_url = Some(url.clone());

    // The project's own `.ruckcode` directory relative to the repository root.
    let prefix = project
        .canonicalize()
        .ok()
        .and_then(|c| c.strip_prefix(&root).ok().map(Path::to_path_buf))
        .filter(|r| !r.as_os_str().is_empty())
        .map(|r| format!("{}/", r.to_string_lossy().replace('\\', "/")))
        .unwrap_or_default();

    let mut source: Option<Source> = None;
    let mut blobs: Option<(Option<String>, Option<String>)> = None;
    match verified_fetch(&url, &p.mirror, &prefix) {
        Ok(f) => {
            let cache = Cache {
                url: url.clone(),
                branch: f.branch.clone(),
                sha: f.sha.clone(),
                fetched_at: now(),
                config_toml: f.config_toml.clone(),
                modes_toml: f.modes_toml.clone(),
            };
            source = Some(Source {
                url: url.clone(),
                branch: f.branch,
                sha: f.sha,
                fetched_at: cache.fetched_at,
                from_cache: false,
            });
            blobs = Some((f.config_toml, f.modes_toml));
            state.cache = Some(cache);
        }
        Err(e) => match state.cache.as_ref().filter(|c| c.url == url) {
            Some(c) => {
                let s = Source {
                    url: url.clone(),
                    branch: c.branch.clone(),
                    sha: c.sha.clone(),
                    fetched_at: c.fetched_at,
                    from_cache: true,
                };
                res.notices.push(format!(
                    "Using project sandbox policy from {} fetched {} (cannot reach the remote: {e:#}).",
                    &s.sha[..s.sha.len().min(12)],
                    fmt_time(s.fetched_at)
                ));
                blobs = Some((c.config_toml.clone(), c.modes_toml.clone()));
                source = Some(s);
            }
            None => {
                res.status = Status::Unavailable {
                    reason: format!("{e:#}"),
                };
            }
        },
    }

    if let (Some(src), Some((cfg, modes))) = (&source, &blobs) {
        match PolicyDocs::parse(cfg.as_deref(), modes.as_deref()) {
            Ok(docs) => {
                res.status = Status::Active;
                res.tracking = Some(format!("{} on {}", src.branch, src.url));
                res.verified = Some(Verified {
                    source: src.clone(),
                    docs,
                });
            }
            Err(e) => {
                res.status = Status::Unavailable {
                    reason: format!("the committed sandbox policy does not parse: {e:#}"),
                };
            }
        }
    }

    let local = working_tree_docs(project);
    let has_policy = res.verified.as_ref().is_some_and(|v| !v.docs.is_empty()) || !local.is_empty();
    if has_policy {
        if let Status::Unavailable { reason } = &res.status {
            res.notices.push(format!(
                "Project sandbox policy is not available ({reason}); ignoring the sandbox settings in the project's .ruckcode files."
            ));
        }
        if let Some(s) = source.as_ref().filter(|_| first_pin || !state.notified) {
            res.notices.push(format!(
                "Project sandbox policy will be taken from {} {} (change the remote with `rc sandbox trust-remote`).",
                s.url, s.branch
            ));
            state.notified = true;
        }
    }
    state.save(&p.state);
    with_local_drift(res, project)
}

fn mismatch_notice(pinned: &str, current: &str) -> String {
    format!(
        "The origin remote is {current} but the project sandbox policy is pinned to {pinned}: ignoring the project's sandbox policy. If the change is legitimate, re-pin it with `rc sandbox trust-remote`."
    )
}

/// Add the drift warning when the working tree's sandbox policy differs from the verified one.
fn with_local_drift(mut res: Resolution, project: &Path) -> Resolution {
    let local = working_tree_docs(project);
    let verified = res
        .verified
        .as_ref()
        .map(|v| v.docs.clone())
        .unwrap_or_default();
    if local != verified {
        res.drift = Some(match &res.tracking {
            Some(t) => format!(
                "The sandbox settings in this working tree differ from the policy committed to {t}: local changes do not take effect until merged to the default branch on the remote."
            ),
            None => "The project's .ruckcode files contain sandbox settings, but no verified remote policy is available: they are ignored.".to_string(),
        });
        // Quiet when there is nothing local to ignore.
        if local.is_empty() && verified.is_empty() {
            res.drift = None;
        }
    }
    if let Some(d) = &res.drift {
        res.notices.push(d.clone());
    }
    res
}

/// `rc sandbox trust-remote`: pin the current `origin` URL (replacing any previous pin and the
/// cached policy). Returns the pinned URL.
pub fn trust_remote(project: &Path, data_dir: &Path) -> Result<String> {
    let root = repo_root(project).ok_or_else(|| anyhow!("not a git repository"))?;
    let url = origin_url(&root).ok_or_else(|| anyhow!("no `origin` remote is configured"))?;
    if !safe_url(&url) {
        bail!("refusing the remote URL `{url}`");
    }
    let p = paths(data_dir, &root);
    let mut state = State::load(&p.state);
    state.repo_root = root.display().to_string();
    state.pinned_url = Some(url.clone());
    state.pinned_at = now();
    state.notified = false;
    state.cache = None;
    state.save(&p.state);
    Ok(url)
}

/// The pinned URL, for `rc sandbox status`.
pub fn pinned_url(project: &Path, data_dir: &Path) -> Option<String> {
    let root = repo_root(project)?;
    State::load(&paths(data_dir, &root).state).pinned_url
}

// ---- applying ----

/// `base` with the verified `[sandbox]` table laid over it (lists replace). `enabled` and
/// `project_policy` always stay as the user set them.
pub fn overlay_sandbox(base: &SandboxConfig, table: &toml::Table) -> Result<SandboxConfig> {
    let toml::Value::Table(mut t) = toml::Value::try_from(base)? else {
        bail!("sandbox config is not a table");
    };
    for (k, v) in table {
        if k != "enabled" && k != "project_policy" {
            t.insert(k.clone(), v.clone());
        }
    }
    toml::Value::Table(t)
        .try_into()
        .context("invalid project [sandbox] policy")
}

/// Apply a resolved policy: the sandbox table onto `config`, the per-mode overrides onto `modes`
/// (over whatever the user's own modes file set). Returns notices for parts that were rejected.
pub fn apply(
    res: &Resolution,
    config: &mut SandboxConfig,
    modes: &mut crate::modes::ModeSet,
) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(v) = &res.verified else {
        return notes;
    };
    if let Some(t) = &v.docs.sandbox {
        match overlay_sandbox(config, t) {
            Ok(c) => *config = c,
            Err(e) => notes.push(format!(
                "Ignoring the committed project [sandbox] policy: {e:#}"
            )),
        }
    }
    for (id, t) in &v.docs.modes {
        let parsed: Result<SandboxOverride, _> = toml::Value::Table(t.clone()).try_into();
        match parsed {
            Ok(o) => {
                if let Some((_, def)) = modes.modes.iter_mut().find(|(i, _)| i == id) {
                    def.sandbox = Some(match &def.sandbox {
                        Some(base) => base.merged_with(&o),
                        None => o,
                    });
                }
            }
            Err(e) => notes.push(format!(
                "Ignoring the committed [modes.{id}.sandbox] policy: {e}"
            )),
        }
    }
    notes
}

#[cfg(test)]
mod tests;
