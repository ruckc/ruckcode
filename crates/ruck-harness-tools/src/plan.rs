use crate::{LeanClass, Tool, ToolContext, ToolKind, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

impl PlanStatus {
    /// The serialized (snake_case) name, as it appears in the tool input.
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStatus::Pending => "pending",
            PlanStatus::InProgress => "in_progress",
            PlanStatus::Completed => "completed",
        }
    }
}

/// Transition alias for [`PlanStatus`].
#[deprecated(note = "renamed to PlanStatus")]
pub type TodoStatus = PlanStatus;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanItem {
    pub content: String,
    pub status: PlanStatus,
}

/// Transition alias for [`PlanItem`].
#[deprecated(note = "renamed to PlanItem")]
pub type TodoItem = PlanItem;

/// Appended to the tool result when the last plan item is completed.
pub const ALL_DONE: &str = "All plan items are complete. Tell the user the plan is finished and summarise what was done, then stop unless they ask for more.";

pub struct TodoWrite;

#[async_trait]
impl Tool for TodoWrite {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    fn description(&self) -> String {
        "Replace the session's task list. Use it to plan multi-step work and mark progress; \
         keep exactly one item in_progress while working."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Think
    }

    fn title(&self, _: &Value) -> String {
        "Update plan".into()
    }

    fn plan(&self, input: &Value) -> Option<Vec<PlanItem>> {
        serde_json::from_value(input["todos"].clone()).ok()
    }

    fn auto_allow(&self) -> bool {
        true
    }

    fn lean_class(&self) -> LeanClass {
        LeanClass::DropWhenLean
    }

    async fn call(&self, input: Value, _: &ToolContext) -> anyhow::Result<ToolOutput> {
        let todos: Vec<PlanItem> = match serde_json::from_value(input["todos"].clone()) {
            Ok(t) => t,
            Err(e) => return Ok(ToolOutput::error(format!("invalid todos: {e}"))),
        };
        let done = todos
            .iter()
            .filter(|t| t.status == PlanStatus::Completed)
            .count();
        let mut content = format!("Plan updated ({done}/{} completed)", todos.len());
        if !todos.is_empty() && done == todos.len() {
            content.push_str(". ");
            content.push_str(ALL_DONE);
        }
        Ok(ToolOutput {
            content,
            plan: Some(todos),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run(todos: Value) -> String {
        let ctx = crate::test_util::ctx(std::path::Path::new("."));
        TodoWrite
            .call(json!({ "todos": todos }), &ctx)
            .await
            .unwrap()
            .content
    }

    #[tokio::test]
    async fn partial_completion_does_not_announce_done() {
        let out = run(json!([
            {"content": "a", "status": "completed"},
            {"content": "b", "status": "in_progress"}
        ]))
        .await;
        assert_eq!(out, "Plan updated (1/2 completed)");
    }

    #[tokio::test]
    async fn last_item_completed_tells_the_model_to_report() {
        let out = run(json!([
            {"content": "a", "status": "completed"},
            {"content": "b", "status": "completed"}
        ]))
        .await;
        assert!(out.starts_with("Plan updated (2/2 completed). "));
        assert!(out.ends_with(ALL_DONE));
        assert_eq!(run(json!([])).await, "Plan updated (0/0 completed)");
    }
}
