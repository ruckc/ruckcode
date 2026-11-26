//! Project sandbox policy of the ruckcode product: `ruck-harness-core`'s, with the lookups bound
//! to the ruckcode identity (`.ruckcode/config.toml`, `rc sandbox trust-remote`).

use ruck_harness_sandbox::ProjectPolicy;
use std::path::Path;

use crate::identity::RUCKCODE;

pub use ruck_harness_core::project_policy::*;

/// [`ruck_harness_core::project_policy::resolve`] for ruckcode.
pub fn resolve(project: &Path, data_dir: &Path, mode: ProjectPolicy) -> Resolution {
    ruck_harness_core::project_policy::resolve(&RUCKCODE, project, data_dir, mode)
}

/// [`ruck_harness_core::project_policy::working_tree_docs`] for ruckcode.
pub fn working_tree_docs(project: &Path) -> PolicyDocs {
    ruck_harness_core::project_policy::working_tree_docs(&RUCKCODE, project)
}
