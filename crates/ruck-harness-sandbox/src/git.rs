//! Protecting the parts of `.git` that a sandboxed command could abuse.
//!
//! The project (including `.git`) is writable by sandboxed commands, so without this a command
//! could forge `refs/remotes/*`, redirect `origin` in `.git/config`, or plant a hook (or a
//! `core.fsmonitor`/`core.hooksPath` setting) that runs *unsandboxed* the next time the user runs
//! git. Normal git use (commit, branch, checkout, stash, objects, index, HEAD, `refs/heads`, logs)
//! stays writable.
//!
//! * namespaces / Seatbelt: the paths are made read-only by the kernel (see `Policy::protected_ro`);
//! * Landlock only (cannot carve a subpath out of an allowed directory): [`Snapshot`] restores them
//!   after each command (best effort).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The git directory of `project` (`.git`, or the target of a `.git` file in a worktree/submodule)
/// and the common directory it shares with other worktrees.
fn git_dirs(project: &Path) -> (Option<PathBuf>, Vec<PathBuf>) {
    let dot = project.join(".git");
    let Ok(meta) = std::fs::symlink_metadata(&dot) else {
        return (None, vec![]);
    };
    let mut dirs = Vec::new();
    let mut pointer = None;
    if meta.is_dir() {
        dirs.push(dot);
    } else if meta.is_file() {
        pointer = Some(dot.clone());
        if let Ok(text) = std::fs::read_to_string(&dot) {
            if let Some(rest) = text.trim().strip_prefix("gitdir:") {
                let p = Path::new(rest.trim());
                let gd = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    project.join(p)
                };
                dirs.push(gd);
            }
        }
    }
    if let Some(gd) = dirs.first().cloned() {
        if let Ok(c) = std::fs::read_to_string(gd.join("commondir")) {
            let p = Path::new(c.trim());
            let common = if p.is_absolute() {
                p.to_path_buf()
            } else {
                gd.join(p)
            };
            let common = common.canonicalize().unwrap_or(common);
            if common != gd {
                dirs.push(common);
            }
        }
    }
    (pointer, dirs)
}

/// The git directory that holds the repository-wide config (`.git`, or a worktree's common dir).
pub fn git_common_dir(project: &Path) -> Option<PathBuf> {
    git_dirs(project).1.pop()
}

fn protect_gitdir(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    for rel in [
        "config",
        "config.worktree",
        "hooks",
        "info",
        "refs/remotes",
        "packed-refs",
        "FETCH_HEAD",
    ] {
        out.push(dir.join(rel));
    }
    // Submodule git dirs live under `modules/` and carry their own config and hooks.
    if depth < 4 {
        if let Ok(rd) = std::fs::read_dir(dir.join("modules")) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    protect_gitdir(&e.path(), depth + 1, out);
                }
            }
        }
    }
}

/// Git paths sandboxed commands (and the Write/Edit tools) must not modify: `.git/config`,
/// `.git/config.worktree`, `.git/hooks`, `.git/info`, `.git/refs/remotes`, `.git/packed-refs`,
/// `.git/FETCH_HEAD`, the same for submodules, the real git dirs of a worktree (a `.git` file
/// pointing at one is protected too). Includes paths that do not exist yet.
pub fn git_protected_paths(project: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let (pointer, dirs) = git_dirs(project);
    out.extend(pointer);
    for d in &dirs {
        protect_gitdir(d, 0, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

// ---- restore-after-run (Landlock-only best effort) ----

const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 50_000;

#[derive(Debug, Clone, PartialEq)]
enum Node {
    File {
        data: Vec<u8>,
        mode: u32,
    },
    Link(PathBuf),
    Dir,
    /// Too large to snapshot: left alone.
    Opaque,
}

/// The state of one protected path (a file, or a directory tree) before a command ran.
#[derive(Debug)]
struct Tree {
    root: PathBuf,
    nodes: BTreeMap<PathBuf, Node>,
}

#[cfg(unix)]
fn mode_of(m: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o7777
}
#[cfg(not(unix))]
fn mode_of(_: &std::fs::Metadata) -> u32 {
    0
}

fn capture(path: &Path, nodes: &mut BTreeMap<PathBuf, Node>) {
    if nodes.len() >= MAX_ENTRIES {
        return;
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    let ft = meta.file_type();
    if ft.is_symlink() {
        if let Ok(t) = std::fs::read_link(path) {
            nodes.insert(path.to_path_buf(), Node::Link(t));
        }
    } else if ft.is_dir() {
        nodes.insert(path.to_path_buf(), Node::Dir);
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                capture(&e.path(), nodes);
            }
        }
    } else if ft.is_file() {
        let node = if meta.len() > MAX_FILE {
            Node::Opaque
        } else {
            match std::fs::read(path) {
                Ok(data) => Node::File {
                    data,
                    mode: mode_of(&meta),
                },
                Err(_) => Node::Opaque,
            }
        };
        nodes.insert(path.to_path_buf(), node);
    }
}

fn walk(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    out.push(path.to_path_buf());
    if meta.is_dir() {
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                walk(&e.path(), out);
            }
        }
    }
}

