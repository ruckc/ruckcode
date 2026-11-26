//! Session transcripts of the ruckcode product: the generic format from `ruck-harness-core`, with
//! the path helpers bound to the ruckcode identity so callers keep passing just the project.

use ruck_harness_core::transcript as generic;
use std::path::{Path, PathBuf};

use crate::identity::RUCKCODE;

pub use generic::*;

/// `<project>/.ruckcode/sessions`.
pub fn sessions_dir(project: &Path) -> PathBuf {
    generic::sessions_dir(&RUCKCODE, project)
}

pub fn session_path(project: &Path, id: &str) -> PathBuf {
    generic::session_path(&RUCKCODE, project, id)
}

/// Sessions stored in the project, newest first.
pub fn list_sessions(project: &Path) -> Vec<SessionSummary> {
    generic::list_sessions(&RUCKCODE, project)
}
