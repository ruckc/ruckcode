//! The concrete policy handed to the launcher (as JSON over an inherited fd) and its builder.

use crate::config::{NetworkMode, SandboxConfig, DEFAULT_DENY_READ};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

pub fn build_policy(i: &BuildInput) -> Policy {
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
    for c in &i.cfg.cache_paths {
        let p = exp(c);
        if p.exists() {
            write_roots.push(p);
        }
    }
    for s in &i.cfg.extra_write_paths {
        let p = exp(s);
        if p.exists() {
            write_roots.push(p);
        }
    }
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
}
