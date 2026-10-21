//! ACP client plumbing shared by the TUI and the headless runner.

use std::path::Path;

use anyhow::{anyhow, Result};
use ruckcode_acp::schema::{self, SessionMode, SessionModeState};
use ruckcode_acp::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const LIST_SESSIONS: &str = "_ruckcode/list_sessions";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskRequest {
    #[serde(rename = "sessionId", default)]
    pub session_id: String,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub question: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AskResponse {
    pub answers: Vec<Answer>,
    pub cancelled: bool,
}

impl AskResponse {
    pub fn cancelled() -> Self {
        Self {
            answers: vec![],
            cancelled: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionEntry {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub first_prompt: String,
    pub mode: String,
}

pub struct Started {
    pub session_id: String,
    pub modes: Option<SessionModeState>,
}

pub async fn initialize(conn: &Connection) -> Result<()> {
    let params = json!({
        "protocolVersion": ruckcode_acp::PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
            "_meta": {"ruckcode": {"askUserQuestion": true}}
        }
    });
    conn.request(schema::INITIALIZE, params)
        .await
        .map_err(|e| anyhow!("initialize failed: {e}"))?;
    Ok(())
}

pub async fn new_session(conn: &Connection, cwd: &Path) -> Result<Started> {
    let r: schema::NewSessionResponse = conn
        .request_typed(
            schema::SESSION_NEW,
            &schema::NewSessionRequest {
                cwd: cwd.to_path_buf(),
                mcp_servers: vec![],
            },
        )
        .await
        .map_err(|e| anyhow!("session/new failed: {e}"))?;
    Ok(Started {
        session_id: r.session_id,
        modes: r.modes,
    })
}

pub async fn load_session(conn: &Connection, cwd: &Path, id: &str) -> Result<Started> {
    let r: schema::LoadSessionResponse = conn
        .request_typed(
            schema::SESSION_LOAD,
            &schema::LoadSessionRequest {
                session_id: id.to_string(),
                cwd: cwd.to_path_buf(),
                mcp_servers: vec![],
            },
        )
        .await
        .map_err(|e| anyhow!("session/load failed: {e}"))?;
    Ok(Started {
        session_id: id.to_string(),
        modes: r.modes,
    })
}

pub async fn set_mode(conn: &Connection, session_id: &str, mode_id: &str) -> Result<()> {
    conn.request(
        schema::SESSION_SET_MODE,
        json!({"sessionId": session_id, "modeId": mode_id}),
    )
    .await
    .map_err(|e| anyhow!("set_mode failed: {e}"))?;
    Ok(())
}

/// Pin (`Some`) or clear (`None`) the session model.
pub async fn set_model(conn: &Connection, session_id: &str, model: Option<&str>) -> Result<()> {
    conn.request(
        "_ruckcode/set_model",
        json!({"sessionId": session_id, "model": model}),
    )
    .await
    .map_err(|e| anyhow!("set_model failed: {e}"))?;
    Ok(())
}

/// Sessions of `cwd`, newest first.
pub async fn list_sessions(conn: &Connection, cwd: &Path) -> Result<Vec<SessionEntry>> {
    let v: Value = conn
        .request(LIST_SESSIONS, json!({"cwd": cwd}))
        .await
        .map_err(|e| anyhow!("list sessions failed: {e}"))?;
    let mut sessions: Vec<SessionEntry> =
        serde_json::from_value(v.get("sessions").cloned().unwrap_or(json!([])))?;
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(sessions)
}

/// Option id to answer a permission request: first of `kinds` present, else cancelled.
pub fn pick_option(
    req: &schema::RequestPermissionRequest,
    allow: bool,
) -> schema::RequestPermissionOutcome {
    use schema::PermissionOptionKind::*;
    let wanted: &[_] = if allow {
        &[AllowOnce, AllowAlways]
    } else {
        &[RejectOnce, RejectAlways]
    };
    for k in wanted {
        if let Some(o) = req.options.iter().find(|o| o.kind == *k) {
            return schema::RequestPermissionOutcome::Selected {
                option_id: o.option_id.clone(),
            };
        }
    }
    schema::RequestPermissionOutcome::Cancelled
}

pub fn mode_list(state: &Option<SessionModeState>) -> (Vec<SessionMode>, String) {
    match state {
        Some(s) => (s.available_modes.clone(), s.current_mode_id.clone()),
        None => (vec![], String::new()),
    }
}
