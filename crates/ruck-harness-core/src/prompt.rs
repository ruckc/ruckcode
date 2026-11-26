//! Generic system prompt assembly.
//!
//! The product supplies the guidance text through a [`PromptProvider`]; this module adds the
//! environment section, the current mode, the list of modes and the client's appended
//! instructions, in that order.

use async_trait::async_trait;
use std::path::Path;

use ruck_harness_tools::Tool;
use std::sync::Arc;

use crate::config::Profile;
use crate::modes::ModeSet;

/// What a [`PromptProvider`] may look at.
pub struct PromptCtx<'a> {
    pub cwd: &'a Path,
    pub mode_id: &'a str,
    pub modes: &'a ModeSet,
    pub profile: Option<&'a Profile>,
    /// Whether the session offers the built-in tool called `name` (a model must not be told to
    /// use a tool it cannot call).
    pub offers: &'a (dyn Fn(&str) -> bool + Sync),
}

/// The product-specific parts of the system prompt.
#[async_trait]
pub trait PromptProvider: Send + Sync {
    /// The opening guidance, before the environment section.
    async fn base(&self, ctx: &PromptCtx<'_>) -> String;

    /// Extra lines (each ending in a newline) at the end of the environment section.
    async fn environment_extra(&self, _ctx: &PromptCtx<'_>) -> String {
        String::new()
    }

    /// Whole sections (each starting with a blank line) between the environment and the mode.
    async fn project_sections(&self, _ctx: &PromptCtx<'_>) -> String {
        String::new()
    }
}

/// Longest accepted client instruction text, in bytes.
pub const MAX_CLIENT_INSTRUCTIONS: usize = 64 * 1024;

/// Build the system prompt: the provider's guidance, then the environment, the provider's
/// project sections and the current mode, then the client's appended instructions (which never
/// replace anything above).
pub async fn system_prompt(
    provider: &dyn PromptProvider,
    builtin_tool: &(dyn Fn(&str) -> Option<Arc<dyn Tool>> + Sync),
    cwd: &Path,
    modes: &ModeSet,
    mode_id: &str,
    client_instructions: Option<&str>,
    profile: Option<&Profile>,
) -> String {
    let offers = |name: &str| {
        profile.is_none_or(|p| {
            let class = builtin_tool(name)
                .map(|t| t.lean_class())
                .unwrap_or_default();
            p.allows_class(name, class)
        })
    };
    let ctx = PromptCtx {
        cwd,
        mode_id,
        modes,
        profile,
        offers: &offers,
    };
    let mut s = provider.base(&ctx).await;
    s.push_str("\n\n## Environment\n");
    s.push_str(&format!("- Working directory: {}\n", cwd.display()));
    s.push_str(&format!("- Platform: {}\n", std::env::consts::OS));
    s.push_str(&format!(
        "- Today's date: {}\n",
        chrono::Utc::now().format("%Y-%m-%d")
    ));
    s.push_str(&provider.environment_extra(&ctx).await);
    s.push_str(&provider.project_sections(&ctx).await);
    if let Some(mode) = modes.get(mode_id) {
        s.push_str(&format!(
            "\n## Current mode: {}\n{}\n{}\n",
            modes.display_name(mode_id),
            mode.description,
            mode.prompt
        ));
    }
    if modes.modes.len() > 1 {
        s.push_str("\n## Modes\n");
        for (id, m) in &modes.modes {
            s.push_str(&format!("- {id}: {}\n", m.description));
        }
    }
    if let Some(c) = client_instructions.map(str::trim).filter(|c| !c.is_empty()) {
        s.push_str(&format!(
            "\n## Client instructions\nThe client that started this session supplied the following \
instructions. They add to, and never override, the guidance above.\n\n{c}\n"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::ModeSchema;

    struct Toy;

    #[async_trait]
    impl PromptProvider for Toy {
        async fn base(&self, _: &PromptCtx<'_>) -> String {
            "TOY BASE".into()
        }
    }

    /// The assembly alone, with a provider that adds nothing, has no product text.
    #[tokio::test]
    async fn the_assembly_adds_only_generic_sections() {
        let modes = ModeSet::implicit_with(&ModeSchema::NEUTRAL);
        let s = system_prompt(
            &Toy,
            &|_| None,
            Path::new("/w"),
            &modes,
            &modes.default_mode,
            Some(" hi "),
            None,
        )
        .await;
        assert!(s.starts_with("TOY BASE\n\n## Environment\n- Working directory: /w\n"));
        assert!(s.contains("## Current mode:"));
        assert!(s.ends_with("add to, and never override, the guidance above.\n\nhi\n"));
        assert!(!s.contains("ruckcode") && !s.contains("Project instructions"));
    }
}
