//! Command sandboxing implemented inside the ruckcode binary.
//!
//! The Bash tool (and other process-spawning tools) re-execute the running binary as a hidden
//! launcher (`rc __sandbox-exec`). The launcher restricts itself (user/mount/pid/net namespaces,
//! Landlock and seccomp on Linux; Seatbelt on macOS) and then `exec`s the command, so every
//! descendant inherits the restrictions. Network access goes through an in-process allowlist
//! proxy. No external sandbox binary (bwrap, sandbox-exec, ...) is ever used.

pub mod caps;
pub mod config;
pub mod launcher;
#[cfg(target_os = "linux")]
mod linux;
pub mod macos;
#[cfg(target_os = "linux")]
mod ns;
pub mod policy;
pub mod proxy;
pub mod runner;
pub mod spawn;

pub use caps::{detect, plan, report, Capabilities, Plan};
pub use config::{domain_matches, NamespaceMode, NetworkMode, SandboxConfig, SandboxOverride};
pub use policy::{build_policy, deny_list, BuildInput, Layers, Policy};
pub use proxy::{DomainAnswer, DomainPrompter, DomainStore};
pub use runner::{Prep, Prepared, Sandbox, SandboxOptions};
pub use spawn::LauncherExe;
