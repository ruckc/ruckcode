//! The ACP server runs a toy product with no coding code: ACME, namespace `acme`, one `Echo` tool
//! and the mock provider. Nothing on the wire may name any other product.
//!
//! This test depends on no ruckcode crate.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use ruck_harness_acp::{spawn_in_process_raw, AcpInfo};
use ruck_harness_core::agent::{Agent, AgentOptions};
use ruck_harness_core::config::{HarnessConfig, Profile};
use ruck_harness_core::modes::{ModeDef, ModeSchema, ModeSet};
use ruck_harness_core::prompt::{PromptCtx, PromptProvider};
use ruck_harness_core::session_policy::SessionPolicy;
use ruck_harness_core::tool_loop_guard::GuardHints;
use ruck_harness_core::{AgentExtension, SessionExt};
use ruck_harness_ident::{testing::ACME, AppIdentity};
use ruck_harness_llm::mock::MockProvider;
use ruck_harness_llm::{ContentBlock, Message, Provider, Role};
use ruck_harness_tools::{PlanItem, Tool, ToolContext, ToolKind, ToolOutput};
use serde_json::{json, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

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

/// A raw client: sends requests, records every line the server writes.
struct Wire {
    tx: UnboundedSender<String>,
    rx: UnboundedReceiver<String>,
    seen: Vec<String>,
    next_id: u64,
}

impl Wire {
    /// Send a request and return its response, recording everything read on the way.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.tx.send(line.to_string()).unwrap();
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(10), self.rx.recv())
                .await
                .expect("timed out waiting for the server")
                .expect("server closed");
            self.seen.push(line.clone());
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) && v.get("method").is_none() {
                return v;
            }
        }
    }
}

#[tokio::test]
async fn the_acp_server_speaks_the_identitys_namespace_and_nothing_else() {
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
    let agent = Arc::new(Agent::new(
        AgentOptions {
            provider_override: Some(mock),
            user_config_dir: Some(user.path().to_path_buf()),
            persist: true,
            ..Default::default()
        },
        Arc::new(AcmeExtension),
    ));
    let (tx, rx) = spawn_in_process_raw(agent, AcpInfo::new("9.9.9"));
    let mut w = Wire {
        tx,
        rx,
        seen: vec![],
        next_id: 0,
    };

    // initialize: the product's name, version and capability namespace.
    let init = w
        .call(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {"_meta": {"acme": {"notices": true}}}}),
        )
        .await;
    assert_eq!(init["result"]["agentInfo"]["name"], "acme");
    assert_eq!(init["result"]["agentInfo"]["version"], "9.9.9");
    let meta = &init["result"]["agentCapabilities"]["_meta"];
    assert_eq!(meta["acme"]["systemPrompt"]["append"], true);

    // session/new honours `_meta.acme.systemPrompt`.
    let new = w
        .call(
            "session/new",
            json!({"cwd": proj.path(), "mcpServers": [],
                   "_meta": {"acme": {"systemPrompt": {"append": "ACME-RULES"}}}}),
        )
        .await;
    let sid = new["result"]["sessionId"].as_str().unwrap().to_string();

    // A turn with a tool call.
    let prompt = w
        .call(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "say hi"}]}),
        )
        .await;
    assert_eq!(prompt["result"]["stopReason"], "end_turn");
    assert!(
        w.seen.iter().any(|l| l.contains("echo: hi")),
        "the tool result is streamed"
    );

    // The `_acme/...` extension methods work.
    let listed = w
        .call("_acme/list_sessions", json!({"cwd": proj.path()}))
        .await;
    assert_eq!(listed["result"]["sessions"][0]["id"], sid.as_str());
    let pinned = w
        .call("_acme/set_model", json!({"sessionId": sid, "model": "m-1"}))
        .await;
    assert_eq!(pinned["result"], json!({}));

    // The standard session/list carries the namespaced `_meta`.
    let list = w.call("session/list", json!({"cwd": proj.path()})).await;
    assert_eq!(
        list["result"]["sessions"][0]["_meta"]["acme"]["mode"],
        "default"
    );

    // Nothing the server wrote names another product.
    let wire = w.seen.join("\n").to_lowercase();
    assert!(!wire.contains("ruckcode"), "{wire}");
    assert!(wire.contains("acme"), "{wire}");
}