fn is_packed_refs(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n == "packed-refs")
}

/// `packed-refs` entries as (refname, lines), plus the header.
fn parse_packed(text: &str) -> (Vec<String>, Vec<(String, Vec<String>)>) {
    let mut header = Vec::new();
    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') {
            header.push(line.to_string());
        } else if line.starts_with('^') {
            if let Some(last) = entries.last_mut() {
                last.1.push(line.to_string());
            }
        } else if let Some((_, name)) = line.split_once(' ') {
            entries.push((name.to_string(), vec![line.to_string()]));
        }
    }
    (header, entries)
}

fn remote_entries(text: &str) -> Vec<(String, Vec<String>)> {
    parse_packed(text)
        .1
        .into_iter()
        .filter(|(n, _)| n.starts_with("refs/remotes/"))
        .collect()
}

/// `current` with its `refs/remotes/*` entries replaced by those of `saved`. Local refs packed by
/// a legitimate `git gc`/`pack-refs` in the meantime are kept.
fn packed_refs_with_saved_remotes(current: &str, saved: &str) -> String {
    let (header, entries) = parse_packed(current);
    let mut all: Vec<(String, Vec<String>)> = entries
        .into_iter()
        .filter(|(n, _)| !n.starts_with("refs/remotes/"))
        .collect();
    all.extend(remote_entries(saved));
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let mut s = String::new();
    for h in if header.is_empty() {
        vec!["# pack-refs with: peeled fully-peeled sorted ".to_string()]
    } else {
        header
    } {
        s.push_str(&h);
        s.push('\n');
    }
    for (_, lines) in all {
        for l in lines {
            s.push_str(&l);
            s.push('\n');
        }
    }
    s
}

impl Tree {
    fn take(root: &Path) -> Self {
        let mut nodes = BTreeMap::new();
        capture(root, &mut nodes);
        Self {
            root: root.to_path_buf(),
            nodes,
        }
    }

