//! The concrete policy handed to the launcher (as JSON over an inherited fd) and its builder.

use crate::config::{NetworkMode, SandboxConfig, DEFAULT_DENY_READ};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Which enforcement layers the launcher must apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layers {
    pub landlock: bool,
    /// Landlock TCP connect/bind rules (ABI >= 4).
    pub landlock_net: bool,
    pub seccomp: bool,
    /// Block UDP/raw/packet sockets in seccomp (off for `network = "full"`).
    pub seccomp_net: bool,
    pub namespaces: bool,
    /// A fresh network namespace (loopback only), with a forwarder to the proxy.
    pub netns: bool,
    pub seatbelt: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub version: u32,
    pub cwd: PathBuf,
    pub project_root: PathBuf,
    /// Read + execute, recursively.
    pub read_paths: Vec<PathBuf>,
    /// Full access, recursively (files or directories).
    pub write_paths: Vec<PathBuf>,
    /// Top-level symlinks to recreate in a namespace root: (link, target).
    pub symlinks: Vec<(PathBuf, PathBuf)>,
    /// Files/directories made unreadable (and unwritable) by masking: namespaces and Seatbelt only.
    pub mask_paths: Vec<PathBuf>,
    /// Paths inside writable roots that must stay read-only (ruckcode's own config).
    pub protected_ro: Vec<PathBuf>,
    /// Paths that are never readable (Seatbelt deny rules; Landlock/namespaces exclude them).
    pub deny_read: Vec<PathBuf>,
    pub network: NetworkMode,
    /// The proxy port on loopback (host side; inside a netns the forwarder listens on it too).
    pub proxy_port: Option<u16>,
    /// TCP ports Landlock lets the command connect to.
    pub connect_ports: Vec<u16>,
    /// Host unix socket of the proxy (bind-mounted into the namespace).
    pub proxy_socket: Option<PathBuf>,
    /// Session directory (host) holding the namespace root mountpoint.
    pub stage_dir: Option<PathBuf>,
    /// Host directory bound over `/tmp` in namespace mode.
    pub private_tmp: Option<PathBuf>,
    pub layers: Layers,
}

impl Policy {
    pub const VERSION: u32 = 1;
}

pub struct BuildInput<'a> {
    pub cfg: &'a SandboxConfig,
    pub cwd: &'a Path,
    pub project_root: &'a Path,
    pub home: Option<PathBuf>,
    /// ruckcode's user config directory (its `credentials.toml` is never readable).
    pub config_dir: Option<PathBuf>,
    pub layers: Layers,
    pub proxy_port: Option<u16>,
    pub proxy_socket: Option<PathBuf>,
    pub stage_dir: Option<PathBuf>,
    pub private_tmp: Option<PathBuf>,
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Expand `~` and project-relative paths.
pub fn expand_path(s: &str, home: Option<&Path>, project: &Path) -> PathBuf {
    if s == "~" {
        return home.map(Path::to_path_buf).unwrap_or_else(|| "/".into());
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return match home {
            Some(h) => h.join(rest),
            None => PathBuf::from("/nonexistent-home").join(rest),
        };
    }
    let p = Path::new(s);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        project.join(p)
    }
}

/// Landlock only allows; to exclude `denies` beneath `root` we allow its children one by one.
/// New entries created later directly under such a directory are not covered (documented).
pub fn expand_excluding(root: &Path, denies: &[PathBuf], out: &mut Vec<PathBuf>) {
    if denies.iter().any(|d| d == root) {
        return;
    }
    if !denies.iter().any(|d| d.starts_with(root)) {
        out.push(root.to_path_buf());
        return;
    }
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        expand_excluding(&e.path(), denies, out);
    }
}

#[cfg(target_os = "linux")]
const SYSTEM_DIRS: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/etc", "/opt", "/var",
    "/proc", "/sys", "/dev", "/snap", "/nix", "/gnu",
];
#[cfg(not(target_os = "linux"))]
const SYSTEM_DIRS: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/etc",
    "/opt",
    "/var",
    "/dev",
    "/System",
    "/Library",
    "/Applications",
    "/private",
    "/nix",
];

/// Device nodes (and dirs) commands may write to.
pub const WRITABLE_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/shm",
    "/dev/ptmx",
    "/dev/pts",
];

