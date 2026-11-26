//! The transcript browser API of the ruckcode product: `ruck-harness-core`'s, with the project
//! lookups bound to the ruckcode identity so callers keep passing just the project.

use anyhow::Result;
use ruck_harness_core::transcript_api as generic;
use std::path::Path;

use crate::identity::RUCKCODE;

pub use generic::*;

/// Stop offering `id` for resumption (appends `session_dismissed`).
pub fn dismiss(project: &Path, id: &str) -> Result<()> {
    generic::dismiss(&RUCKCODE, project, id)
}

/// Sessions of the project, most recently updated first.
pub fn list(project: &Path, query: Option<&str>) -> Result<Vec<SessionItem>> {
    generic::list(&RUCKCODE, project, query)
}

/// A page of a transcript's records plus its header.
pub fn read(project: &Path, id: &str, opts: ReadOptions) -> Result<ReadResult> {
    generic::read(&RUCKCODE, project, id, opts)
}

/// Case-insensitive search over the project's transcripts.
pub fn search(project: &Path, text: &str, limit: Option<usize>) -> Result<Vec<SearchHit>> {
    generic::search(&RUCKCODE, project, text, limit)
}
