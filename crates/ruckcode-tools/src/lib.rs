//! Tool abstraction and the built-in tool set.

pub mod ask;
mod bash;
mod fs;
mod sandboxed;
mod search;
mod todo;
pub mod web;

pub use ask::{Answer, AskUserQuestion, Question, QuestionOption};
pub use bash::Bash;
pub use fs::{Edit, Read, Write};
pub use sandboxed::{launch, protected_write_msg, Launch, SandboxHandle};
pub use search::{Glob, Grep};
pub use todo::{PlanItem, PlanStatus, TodoWrite, ALL_DONE as PLAN_ALL_DONE};
#[allow(deprecated)]
pub use todo::{TodoItem, TodoStatus};
pub use web::{WebConfig, WebFetch, WebSearch};

use async_trait::async_trait;
use ruck_harness_ident::AppIdentity;
use ruck_harness_llm::ToolSpec;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Matches the ACP `ToolKind` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolKind::Read => "read",
            ToolKind::Edit => "edit",
            ToolKind::Delete => "delete",
            ToolKind::Move => "move",
            ToolKind::Search => "search",
            ToolKind::Execute => "execute",
            ToolKind::Think => "think",
            ToolKind::Fetch => "fetch",
            ToolKind::SwitchMode => "switch_mode",
            ToolKind::Other => "other",
        }
    }

    /// Whether the tool never mutates anything (safe to auto-allow by default).
    pub fn is_read_only(self) -> bool {
        matches!(
            self,
            ToolKind::Read | ToolKind::Search | ToolKind::Think | ToolKind::Fetch
        )
    }
}

/// A file modification performed by a tool, used for diffs and post-edit hooks.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEdit {
    pub path: PathBuf,
    pub old_text: Option<String>,
    pub new_text: String,
}

#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    pub edits: Vec<FileEdit>,
    pub plan: Option<Vec<PlanItem>>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            ..Default::default()
        }
    }
}

/// Interaction with the human on the other side of the protocol.
#[async_trait]
pub trait UserBridge: Send + Sync {
    async fn ask_user(
        &self,
        session_id: &str,
        questions: Vec<Question>,
    ) -> anyhow::Result<Vec<Answer>>;
}

#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub session_id: String,
    pub user: Arc<dyn UserBridge>,
    pub cancel: CancellationToken,
    /// The id of the tool call being run (permission prompts raised by the call refer to it).
    pub tool_call_id: String,
    /// Set when commands run under the sandbox.
    pub sandbox: Option<SandboxHandle>,
    /// The product the tools run for (names it in the messages they return).
    pub identity: AppIdentity,
}

impl ToolContext {
    pub fn resolve(&self, p: &str) -> PathBuf {
        let p = Path::new(p);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(p)
        }
    }
}

/// How a profile's `lean_tools` setting treats a built-in tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeanClass {
    /// Offered as usual.
    #[default]
    Keep,
    /// Left out when `lean_tools` is on, even if the profile's `tools` allowlist names it.
    DropWhenLean,
    /// Left out when `lean_tools` is on unless the profile's `tools` allowlist names it.
    OptInWhenLean,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String;
    fn input_schema(&self) -> Value;
    fn kind(&self) -> ToolKind;

    /// Short human-readable title for a call, e.g. "Read src/main.rs".
    fn title(&self, input: &Value) -> String {
        let _ = input;
        self.name().to_string()
    }

    /// Files this call touches, for client "follow along" features.
    fn locations(&self, input: &Value) -> Vec<String> {
        ["file_path", "path"]
            .iter()
            .filter_map(|k| input.get(*k).and_then(Value::as_str).map(str::to_string))
            .take(1)
            .collect()
    }

    /// The plan this call writes, for tools that maintain the session's plan (the agent shows
    /// it to the client, skips updates that change nothing and carries it into the compaction
    /// summary). `None` for every other tool, and for input that is not a valid plan.
    fn plan(&self, input: &Value) -> Option<Vec<PlanItem>> {
        let _ = input;
        None
    }

    /// Whether the tool runs without a permission prompt when no mode or config rule applies,
    /// regardless of its [`ToolKind`].
    fn auto_allow(&self) -> bool {
        false
    }

    /// How the `lean_tools` profile setting treats this tool.
    fn lean_class(&self) -> LeanClass {
        LeanClass::Keep
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput>;

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: self.description(),
            input_schema: self.input_schema(),
        }
    }
}

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
    if web.enabled {
        v.push(Arc::new(WebFetch::new(web)));
        v.push(Arc::new(WebSearch::new(web)));
    }
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

pub(crate) fn str_arg<'a>(input: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing required string parameter `{key}`"))
}

/// Truncate long output, keeping the head and tail, with a marker naming what was cut.
pub fn truncate_middle(s: &str, max: usize, identity: &AppIdentity) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let mut head_end = half;
    while !s.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = s.len() - half;
    while !s.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let cut = &s[head_end..tail_start];
    format!(
        "{}\n\n[... {} lines ({} bytes) truncated by {}; re-run with a narrower command \
         or read a range ...]\n\n{}",
        &s[..head_end],
        cut.matches('\n').count() + 1,
        cut.len(),
        identity.name,
        &s[tail_start..]
    )
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    pub struct NoUser;

    #[async_trait]
    impl UserBridge for NoUser {
        async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
            anyhow::bail!("no user")
        }
    }

    pub fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
            session_id: "test".into(),
            user: Arc::new(NoUser),
            cancel: CancellationToken::new(),
            tool_call_id: String::new(),
            sandbox: None,
            identity: ruck_harness_ident::testing::RUCKCODE,
        }
    }
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
mod identity_tests {
    use super::*;
    use ruck_harness_ident::testing::{ACME, RUCKCODE};

    #[test]
    fn truncation_marker_names_the_product() {
        let long = format!("{}\n{}", "a".repeat(40), "b".repeat(40));
        let cut = |id| truncate_middle(&long, 20, &id);
        assert_eq!(
            cut(RUCKCODE),
            "aaaaaaaaaa\n\n[... 2 lines (61 bytes) truncated by ruckcode; re-run with a narrower command or read a range ...]\n\nbbbbbbbbbb"
        );
        assert!(cut(ACME).contains("truncated by acme; re-run"));
        assert_eq!(truncate_middle("short", 20, &ACME), "short");
    }

    #[test]
    fn protected_write_message_names_the_products_paths() {
        assert_eq!(
            protected_write_msg(&RUCKCODE),
            "refused: this path is ruckcode's own configuration (.ruckcode/, .mcp.json or the user config directory) or protected git state (.git/config, .git/hooks/, .git/info/, remote-tracking refs). The sandbox policy can only be changed by the user."
        );
        assert_eq!(
            protected_write_msg(&ACME),
            "refused: this path is acme's own configuration (.acme/, .acme-tools.json or the user config directory) or protected git state (.git/config, .git/hooks/, .git/info/, remote-tracking refs). The sandbox policy can only be changed by the user."
        );
    }
}

#[cfg(test)]
mod hook_tests {
    use super::*;
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