/// Project `.env*` files (not `.env.example` and friends), depth-limited.
pub fn find_env_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 5 || out.len() >= 200 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if matches!(
                    name.as_str(),
                    ".git" | "node_modules" | "target" | ".venv" | "venv" | ".tox" | "dist"
                ) {
                    continue;
                }
                walk(&e.path(), depth + 1, out);
            } else if (name == ".env" || name.starts_with(".env."))
                && !(name.ends_with(".example")
                    || name.ends_with(".sample")
                    || name.ends_with(".template")
                    || name.ends_with(".dist"))
            {
                out.push(e.path());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

/// The paths whose contents are never readable (defaults, ruckcode credentials, user list).
pub fn deny_list(
    cfg: &SandboxConfig,
    home: Option<&Path>,
    project: &Path,
    config_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut denies: Vec<PathBuf> = DEFAULT_DENY_READ
        .iter()
        .map(|s| canon(&expand_path(s, home, project)))
        .collect();
    if let Some(cd) = config_dir {
        denies.push(canon(&cd.join("credentials.toml")));
    }
    denies.extend(
        cfg.deny_read_paths
            .iter()
            .map(|s| canon(&expand_path(s, home, project))),
    );
    denies.sort();
    denies.dedup();
    denies
}

/// What happened to one configured writable path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathState {
    /// Exists and is writable inside the sandbox.
    Active,
    /// Did not exist; was created (cache dirs only) and is writable.
    Created,
    /// Does not exist and is not writable.
    Missing,
    /// Deliberately not used (unsafe, irrelevant on this platform, or could not be created).
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathStatus {
    /// `"cache"` or `"extra_write"`.
    pub kind: String,
    /// The entry as configured.
    pub entry: String,
    pub path: PathBuf,
    pub state: PathState,
    pub reason: Option<String>,
}

impl PathStatus {
    pub fn writable(&self) -> bool {
        matches!(self.state, PathState::Active | PathState::Created)
    }

    /// A note for the model/user when something needs attention (or was done on their behalf).
    pub fn note(&self) -> Option<String> {
        let why = self.reason.as_deref().unwrap_or("");
        match (self.kind.as_str(), self.state) {
            (_, PathState::Created) => Some(format!(
                "[sandbox] created missing tool cache directory {} (sandbox.create_cache_dirs).",
                self.path.display()
            )),
            ("extra_write", PathState::Missing | PathState::Skipped) => Some(format!(
                "[sandbox] extra_write_paths entry {} does not exist; it is not writable ({why}).",
                self.entry
            )),
            ("cache", PathState::Skipped) => Some(format!(
                "[sandbox] cache path {} is not writable: {why}.",
                self.entry
            )),
            _ => None,
        }
    }

    /// Whether the note reports a problem (as opposed to a courtesy).
    pub fn is_problem(&self) -> bool {
        matches!(self.state, PathState::Missing | PathState::Skipped) && self.note().is_some()
    }
}

#[cfg(unix)]
fn owned_by_me(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions.
    m.uid() == unsafe { libc::geteuid() }
}
#[cfg(not(unix))]
fn owned_by_me(_: &std::fs::Metadata) -> bool {
    true
}

fn make_private_dir(leaf: &Path) -> std::io::Result<()> {
    if let Some(parent) = leaf.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    match b.create(leaf) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        r => r,
    }
}

/// Resolve a cache entry that lives under `$HOME` (`rel` is relative to the canonical `home`),
/// creating it when `create` is set and it is safe to.
fn resolve_home_cache(
    entry: &str,
    rel: &Path,
    home: &Path,
    create: bool,
) -> (PathBuf, PathState, Option<String>) {
    let skip = |p: PathBuf, why: String| (p, PathState::Skipped, Some(why));
    let mut cur = home.to_path_buf();
    let comps: Vec<_> = rel.components().collect();
    for (i, comp) in comps.iter().enumerate() {
        let Component::Normal(name) = comp else {
            return skip(cur, format!("unsupported path component in {entry}"));
        };
        let next = cur.join(name);
        match std::fs::symlink_metadata(&next) {
            Ok(m) if m.file_type().is_symlink() => match next.canonicalize() {
                Ok(t) if t.starts_with(home) => cur = t,
                Ok(t) => {
                    return skip(
                        next,
                        format!(
                            "it is a symlink leaving $HOME (to {}); list the target in sandbox.extra_write_paths if that is intended",
                            t.display()
                        ),
                    )
                }
                Err(_) => return skip(next, "it is a dangling symlink".into()),
            },
            Ok(_) => cur = next,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut leaf = next;
                for c in &comps[i + 1..] {
                    leaf.push(c);
                }
                if !create {
                    return (
                        leaf,
                        PathState::Missing,
                        Some("does not exist; created on the first sandboxed command".into()),
                    );
                }
                return match make_private_dir(&leaf) {
                    Ok(()) => (leaf, PathState::Created, None),
                    Err(e) => skip(leaf, format!("could not create it: {e}")),
                };
            }
            Err(e) => return skip(next, format!("cannot inspect it: {e}")),
        }
    }
    (cur, PathState::Active, None)
}

