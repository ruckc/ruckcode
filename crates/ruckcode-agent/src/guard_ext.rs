//! ruckcode's hints for the tool-loop guard: which tools edit a file, how a search command is
//! recognised, and how a shell command's effect on the working tree is observed.

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::search_pattern::search_pattern;
use ruck_harness_core::tool_loop_guard::{GuardHints, ProgressProbe};

/// Tools that change one file.
const EDIT_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];
/// The arguments of those tools that hold the file path.
const PATH_KEYS: &[&str] = &["file_path", "path", "notebook_path"];

/// `git status` output larger than this (a huge or very dirty tree) disables the Bash snapshot.
const MAX_STATUS_BYTES: usize = 256 * 1024;
/// At most this many changed files are hashed per Bash call; more disables the snapshot.
const MAX_SNAPSHOT_FILES: usize = 200;
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// The guard hints of the ruckcode coding agent.
pub fn ruckcode_guard_hints() -> GuardHints {
    GuardHints {
        edit_tools: EDIT_TOOLS,
        path_keys: PATH_KEYS,
        search_tool: Some(search_pattern),
        progress_probe: Some(std::sync::Arc::new(GitProbe)),
    }
}

/// Covers `Bash` calls by the files git reports as changed.
pub struct GitProbe;

#[async_trait]
impl ProgressProbe for GitProbe {
    fn watches(&self, tool: &str) -> bool {
        tool == "Bash"
    }

    async fn changed_files(&self, root: &Path) -> Option<Vec<PathBuf>> {
        git_changed_files(root).await
    }
}

async fn git(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = tokio::time::timeout(
        GIT_TIMEOUT,
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    (out.status.success() && out.stdout.len() <= MAX_STATUS_BYTES).then_some(out.stdout)
}

/// Absolute paths that git reports as modified or untracked in the repository containing `cwd`,
/// or `None` if `cwd` is not in a git repository, git is slow or missing, or the tree is too
/// dirty to look at cheaply (status output over 256 KiB, or more than 200 files).
async fn git_changed_files(cwd: &Path) -> Option<Vec<PathBuf>> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).await?;
    let root = PathBuf::from(String::from_utf8_lossy(&root).trim_end());
    let status = git(
        cwd,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ],
    )
    .await?;
    let files: Vec<PathBuf> = status
        .split(|b| *b == 0)
        .filter(|e| e.len() > 3)
        .map(|e| root.join(String::from_utf8_lossy(&e[3..]).as_ref()))
        .collect();
    (files.len() <= MAX_SNAPSHOT_FILES).then_some(files)
}
