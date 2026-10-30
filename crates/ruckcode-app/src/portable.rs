//! Self-update of the portable Windows build (no installer, per-user, no admin).
//!
//! The update is a signed zip (`ruckcode-app-<ver>/ruckcode.exe`, `wsl/rc`, ...) whose signature
//! `tauri-plugin-updater` has already verified. This module only does the filesystem part:
//! extract the zip's *contents* into a staging folder next to the exe, then swap the files into
//! the install folder. Windows cannot overwrite a running exe but can rename it, so every
//! replaced file is renamed to `<name>.old` first. If anything fails, every rename is undone and
//! every new file removed, so the folder always still holds a runnable exe. The `.old` leftovers
//! (the running exe cannot be deleted) are removed on the next start.
//!
//! The logic is plain `std::fs` and platform-agnostic so it is unit tested on Linux.

use std::fs;
use std::io::{self, Cursor};
use std::path::{Component, Path, PathBuf};

/// Marker file shipped in the zip; its presence next to the exe makes this a portable install.
pub const MARKER: &str = "portable.txt";
/// The exe's name inside the zip.
pub const ZIP_EXE: &str = "ruckcode.exe";
/// Suffix of a replaced file kept until the next start.
pub const OLD_SUFFIX: &str = ".old";
/// Staging folder (inside the install folder, so renames never cross volumes).
pub const STAGING: &str = ".update-staging";
/// The updater-manifest platform key of the portable zip.
pub const PLATFORM_KEY: &str = "windows-x86_64-portable";

/// What a successful swap did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Files now at their new version (relative to the install folder).
    pub replaced: Vec<PathBuf>,
    /// Optional resources that could not be replaced (in use); left at the old version.
    pub skipped: Vec<PathBuf>,
}

fn old_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(OLD_SUFFIX);
    PathBuf::from(s)
}

/// `Ok` when files can be created and renamed in `dir`; else a user-facing explanation.
pub fn check_writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    let r = fs::write(&probe, b"x").and_then(|_| {
        let moved = probe.with_extension("moved");
        fs::rename(&probe, &moved)?;
        fs::remove_file(&moved)
    });
    if r.is_err() {
        let _ = fs::remove_file(&probe);
    }
    r.map_err(|e| {
        format!(
            "The folder {} is not writable ({e}), so this portable copy cannot update itself. \
             Move it to a folder you can write to (for example %LocalAppData%\\ruckcode) or \
             download the new zip from the download page.",
            dir.display()
        )
    })
}

/// Extract the zip's contents into `staging`, dropping the single versioned top-level folder
/// (`ruckcode-app-<ver>/`). Entries that would escape `staging` are rejected. Returns the
/// extracted files, relative to `staging`.
pub fn extract_zip(bytes: &[u8], staging: &Path) -> Result<Vec<PathBuf>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("bad zip: {e}"))?;
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| format!("bad zip entry: {e}"))?;
        if f.is_dir() {
            continue;
        }
        let name = f
            .enclosed_name()
            .ok_or_else(|| "unsafe path in zip".to_string())?;
        // Drop the first component: the versioned folder.
        let rel: PathBuf = name.components().skip(1).collect();
        if rel.as_os_str().is_empty()
            || rel.components().any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!("unexpected path in zip: {}", name.display()));
        }
        let dest = staging.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let mut w =
            fs::File::create(&dest).map_err(|e| format!("write {}: {e}", dest.display()))?;
        io::copy(&mut f, &mut w).map_err(|e| format!("extract {}: {e}", rel.display()))?;
        out.push(rel);
    }
    if !out.iter().any(|p| p == Path::new(ZIP_EXE)) {
        return Err(format!("the update zip has no {ZIP_EXE}"));
    }
    Ok(out)
}

/// One completed step, enough to undo it.
struct Step {
    target: PathBuf,
    /// The `.old` the previous file was moved to, if there was a previous file.
    backup: Option<PathBuf>,
}

fn undo(steps: Vec<Step>) {
    for s in steps.into_iter().rev() {
        let _ = fs::remove_file(&s.target);
        if let Some(b) = s.backup {
            let _ = fs::rename(&b, &s.target);
        }
    }
}

fn replace_one(staged: &Path, target: &Path) -> io::Result<Step> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let backup = if target.exists() {
        let b = old_path(target);
        // A leftover from an earlier update may still be locked or gone; try to clear it.
        let _ = fs::remove_file(&b);
        fs::rename(target, &b)?;
        Some(b)
    } else {
        None
    };
    if let Err(e) = fs::rename(staged, target) {
        if let Some(b) = &backup {
            let _ = fs::rename(b, target);
        }
        return Err(e);
    }
    Ok(Step {
        target: target.to_path_buf(),
        backup,
    })
}

