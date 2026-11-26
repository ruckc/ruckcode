//! The generic agent loop runs under an extension that has nothing to do with coding.
//!
//! This test names no coding crate, no mise and no `RuckcodeExtension`: the product is a toy
//! called ACME with one `Echo` tool, no plan tool, neutral guard hints and a one-line prompt.

use anyhow::Result;
use async_trait::async_trait;
use ruck_harness_ident::{testing::ACME, AppIdentity};
use ruck_harness_llm::mock::MockProvider;
use ruck_harness_llm::{ContentBlock, Message, Provider, Role};
use ruck_harness_tools::{
    Answer, PlanItem, Question, Tool, ToolContext, ToolKind, ToolOutput, UserBridge,
};
use ruckcode_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
};
use ruckcode_core::config::{HarnessConfig, Profile};
use ruckcode_core::modes::{ModeDef, ModeSchema, ModeSet};
use ruckcode_core::prompt::{PromptCtx, PromptProvider};
use ruckcode_core::session_policy::SessionPolicy;
use ruckcode_core::tool_loop_guard::GuardHints;
use ruckcode_core::{AgentExtension, SessionExt};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

struct Echo;

#[async_trait]
impl Tool for Echo {
    fn name(&self) -> &str {
        "Echo"
    }
    fn description(&self) -> String {
        "Echo the text back.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"text": {"type": "string"}}})
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }
    async fn call(&self, input: Value, _: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::ok(format!(
            "echo: {}",
            input["text"].as_str().unwrap_or_default()
        )))
    }
}

struct AcmePrompt;

#[async_trait]
impl PromptProvider for AcmePrompt {
    async fn base(&self, _: &PromptCtx<'_>) -> String {
        "You are the ACME assistant.".into()
    }
}

static ACME_MODES: ModeSchema = ModeSchema {
    extra_keys: &[],
    check_extra: |_, _| Ok(()),
    implicit_extra: BTreeMap::new,
    is_builtin_tool: |name| name == "Echo",
};

struct AcmeExtension;

#[async_trait]
impl AgentExtension for AcmeExtension {
    fn identity(&self) -> &AppIdentity {
        &ACME
    }
    fn prompt(&self) -> Arc<dyn PromptProvider> {
        Arc::new(AcmePrompt)
    }
    fn guard_hints(&self) -> GuardHints {
        GuardHints::default()
    }
    fn session_policy(&self) -> Option<Arc<dyn SessionPolicy>> {
        None
    }
    fn mode_schema(&self) -> &ModeSchema {
        &ACME_MODES
    }
    fn make_provider(&self, _: &Profile, _: Option<&Path>) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(MockProvider::default()))
    }
    async fn session_init(
        &self,
        _: &Path,
        _: &HarnessConfig,
        _: &toml::Table,
        _: &ModeSet,
    ) -> Result<Box<dyn SessionExt>> {
        Ok(Box::new(AcmeSession))
    }
    fn decode_plan(&self, _: &str, _: &Value) -> Option<Vec<PlanItem>> {
        None
    }
    fn builtin_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        (name == "Echo").then(|| Arc::new(Echo) as Arc<dyn Tool>)
    }
}

struct AcmeSession;

#[async_trait]
impl SessionExt for AcmeSession {
    fn builtin_tools(
        &self,
        _: &ModeDef,
        _: &HarnessConfig,
        _: Option<&Path>,
    ) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(Echo)]
    }
    async fn refresh(&mut self, _: &Path) {}
}

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
}

#[async_trait]
impl UserBridge for TestClient {
    async fn ask_user(&self, _: &str, _: Vec<Question>) -> Result<Vec<Answer>> {
        anyhow::bail!("no user")
    }
}

#[async_trait]
impl Client for TestClient {
    async fn request_permission(&self, _: &str, _: PermissionRequest) -> PermissionDecision {
        PermissionDecision::AllowOnce
    }
    fn send_event(&self, _: &str, event: SessionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

#[tokio::test]
async fn a_turn_runs_under_an_extension_with_no_coding_code() {
    let proj = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::default());
    mock.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "t1".into(),
            name: "Echo".into(),
            input: json!({"text": "hi"}),
        }],
    });
    mock.push(Message::assistant_text("all done"));
    let agent = Agent::new(
        AgentOptions {
            provider_override: Some(mock.clone()),
            user_config_dir: Some(user.path().to_path_buf()),
            persist: false,
            ..Default::default()
        },
        Arc::new(AcmeExtension),
    );
    let info = agent
        .new_session(proj.path().to_path_buf(), None)
        .await
        .unwrap();
    let client = Arc::new(TestClient::default());
    let stop = agent
        .prompt(&info.id, "say hi".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(stop, StopReason::EndTurn);
    assert!(client
        .events
        .lock()
        .unwrap()
        .contains(&SessionEvent::AgentText("all done".into())));

    let reqs = mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    // The extension's prompt and tool set, nothing else.
    assert!(reqs[0].system.starts_with("You are the ACME assistant."));
    assert!(!reqs[0].system.to_lowercase().contains("ruckcode"));
    let names: Vec<_> = reqs[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Echo"]);
    // The tool ran and its result reached the model.
    let result = reqs[1]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .find_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        });
    assert_eq!(result.as_deref(), Some("echo: hi"));
}