    /// Undo changes; true when something had to be restored.
    fn restore(&self) -> bool {
        let mut changed = false;
        // `packed-refs` is shared with local refs (`git gc` legitimately rewrites it): only the
        // remote-tracking entries are reverted.
        if is_packed_refs(&self.root) {
            let saved = match self.nodes.get(&self.root) {
                Some(Node::File { data, .. }) => String::from_utf8_lossy(data).into_owned(),
                _ => String::new(),
            };
            let now = std::fs::read_to_string(&self.root).unwrap_or_default();
            if remote_entries(&now) != remote_entries(&saved) {
                if now.is_empty() && !self.nodes.contains_key(&self.root) {
                    return false;
                }
                let new = packed_refs_with_saved_remotes(&now, &saved);
                let _ = std::fs::write(&self.root, new);
                changed = true;
            }
            return changed;
        }
        // Things that appeared.
        let mut current = Vec::new();
        walk(&self.root, &mut current);
        current.sort();
        for p in current.iter().rev() {
            if !self.nodes.contains_key(p) {
                if let Ok(m) = std::fs::symlink_metadata(p) {
                    let _ = if m.is_dir() {
                        std::fs::remove_dir_all(p)
                    } else {
                        std::fs::remove_file(p)
                    };
                    changed = true;
                }
            }
        }
        // Things that changed or disappeared (parents first).
        for (p, node) in &self.nodes {
            let m = std::fs::symlink_metadata(p).ok();
            match node {
                Node::Opaque => {}
                Node::Dir => {
                    if !m.as_ref().is_some_and(|m| m.is_dir()) {
                        if m.is_some() {
                            let _ = std::fs::remove_file(p);
                        }
                        let _ = std::fs::create_dir_all(p);
                        changed = true;
                    }
                }
                Node::Link(t) => {
                    if std::fs::read_link(p).ok().as_ref() != Some(t) {
                        remove_any(p, m.as_ref());
                        #[cfg(unix)]
                        let _ = std::os::unix::fs::symlink(t, p);
                        changed = true;
                    }
                }
                Node::File { data, mode } => {
                    let same = m.as_ref().is_some_and(|m| m.is_file())
                        && std::fs::read(p).ok().as_ref() == Some(data)
                        && m.as_ref().map(mode_of) == Some(*mode);
                    if !same {
                        remove_any(p, m.as_ref());
                        let _ = std::fs::write(p, data);
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ =
                                std::fs::set_permissions(p, std::fs::Permissions::from_mode(*mode));
                        }
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}

fn remove_any(p: &Path, m: Option<&std::fs::Metadata>) {
    if let Some(m) = m {
        let _ = if m.is_dir() {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        };
    }
}

/// The protected git paths of a project before a command ran.
#[derive(Debug)]
pub struct Snapshot {
    trees: Vec<Tree>,
}

impl Snapshot {
    pub fn take(project: &Path) -> Self {
        Self {
            trees: git_protected_paths(project)
                .iter()
                .map(|p| Tree::take(p))
                .collect(),
        }
    }

    /// Revert changes made since the snapshot; returns the protected paths that were changed.
    pub fn restore(&self) -> Vec<PathBuf> {
        self.trees
            .iter()
            .filter(|t| t.restore())
            .map(|t| t.root.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let g = d.path().join(".git");
        for sub in ["hooks", "info", "refs/remotes/origin", "refs/heads"] {
            std::fs::create_dir_all(g.join(sub)).unwrap();
        }
        std::fs::write(g.join("config"), "[core]\n").unwrap();
        std::fs::write(g.join("hooks/pre-commit.sample"), "#!/bin/sh\n").unwrap();
        std::fs::write(g.join("refs/remotes/origin/main"), "aaaa\n").unwrap();
        d
    }

    #[test]
    fn protected_paths_cover_the_attack_surface() {
        let d = repo();
        let p = git_protected_paths(d.path());
        let g = d.path().join(".git");
        for rel in ["config", "hooks", "info", "refs/remotes", "packed-refs"] {
            assert!(p.contains(&g.join(rel)), "{rel}");
        }
        assert!(!p.contains(&g.join("refs/heads")));
        assert!(!p.contains(&g.join("index")));
        assert!(git_protected_paths(&d.path().join("nope")).is_empty());
    }

    #[test]
    fn worktree_pointer_and_commondir() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("main/.git");
        let wt = main.join("worktrees/w");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("commondir"), "../..\n").unwrap();
        let proj = d.path().join("w");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join(".git"), format!("gitdir: {}\n", wt.display())).unwrap();
        let p = git_protected_paths(&proj);
        assert!(p.contains(&proj.join(".git")));
        assert!(p.contains(&wt.join("config.worktree")));
        assert!(p.contains(&wt.join("hooks")));
        let common = main.canonicalize().unwrap();
        assert!(p.contains(&common.join("config")));
        assert!(p.contains(&common.join("hooks")));
    }

    #[test]
    fn snapshot_reverts_config_hooks_and_remote_refs() {
        let d = repo();
        let g = d.path().join(".git");
        let snap = Snapshot::take(d.path());
        std::fs::write(g.join("config"), "[core]\nfsmonitor = evil\n").unwrap();
        std::fs::write(g.join("hooks/post-commit"), "evil").unwrap();
        std::fs::remove_file(g.join("hooks/pre-commit.sample")).unwrap();
        std::fs::write(g.join("refs/remotes/origin/main"), "forged\n").unwrap();
        std::fs::write(g.join("refs/remotes/origin/extra"), "forged\n").unwrap();
        std::fs::write(g.join("info/exclude"), "x").unwrap();
        std::fs::write(g.join("packed-refs"), "x").unwrap();
        // local work is untouched
        std::fs::write(g.join("refs/heads/topic"), "bbbb\n").unwrap();
        let changed = snap.restore();
        assert_eq!(changed.len(), 4, "{changed:?}");
        assert_eq!(
            std::fs::read_to_string(g.join("config")).unwrap(),
            "[core]\n"
        );
        assert!(!g.join("hooks/post-commit").exists());
        assert!(g.join("hooks/pre-commit.sample").exists());
        assert_eq!(
            std::fs::read_to_string(g.join("refs/remotes/origin/main")).unwrap(),
            "aaaa\n"
        );
        assert!(!g.join("refs/remotes/origin/extra").exists());
        assert!(!g.join("info/exclude").exists());
        assert!(g.join("refs/heads/topic").exists());
        assert!(snap.restore().is_empty());
    }

    #[test]
    fn packed_refs_only_loses_remote_changes() {
        let d = repo();
        let g = d.path().join(".git");
        let orig = "# pack-refs with: peeled fully-peeled sorted \n1111 refs/heads/a\n2222 refs/remotes/origin/main\n";
        std::fs::write(g.join("packed-refs"), orig).unwrap();
        let snap = Snapshot::take(d.path());
        // `git gc` packs a new local branch (legitimate); a forged remote ref is added too.
        std::fs::write(
            g.join("packed-refs"),
            "# pack-refs with: peeled fully-peeled sorted \n1111 refs/heads/a\n3333 refs/heads/b\n9999 refs/remotes/origin/main\n",
        )
        .unwrap();
        assert_eq!(snap.restore().len(), 1);
        let now = std::fs::read_to_string(g.join("packed-refs")).unwrap();
        assert!(now.contains("3333 refs/heads/b"));
        assert!(now.contains("2222 refs/remotes/origin/main"));
        assert!(!now.contains("9999"));
        // an untouched legit rewrite is not reverted
        let snap = Snapshot::take(d.path());
        assert!(snap.restore().is_empty());
    }
}