/// The state of every configured cache path and `extra_write_paths` entry. With `create`, missing
/// cache directories under `$HOME` are created (owner-only); nothing else is ever created.
pub fn path_report(
    cfg: &SandboxConfig,
    home: Option<&Path>,
    project: &Path,
    create: bool,
) -> Vec<PathStatus> {
    let canon_home = home
        .map(canon)
        .filter(|h| h.is_dir() && h != Path::new("/"));
    let owned = canon_home
        .as_ref()
        .and_then(|h| std::fs::metadata(h).ok())
        .is_some_and(|m| owned_by_me(&m));
    let mut out = Vec::new();
    for c in &cfg.cache_paths {
        let p = expand_path(c, home, project);
        let mk = |path: PathBuf, state, reason: Option<String>| PathStatus {
            kind: "cache".into(),
            entry: c.clone(),
            path,
            state,
            reason,
        };
        let existing = |p: &Path, why: &str| {
            if p.exists() {
                mk(canon(p), PathState::Active, None)
            } else {
                mk(p.to_path_buf(), PathState::Missing, Some(why.to_string()))
            }
        };
        if !cfg.create_cache_dirs {
            out.push(existing(
                &p,
                "does not exist (sandbox.create_cache_dirs = false)",
            ));
            continue;
        }
        let rel = canon_home.as_ref().and_then(|ch| {
            [Some(ch.as_path()), home]
                .into_iter()
                .flatten()
                .find_map(|h| p.strip_prefix(h).ok())
                .filter(|r| !r.as_os_str().is_empty())
                .map(Path::to_path_buf)
        });
        let Some(rel) = rel else {
            out.push(existing(
                &p,
                "does not exist and is outside $HOME, so it is not created",
            ));
            continue;
        };
        let ch = canon_home.as_ref().unwrap();
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            out.push(mk(p, PathState::Skipped, Some("contains `..`".into())));
            continue;
        }
        let target = ch.join(&rel);
        if rel.starts_with("Library")
            && !cfg!(target_os = "macos")
            && std::fs::symlink_metadata(&target).is_err()
        {
            out.push(mk(
                target,
                PathState::Skipped,
                Some("macOS-only location; not created on this platform".into()),
            ));
            continue;
        }
        let missing = std::fs::symlink_metadata(&target).is_err();
        if create && missing && !owned {
            out.push(mk(
                target,
                PathState::Skipped,
                Some("does not exist and $HOME is not owned by the current user, so it is not created".into()),
            ));
            continue;
        }
        let (path, state, reason) = resolve_home_cache(c, &rel, ch, create);
        out.push(mk(path, state, reason));
    }
    for s in &cfg.extra_write_paths {
        let p = canon(&expand_path(s, home, project));
        let (state, reason) = if p.exists() {
            (PathState::Active, None)
        } else {
            (
                PathState::Missing,
                Some("create it, or remove the entry".to_string()),
            )
        };
        out.push(PathStatus {
            kind: "extra_write".into(),
            entry: s.clone(),
            path: p,
            state,
            reason,
        });
    }
    out
}

pub fn build_policy(i: &BuildInput) -> Policy {
    build_policy_report(i).0
}

/// Creates missing cache directories (in the calling, unsandboxed process) and builds the policy.
pub fn build_policy_report(i: &BuildInput) -> (Policy, Vec<PathStatus>) {
    let project = canon(i.project_root);
    let report = path_report(i.cfg, i.home.as_deref(), &project, true);
    (build_policy_with(i, &report), report)
}

