//! Scripted provider for tests and offline demos.

use crate::{Message, Provider, Request, Response, Role, StopReason, StreamEvent, Usage};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

/// Returns queued assistant messages in order and records every request it receives.
#[derive(Default)]
pub struct MockProvider {
    responses: Mutex<VecDeque<Message>>,
    pub requests: Mutex<Vec<Request>>,
}

impl MockProvider {
    pub fn new(responses: impl IntoIterator<Item = Message>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::default(),
        }
    }

    pub fn push(&self, m: Message) {
        self.responses.lock().unwrap().push_back(m);
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn complete(
        &self,
        request: Request,
        events: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response> {
        self.requests.lock().unwrap().push(request);
        let message = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Message::assistant_text("(mock: no more scripted responses)"));
        assert_eq!(message.role, Role::Assistant);
        let text = message.text();
        if !text.is_empty() {
            let _ = events.send(StreamEvent::TextDelta(text));
        }
        let stop_reason = if message.tool_uses().next().is_some() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        Ok(Response {
            message,
            stop_reason,
            usage: Usage::default(),
        })
    }
}
