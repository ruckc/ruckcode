//! The ruckcode product on the agent harness: its extension (prompt, tools, guard hints), its
//! configuration sections, the setup and mode-editing APIs.
//!
//! The generic engine lives in `ruck-harness-core`; import its types from there. This crate
//! exports only ruckcode-specific items.

pub mod config;
pub mod extension;
pub mod guard_ext;
pub mod identity;
pub mod init;
pub mod keys;
pub mod mode_ext;
pub mod modes_api;
mod modes_edit;
pub mod prompt_ext;
pub mod search_pattern;
pub mod setup;
pub mod setup_api;

pub use config::Config;
pub use extension::RuckcodeExtension;
