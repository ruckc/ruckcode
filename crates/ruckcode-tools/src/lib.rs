//! The coding agent's built-in tools: file access, search and the shell, registered together
//! with the harness plan, ask and web tools.

mod bash;
mod fs;
mod search;

// Transition shims: the generic tools now live in `ruck-harness-tools` and `ruck-harness-web`.
pub use ruck_harness_tools::*;
pub use ruck_harness_web::*;

pub use bash::Bash;
pub use fs::{Edit, Read, Write};
pub use search::{Glob, Grep};

use serde_json::Value;
use std::sync::Arc;

/// All built-in tools (excluding agent-level tools such as SwitchMode), with the default
/// `[web]` configuration.
pub fn builtin_tools() -> Vec<Arc<dyn Tool>> {
    builtin_tools_with(&WebConfig::default())
}

/// All built-in tools. `WebFetch` and `WebSearch` are left out when `web.enabled` is false and
/// are built from `web` otherwise.
pub fn builtin_tools_with(web: &WebConfig) -> Vec<Arc<dyn Tool>> {
    let mut v: Vec<Arc<dyn Tool>> = vec![
        Arc::new(Read),
        Arc::new(Write),
        Arc::new(Edit),
        Arc::new(Glob),
        Arc::new(Grep),
        Arc::new(Bash),
        Arc::new(TodoWrite),
        Arc::new(AskUserQuestion),
    ];
    v.extend(web_tools(web));
    v
}

/// The built-in tool called `name`, from a registry built once. For looking up what a tool
/// says about itself (its [`LeanClass`], how it decodes a plan) without a session's tool set.
pub fn builtin_tool(name: &str) -> Option<&'static Arc<dyn Tool>> {
    static TOOLS: std::sync::OnceLock<Vec<Arc<dyn Tool>>> = std::sync::OnceLock::new();
    TOOLS
        .get_or_init(builtin_tools)
        .iter()
        .find(|t| t.name() == name)
}

/// The plan a recorded call to the built-in tool `name` wrote, if that tool maintains a plan.
/// Transcripts keep only tool names and inputs, so replaying one decodes plans through here
/// when the session's own tool set (the preferred source) does not offer the tool.
pub fn decode_builtin_plan(name: &str, input: &Value) -> Option<Vec<PlanItem>> {
    builtin_tool(name)?.plan(input)
}

#[cfg(test)]
mod web_registration_tests {
    use super::*;

    #[test]
    fn web_tools_are_registered_unless_disabled() {
        let names = |w: &WebConfig| -> Vec<String> {
            builtin_tools_with(w)
                .iter()
                .map(|t| t.name().to_string())
                .collect()
        };
        let on = names(&WebConfig::default());
        assert!(on.contains(&"WebFetch".to_string()) && on.contains(&"WebSearch".to_string()));
        let off = names(&WebConfig {
            enabled: false,
            ..WebConfig::default()
        });
        assert!(!off.iter().any(|n| n.starts_with("Web")));
        let t = builtin_tools_with(&WebConfig::default());
        let fetch = t.iter().find(|t| t.name() == "WebFetch").unwrap();
        assert_eq!(fetch.kind(), ToolKind::Fetch);
        assert!(fetch.kind().is_read_only());
        assert_eq!(fetch.input_schema()["required"][0], "url");
    }
}

#[cfg(test)]
mod hook_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    /// A tool that overrides none of the hooks.
    struct Plain;

    #[async_trait]
    impl Tool for Plain {
        fn name(&self) -> &str {
            "Plain"
        }
        fn description(&self) -> String {
            String::new()
        }
        fn input_schema(&self) -> Value {
            json!({})
        }
        fn kind(&self) -> ToolKind {
            ToolKind::Other
        }
        async fn call(&self, _: Value, _: &ToolContext) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::ok(""))
        }
    }

    #[test]
    fn hook_defaults() {
        let input = json!({"todos": [{"content": "a", "status": "pending"}]});
        assert_eq!(Plain.plan(&input), None);
        assert!(!Plain.auto_allow());
        assert_eq!(Plain.lean_class(), LeanClass::Keep);
        assert_eq!(LeanClass::default(), LeanClass::Keep);
    }

    #[test]
    fn builtin_hook_overrides() {
        let input = json!({"todos": [
            {"content": "a", "status": "pending"},
            {"content": "b", "status": "completed"}
        ]});
        let items = TodoWrite.plan(&input).unwrap();
        assert_eq!(
            items,
            vec![
                PlanItem {
                    content: "a".into(),
                    status: PlanStatus::Pending
                },
                PlanItem {
                    content: "b".into(),
                    status: PlanStatus::Completed
                }
            ]
        );
        assert_eq!(TodoWrite.plan(&json!({"todos": "x"})), None);
        assert_eq!(TodoWrite.plan(&json!({})), None);
        let class = |n: &str| {
            let t = builtin_tool(n).unwrap();
            (t.auto_allow(), t.lean_class())
        };
        assert_eq!(class("TodoWrite"), (true, LeanClass::DropWhenLean));
        assert_eq!(class("AskUserQuestion"), (true, LeanClass::DropWhenLean));
        assert_eq!(class("WebFetch"), (false, LeanClass::OptInWhenLean));
        assert_eq!(class("WebSearch"), (false, LeanClass::OptInWhenLean));
        for n in ["Read", "Write", "Edit", "Glob", "Grep", "Bash"] {
            assert_eq!(class(n), (false, LeanClass::Keep), "{n}");
        }
    }

    #[test]
    fn decode_builtin_plan_by_name() {
        let input = json!({"todos": [{"content": "a", "status": "in_progress"}]});
        assert_eq!(decode_builtin_plan("TodoWrite", &input).unwrap().len(), 1);
        assert_eq!(decode_builtin_plan("Read", &input), None);
        assert_eq!(decode_builtin_plan("Nope", &input), None);
    }

    #[test]
    fn plan_serde_is_unchanged() {
        let item: PlanItem =
            serde_json::from_value(json!({"content": "x", "status": "in_progress"})).unwrap();
        assert_eq!(item.status.as_str(), "in_progress");
        assert_eq!(
            serde_json::to_value(&item).unwrap(),
            json!({"content": "x", "status": "in_progress"})
        );
        #[allow(deprecated)]
        let _alias: TodoItem = item;
    }
}
