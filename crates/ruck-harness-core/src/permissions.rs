//! Tool permission resolution.

use globset::Glob;
use ruck_harness_tools::Tool;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::config::HarnessConfig;
use crate::modes::ModeDef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Allow,
    Ask,
    Deny,
}

fn lookup(map: &BTreeMap<String, Permission>, tool: &str) -> Option<Permission> {
    if let Some(p) = map.get(tool) {
        return Some(*p);
    }
    map.iter().find_map(|(pat, p)| {
        Glob::new(pat)
            .ok()
            .filter(|g| g.compile_matcher().is_match(tool))
            .map(|_| *p)
    })
}

/// Mode rules, then config rules (exact names before globs), then the default for the tool: its
/// kind, or the tool opting in with [`Tool::auto_allow`].
pub fn resolve(tool: &dyn Tool, mode: &ModeDef, config: &HarnessConfig) -> Permission {
    let name = tool.name();
    lookup(&mode.permissions, name)
        .or_else(|| lookup(&config.permissions, name))
        .unwrap_or_else(|| {
            if tool.kind().is_read_only() || tool.auto_allow() {
                Permission::Allow
            } else {
                Permission::Ask
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ruck_harness_tools::{ToolContext, ToolKind, ToolOutput};
    use serde_json::{json, Value};

    /// A tool that only has a name, a kind and whether it opts in with `auto_allow`.
    struct Stub(String, ToolKind, bool);

    #[async_trait]
    impl Tool for Stub {
        fn name(&self) -> &str {
            &self.0
        }
        fn description(&self) -> String {
            String::new()
        }
        fn input_schema(&self) -> Value {
            json!({})
        }
        fn kind(&self) -> ToolKind {
            self.1
        }
        fn auto_allow(&self) -> bool {
            self.2
        }
        async fn call(&self, _: Value, _: &ToolContext) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok(""))
        }
    }

    /// Resolve a stub tool `name` of `kind`.
    fn resolve(name: &str, kind: ToolKind, mode: &ModeDef, config: &HarnessConfig) -> Permission {
        super::resolve(&Stub(name.into(), kind, false), mode, config)
    }

    fn mode(perms: &str) -> ModeDef {
        toml::from_str(&format!("[permissions]\n{perms}")).unwrap()
    }

    #[test]
    fn precedence() {
        let cfg =
            HarnessConfig::from_toml_str("[permissions]\nBash = \"allow\"\n\"mise__*\" = \"deny\"")
                .unwrap();
        let m = mode("Bash = \"ask\"\n\"mise__t*\" = \"allow\"");
        assert_eq!(
            resolve("Bash", ToolKind::Execute, &m, &cfg),
            Permission::Ask
        );
        assert_eq!(
            resolve("mise__test", ToolKind::Execute, &m, &cfg),
            Permission::Allow
        );
        assert_eq!(
            resolve("mise__build", ToolKind::Execute, &m, &cfg),
            Permission::Deny
        );
        let none = mode("");
        assert_eq!(
            resolve("Bash", ToolKind::Execute, &none, &cfg),
            Permission::Allow
        );
    }

    #[test]
    fn defaults() {
        let cfg = HarnessConfig::default();
        let m = mode("");
        assert_eq!(resolve("Read", ToolKind::Read, &m, &cfg), Permission::Allow);
        assert_eq!(resolve("Write", ToolKind::Edit, &m, &cfg), Permission::Ask);
        // A tool that is not read-only asks unless it opts in with `auto_allow`.
        assert_eq!(resolve("Other", ToolKind::Other, &m, &cfg), Permission::Ask);
        assert_eq!(
            super::resolve(&Stub("Plan".into(), ToolKind::Other, true), &m, &cfg),
            Permission::Allow
        );
    }

    #[test]
    fn mcp_globs() {
        let cfg =
            HarnessConfig::from_toml_str("[permissions]\n\"mcp__github__*\" = \"allow\"").unwrap();
        let m = mode("\"mcp__github__delete*\" = \"deny\"");
        let r = |t, k| resolve(t, k, &m, &cfg);
        assert_eq!(r("mcp__github__search", ToolKind::Other), Permission::Allow);
        assert_eq!(
            r("mcp__github__delete_repo", ToolKind::Other),
            Permission::Deny
        );
        // Unlisted MCP tools ask unless annotated read-only.
        assert_eq!(r("mcp__other__x", ToolKind::Other), Permission::Ask);
        assert_eq!(r("mcp__other__x", ToolKind::Read), Permission::Allow);
    }
}
