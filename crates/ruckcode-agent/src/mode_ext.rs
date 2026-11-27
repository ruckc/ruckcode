//! ruckcode's additions to the generic mode format: the `mise_tasks` key.

use anyhow::{bail, Result};
use std::collections::BTreeMap;

use std::path::Path;

use ruck_harness_core::modes::{ModeDef, ModeSchema, ModeSet};

const MISE_TASKS: &str = "mise_tasks";

fn string_list(v: &toml::Value) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|x| x.as_str().map(str::to_string))
        .collect()
}

fn check(key: &str, v: &toml::Value) -> Result<()> {
    if key == MISE_TASKS && string_list(v).is_none() {
        bail!("`{MISE_TASKS}` must be an array of strings");
    }
    Ok(())
}

fn implicit() -> BTreeMap<String, toml::Value> {
    BTreeMap::from([(
        MISE_TASKS.to_string(),
        toml::Value::Array(vec![toml::Value::String("*".into())]),
    )])
}

/// The schema ruckcode loads modes with. The implicit `default` mode selects every mise task.
pub static RUCKCODE_MODE_SCHEMA: ModeSchema = ModeSchema {
    extra_keys: &[MISE_TASKS],
    check_extra: check,
    implicit_extra: implicit,
    is_builtin_tool: |name| {
        ruckcode_tools::builtin_tools()
            .iter()
            .any(|t| t.name() == name)
    },
};

/// The mode loaders with ruckcode's schema and identity (`ModeSet` is a harness type, so they
/// are a trait; import it to call `ModeSet::load(..)` and friends).
pub trait RuckcodeModeSet: Sized {
    /// A single permissive `default` mode, used when no modes file exists.
    fn implicit() -> Self;

    fn from_toml_str(text: &str) -> Result<Self>;

    /// Project `.ruckcode/modes.toml`, else the user's, else the implicit set (see
    /// [`ModeSet::load_with`]).
    fn load(project_dir: &Path, user_dir: Option<&Path>) -> Result<Self>;
}

impl RuckcodeModeSet for ModeSet {
    fn implicit() -> Self {
        Self::implicit_with(&RUCKCODE_MODE_SCHEMA)
    }

    fn from_toml_str(text: &str) -> Result<Self> {
        Self::from_toml_str_with(text, &RUCKCODE_MODE_SCHEMA)
    }

    fn load(project_dir: &Path, user_dir: Option<&Path>) -> Result<Self> {
        Self::load_with(
            project_dir,
            user_dir,
            &crate::identity::RUCKCODE,
            &RUCKCODE_MODE_SCHEMA,
        )
    }
}

/// Typed access to ruckcode's keys in [`ModeDef::extra`].
pub trait RuckcodeModeExt {
    /// Task selectors (globs over mise task names); empty when unset.
    fn mise_tasks(&self) -> Vec<String>;
}

impl RuckcodeModeExt for ModeDef {
    fn mise_tasks(&self) -> Vec<String> {
        self.extra
            .get(MISE_TASKS)
            .and_then(string_list)
            .unwrap_or_default()
    }
}