fn build_policy_with(i: &BuildInput, report: &[PathStatus]) -> Policy {
    let home = i.home.as_deref();
    let project = canon(i.project_root);
    let cwd = canon(i.cwd);
    let exp = |s: &String| canon(&expand_path(s, home, &project));

    let denies = deny_list(i.cfg, home, &project, i.config_dir.as_deref());
    // Only denies that exist matter for expansion (others would split directories for nothing).
    let existing_denies: Vec<PathBuf> = denies.iter().filter(|d| d.exists()).cloned().collect();

    let ns = i.layers.namespaces;
    let mut symlinks = Vec::new();
    let mut read_paths: Vec<PathBuf> = Vec::new();
    for d in SYSTEM_DIRS {
        let p = Path::new(d);
        // Kernel-facing views are handled by the namespace builder itself.
        let Ok(meta) = std::fs::symlink_metadata(p) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            if let Ok(t) = std::fs::read_link(p) {
                symlinks.push((p.to_path_buf(), t));
            }
            continue;
        }
        expand_excluding(p, &existing_denies, &mut read_paths);
    }
    // $HOME: everything except the secrets.
    if let Some(h) = home.map(canon) {
        if h.is_dir() && h != Path::new("/") {
            expand_excluding(&h, &existing_denies, &mut read_paths);
        }
    }
    for s in &i.cfg.extra_read_paths {
        let p = exp(s);
        if p.exists() {
            expand_excluding(&p, &existing_denies, &mut read_paths);
        }
    }

    // Writable: project, temp, caches, devices, extras.
    let mut write_roots: Vec<PathBuf> = vec![project.clone()];
    if !cwd.starts_with(&project) {
        write_roots.push(cwd.clone());
    }
    let mut temps = vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")];
    if !(ns && i.private_tmp.is_some()) {
        if let Some(t) = std::env::var_os("TMPDIR").filter(|v| !v.is_empty()) {
            temps.push(PathBuf::from(t));
        }
    }
    write_roots.extend(temps.iter().map(|t| canon(t)));
    write_roots.extend(
        report
            .iter()
            .filter(|s| s.writable())
            .map(|s| s.path.clone()),
    );
    let mut write_paths: Vec<PathBuf> = Vec::new();
    for r in write_roots {
        if r.exists() {
            expand_excluding(&r, &existing_denies, &mut write_paths);
        }
    }
    for d in WRITABLE_DEVICES {
        let p = PathBuf::from(d);
        if p.exists() {
            write_paths.push(p);
        }
    }
    write_paths.sort();
    write_paths.dedup();
    read_paths.sort();
    read_paths.dedup();
    // /run (sockets such as docker.sock, ssh agents) is never offered; /proc, /sys, /dev are
    // not written to by the model's commands except for the device nodes above.

    // Config the model must not be able to rewrite.
    let mut protected_ro = Vec::new();
    for rel in [".ruckcode", ".mcp.json"] {
        let p = project.join(rel);
        if p.exists() {
            protected_ro.push(p);
        }
    }

    // Paths that could forge the remote or run code outside the sandbox later. Kernel enforced
    // with namespaces / Seatbelt (existing paths only); Landlock-only mode restores them after
    // each command instead.
    for p in crate::git::git_protected_paths(&project) {
        if p.exists() && !protected_ro.contains(&p) {
            protected_ro.push(p);
        }
    }

    let mut mask_paths = Vec::new();
    if i.cfg.mask_env_files {
        mask_paths.extend(find_env_files(&project));
    }
    for d in &existing_denies {
        // Denies inside a writable root that expansion could not carve out (project root).
        if d.starts_with(&project) && !mask_paths.contains(d) {
            mask_paths.push(d.clone());
        }
    }

    let proxy = match i.cfg.network {
        NetworkMode::Allowlist => i.proxy_port,
        _ => None,
    };
    let mut connect_ports: Vec<u16> = proxy.into_iter().collect();
    if i.cfg.network != NetworkMode::Off {
        connect_ports.extend(i.cfg.allowed_ports.iter().copied());
    }
    let mut policy = Policy {
        version: Policy::VERSION,
        cwd,
        project_root: project,
        read_paths,
        write_paths,
        symlinks,
        mask_paths,
        protected_ro,
        deny_read: denies,
        network: i.cfg.network,
        proxy_port: proxy,
        connect_ports,
        proxy_socket: i.proxy_socket.clone(),
        stage_dir: i.stage_dir.clone(),
        private_tmp: if ns && i.cfg.private_tmp {
            i.private_tmp.clone()
        } else {
            None
        },
        layers: i.layers,
    };
    if let Some(s) = &policy.stage_dir {
        // The session directory itself is private to the parent.
        policy.write_paths.retain(|p| !p.starts_with(s));
    }
    policy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(cfg: &'a SandboxConfig, cwd: &'a Path, home: &Path) -> BuildInput<'a> {
        BuildInput {
            cfg,
            cwd,
            project_root: cwd,
            home: Some(home.to_path_buf()),
            config_dir: None,
            layers: Layers::default(),
            proxy_port: Some(4321),
            proxy_socket: None,
            stage_dir: None,
            private_tmp: None,
        }
    }

    #[test]
    fn secrets_excluded_and_project_writable() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        for d in [".ssh", ".config/gh", ".config/other", ".cache", "work"] {
            std::fs::create_dir_all(home.path().join(d)).unwrap();
        }
        std::fs::write(home.path().join(".netrc"), "x").unwrap();
        std::fs::write(proj.path().join(".env"), "S=1").unwrap();
        std::fs::write(proj.path().join(".env.example"), "S=").unwrap();
        std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
        let cfg = SandboxConfig::default();
        let p = build_policy(&input(&cfg, proj.path(), home.path()));
        let h = canon(home.path());
        assert!(!p.read_paths.contains(&h));
        assert!(!p.read_paths.iter().any(|r| r.starts_with(h.join(".ssh"))));
        assert!(!p
            .read_paths
            .iter()
            .any(|r| r.starts_with(h.join(".config/gh"))));
        assert!(!p.read_paths.contains(&h.join(".netrc")));
        assert!(p.read_paths.contains(&h.join(".config/other")));
        assert!(p.read_paths.contains(&h.join("work")));
        assert!(p.write_paths.contains(&h.join(".cache")));
        let proj_c = canon(proj.path());
        assert!(p.write_paths.contains(&proj_c));
        assert_eq!(p.mask_paths, [proj_c.join(".env")]);
        assert_eq!(p.protected_ro, [proj_c.join(".ruckcode")]);
        assert_eq!(p.connect_ports, [4321]);
    }

    #[test]
    fn network_modes_and_extras() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let mut cfg = SandboxConfig {
            extra_write_paths: vec![extra.path().display().to_string()],
            allowed_ports: vec![5432],
            ..Default::default()
        };
        let p = build_policy(&input(&cfg, proj.path(), home.path()));
        assert!(p.write_paths.contains(&canon(extra.path())));
        assert_eq!(p.connect_ports, [4321, 5432]);
        cfg.network = NetworkMode::Off;
        let p = build_policy(&input(&cfg, proj.path(), home.path()));
        assert!(p.connect_ports.is_empty() && p.proxy_port.is_none());
    }

    #[test]
    fn policy_json_round_trip() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig::default();
        let p = build_policy(&input(&cfg, proj.path(), home.path()));
        let s = serde_json::to_string(&p).unwrap();
        let q: Policy = serde_json::from_str(&s).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn expand_excluding_splits_only_when_needed() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/secret")).unwrap();
        std::fs::create_dir_all(d.path().join("b")).unwrap();
        let mut out = vec![];
        expand_excluding(d.path(), &[d.path().join("a/secret")], &mut out);
        out.sort();
        assert_eq!(out, [d.path().join("b")]);
        let mut out = vec![];
        expand_excluding(d.path(), &[], &mut out);
        assert_eq!(out, [d.path().to_path_buf()]);
    }

    #[test]
    fn path_expansion() {
        let home = Path::new("/home/u");
        let proj = Path::new("/p");
        assert_eq!(expand_path("~/x", Some(home), proj), Path::new("/home/u/x"));
        assert_eq!(expand_path("rel", Some(home), proj), Path::new("/p/rel"));
        assert_eq!(expand_path("/abs", Some(home), proj), Path::new("/abs"));
    }

    fn cache_cfg(entries: &[&str]) -> SandboxConfig {
        SandboxConfig {
            cache_paths: entries.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn missing_cache_dir_is_created_private_and_writable() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let cfg = cache_cfg(&["~/.npm", "~/.local/share/pnpm"]);
        let (p, report) = build_policy_report(&input(&cfg, proj.path(), home.path()));
        let h = canon(home.path());
        assert!(h.join(".npm").is_dir() && h.join(".local/share/pnpm").is_dir());
        assert!(p.write_paths.contains(&h.join(".npm")));
        assert!(p.write_paths.contains(&h.join(".local/share/pnpm")));
        assert!(report.iter().all(|s| s.state == PathState::Created));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(h.join(".npm"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        // A second build sees them as active.
        let (_, report) = build_policy_report(&input(&cfg, proj.path(), home.path()));
        assert!(report.iter().all(|s| s.state == PathState::Active));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cache_dir_leaving_home_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join(".cache")).unwrap();
        // A symlinked parent: ~/.local -> outside, entry ~/.local/share/pnpm must not be created.
        std::os::unix::fs::symlink(outside.path(), home.path().join(".local")).unwrap();
        let cfg = cache_cfg(&["~/.cache", "~/.local/share/pnpm"]);
        let (p, report) = build_policy_report(&input(&cfg, proj.path(), home.path()));
        assert!(
            report.iter().all(|s| s.state == PathState::Skipped),
            "{report:?}"
        );
        assert!(!p
            .write_paths
            .iter()
            .any(|w| w.starts_with(canon(outside.path()))));
        assert!(!outside.path().join("share").exists());
        assert!(report[0].note().unwrap().contains("not writable"));
    }

    #[test]
    fn entries_outside_home_are_never_created() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let target = other.path().join("cache");
        let cfg = cache_cfg(&[&target.display().to_string(), "~/../escape"]);
        let (p, report) = build_policy_report(&input(&cfg, proj.path(), home.path()));
        assert!(!target.exists());
        assert!(!home.path().parent().unwrap().join("escape").exists());
        assert!(report.iter().all(|s| !s.writable()));
        assert!(!p.write_paths.contains(&target));
    }

    #[test]
    fn create_cache_dirs_off_keeps_old_behaviour() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".cargo")).unwrap();
        let mut cfg = cache_cfg(&["~/.npm", "~/.cargo"]);
        cfg.create_cache_dirs = false;
        let (p, report) = build_policy_report(&input(&cfg, proj.path(), home.path()));
        let h = canon(home.path());
        assert!(!h.join(".npm").exists());
        assert_eq!(report[0].state, PathState::Missing);
        assert!(p.write_paths.contains(&h.join(".cargo")));
        assert!(!p.write_paths.contains(&h.join(".npm")));
    }

    #[test]
    fn platform_irrelevant_cache_dirs_are_skipped() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let cfg = cache_cfg(&["~/Library/Caches"]);
        let (_, report) = build_policy_report(&input(&cfg, proj.path(), home.path()));
        if cfg!(target_os = "macos") {
            assert_eq!(report[0].state, PathState::Created);
        } else {
            assert_eq!(report[0].state, PathState::Skipped);
            assert!(!home.path().join("Library").exists());
        }
    }

    #[test]
    fn missing_extra_write_path_is_reported_not_created() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            cache_paths: vec![],
            extra_write_paths: vec!["~/.config/mytool".into(), "/nonexistent-rc-x".into()],
            ..Default::default()
        };
        let (p, report) = build_policy_report(&input(&cfg, proj.path(), home.path()));
        assert!(!home.path().join(".config/mytool").exists());
        assert_eq!(report.len(), 2);
        for s in &report {
            assert_eq!(s.state, PathState::Missing);
            assert!(s.is_problem());
            assert!(s
                .note()
                .unwrap()
                .contains("does not exist; it is not writable"));
        }
        assert!(report[1].note().unwrap().contains("/nonexistent-rc-x"));
        assert!(!p.write_paths.iter().any(|w| w.ends_with("mytool")));
    }

    #[test]
    fn dry_run_reports_without_creating() {
        let home = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let cfg = cache_cfg(&["~/.npm"]);
        let r = path_report(&cfg, Some(home.path()), proj.path(), false);
        assert_eq!(r[0].state, PathState::Missing);
        assert!(!home.path().join(".npm").exists());
    }
}