/// Swap the staged files into `install_dir`. `exe_rel` is the (relative) path of the exe among
/// the staged files; it is replaced last and a failure there (or on any non-optional file)
/// rolls everything back. Resources in `optional` that cannot be replaced (for example `wsl/rc`
/// held open by WSL) are skipped instead. `portable.txt` already in the folder is kept.
pub fn apply(
    staged_files: &[PathBuf],
    staging: &Path,
    install_dir: &Path,
    exe_rel: &Path,
    optional: &[&Path],
) -> Result<Applied, String> {
    let mut order: Vec<&PathBuf> = staged_files
        .iter()
        .filter(|f| f.as_path() != exe_rel)
        .collect();
    let exe = staged_files
        .iter()
        .find(|f| f.as_path() == exe_rel)
        .ok_or("the update has no executable")?;
    order.push(exe);

    let mut steps: Vec<Step> = Vec::new();
    let mut done = Applied::default();
    for rel in order {
        let target = install_dir.join(rel);
        if rel.as_path() == Path::new(MARKER) && target.exists() {
            continue;
        }
        match replace_one(&staging.join(rel), &target) {
            Ok(step) => {
                steps.push(step);
                done.replaced.push(rel.clone());
            }
            Err(_) if optional.contains(&rel.as_path()) => done.skipped.push(rel.clone()),
            Err(e) => {
                undo(steps);
                return Err(format!(
                    "could not replace {}: {e}. Nothing was changed.",
                    rel.display()
                ));
            }
        }
    }
    Ok(done)
}

/// Best-effort removal of `.old` leftovers from an earlier update, and of a stale staging
/// folder. Only the install folder and its `wsl` subfolder are looked at.
pub fn cleanup(install_dir: &Path) {
    for dir in [install_dir.to_path_buf(), install_dir.join("wsl")] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && p.to_string_lossy().ends_with(OLD_SUFFIX) {
                let _ = fs::remove_file(&p);
            }
        }
    }
    let _ = fs::remove_dir_all(install_dir.join(STAGING));
}

/// Full update from verified zip bytes: stage, swap, clean the staging folder.
/// `exe_name` is the running exe's file name (the zip's `ruckcode.exe` is installed under it).
pub fn install_zip(bytes: &[u8], install_dir: &Path, exe_name: &str) -> Result<Applied, String> {
    check_writable(install_dir)?;
    let staging = install_dir.join(STAGING);
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("create {}: {e}", staging.display()))?;
    let result = (|| {
        let mut files = extract_zip(bytes, &staging)?;
        if exe_name != ZIP_EXE {
            fs::rename(staging.join(ZIP_EXE), staging.join(exe_name))
                .map_err(|e| format!("rename {ZIP_EXE}: {e}"))?;
            for f in &mut files {
                if f.as_path() == Path::new(ZIP_EXE) {
                    *f = PathBuf::from(exe_name);
                }
            }
        }
        apply(
            &files,
            &staging,
            install_dir,
            Path::new(exe_name),
            &[Path::new("wsl/rc"), Path::new("wsl\\rc")],
        )
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

/// The running exe's real name: after a swap Windows may report the renamed `<exe>.old`.
pub fn exe_name_of(path: &Path) -> Option<String> {
    let n = path.file_name()?.to_str()?;
    Some(n.strip_suffix(OLD_SUFFIX).unwrap_or(n).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (n, c) in files {
            w.start_file(*n, opts).unwrap();
            w.write_all(c.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap()
    }

    fn install_v1(dir: &Path) {
        fs::create_dir_all(dir.join("wsl")).unwrap();
        fs::write(dir.join("ruckcode.exe"), "exe1").unwrap();
        fs::write(dir.join("wsl/rc"), "rc1").unwrap();
        fs::write(dir.join(MARKER), "marker").unwrap();
        fs::write(dir.join("README.txt"), "readme1").unwrap();
    }

    fn zip_v2() -> Vec<u8> {
        make_zip(&[
            ("ruckcode-app-2.0.0/ruckcode.exe", "exe2"),
            ("ruckcode-app-2.0.0/wsl/rc", "rc2"),
            ("ruckcode-app-2.0.0/portable.txt", "marker2"),
            ("ruckcode-app-2.0.0/README.txt", "readme2"),
        ])
    }

    #[test]
    fn extracts_contents_without_the_versioned_folder() {
        let t = tempfile::tempdir().unwrap();
        let files = extract_zip(&zip_v2(), t.path()).unwrap();
        assert_eq!(files.len(), 4);
        assert_eq!(read(&t.path().join("ruckcode.exe")), "exe2");
        assert_eq!(read(&t.path().join("wsl/rc")), "rc2");
        assert!(!t.path().join("ruckcode-app-2.0.0").exists());
    }

    #[test]
    fn rejects_bad_zips() {
        let t = tempfile::tempdir().unwrap();
        assert!(extract_zip(b"not a zip", t.path()).is_err());
        let no_exe = make_zip(&[("ruckcode-app-2/README.txt", "x")]);
        assert!(extract_zip(&no_exe, t.path())
            .unwrap_err()
            .contains("ruckcode.exe"));
        let evil = make_zip(&[("ruckcode-app-2/../../evil.txt", "x")]);
        assert!(extract_zip(&evil, t.path()).is_err());
        assert!(!t.path().parent().unwrap().join("evil.txt").exists());
    }

    #[test]
    fn install_swaps_files_keeps_marker_and_leaves_old_copies() {
        let t = tempfile::tempdir().unwrap();
        install_v1(t.path());
        let done = install_zip(&zip_v2(), t.path(), "ruckcode.exe").unwrap();
        assert!(done.skipped.is_empty());
        assert_eq!(read(&t.path().join("ruckcode.exe")), "exe2");
        assert_eq!(read(&t.path().join("wsl/rc")), "rc2");
        assert_eq!(read(&t.path().join("README.txt")), "readme2");
        assert_eq!(read(&t.path().join(MARKER)), "marker", "marker is kept");
        assert_eq!(read(&t.path().join("ruckcode.exe.old")), "exe1");
        assert!(!t.path().join(STAGING).exists());
        cleanup(t.path());
        assert!(!t.path().join("ruckcode.exe.old").exists());
        assert!(!t.path().join("wsl/rc.old").exists());
        assert_eq!(read(&t.path().join("ruckcode.exe")), "exe2");
    }

    #[test]
    fn renamed_exe_is_updated_under_its_own_name() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("mine.exe"), "exe1").unwrap();
        install_zip(&zip_v2(), t.path(), "mine.exe").unwrap();
        assert_eq!(read(&t.path().join("mine.exe")), "exe2");
        assert!(!t.path().join("ruckcode.exe").exists());
        assert_eq!(
            exe_name_of(Path::new("C:/x/mine.exe.old")).unwrap(),
            "mine.exe"
        );
        assert_eq!(
            exe_name_of(Path::new("/x/ruckcode.exe")).unwrap(),
            "ruckcode.exe"
        );
    }

    /// A failure on the exe (last step) undoes every earlier replacement.
    #[test]
    fn failure_rolls_back_everything() {
        let t = tempfile::tempdir().unwrap();
        install_v1(t.path());
        let staging = t.path().join(STAGING);
        fs::create_dir_all(&staging).unwrap();
        let files = extract_zip(&zip_v2(), &staging).unwrap();
        // Make the exe step fail: its staged source vanishes.
        fs::remove_file(staging.join("ruckcode.exe")).unwrap();
        let err = apply(&files, &staging, t.path(), Path::new("ruckcode.exe"), &[]).unwrap_err();
        assert!(err.contains("ruckcode.exe"), "{err}");
        assert_eq!(read(&t.path().join("ruckcode.exe")), "exe1");
        assert_eq!(read(&t.path().join("wsl/rc")), "rc1");
        assert_eq!(read(&t.path().join("README.txt")), "readme1");
        assert!(!t.path().join("ruckcode.exe.old").exists());
        assert!(!t.path().join("wsl/rc.old").exists());
        assert!(!t.path().join("README.txt.old").exists());
    }

    /// An optional resource that cannot be replaced is skipped; the rest still updates.
    #[test]
    fn optional_resource_failure_is_skipped() {
        let t = tempfile::tempdir().unwrap();
        install_v1(t.path());
        let staging = t.path().join(STAGING);
        fs::create_dir_all(&staging).unwrap();
        let files = extract_zip(&zip_v2(), &staging).unwrap();
        fs::remove_file(staging.join("wsl/rc")).unwrap();
        let done = apply(
            &files,
            &staging,
            t.path(),
            Path::new("ruckcode.exe"),
            &[Path::new("wsl/rc")],
        )
        .unwrap();
        assert_eq!(done.skipped, vec![PathBuf::from("wsl/rc")]);
        assert_eq!(read(&t.path().join("ruckcode.exe")), "exe2");
        assert_eq!(read(&t.path().join("wsl/rc")), "rc1", "old rc restored");
    }

    #[test]
    fn stale_old_file_does_not_block_a_second_update() {
        let t = tempfile::tempdir().unwrap();
        install_v1(t.path());
        fs::write(t.path().join("ruckcode.exe.old"), "ancient").unwrap();
        install_zip(&zip_v2(), t.path(), "ruckcode.exe").unwrap();
        assert_eq!(read(&t.path().join("ruckcode.exe.old")), "exe1");
    }

    #[test]
    fn unwritable_folder_is_a_clear_error() {
        let t = tempfile::tempdir().unwrap();
        let missing = t.path().join("nope");
        let e = check_writable(&missing).unwrap_err();
        assert!(
            e.contains("not writable") && e.contains("download page"),
            "{e}"
        );
        assert!(install_zip(&zip_v2(), &missing, "ruckcode.exe").is_err());
        assert!(check_writable(t.path()).is_ok());
    }
}
