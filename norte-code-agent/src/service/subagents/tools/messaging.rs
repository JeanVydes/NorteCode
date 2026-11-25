//! Child messaging tools.

use std::sync::Arc;

use async_trait::async_trait;
use norte::id::SessionId;
use norte::{FnExecutor, ToolContext, ToolOutput};
use norte_code_wire::{ChildState, SessionKey};

use super::super::{
    MessageArgs, MessageDelivery, RwLock, Service, SharedChildren, answer, child_key, parent_key,
    phase_of, schedule_child_turn, service_of,
};
use crate::service::steering::Steered;
use crate::service::subagent_views::{AcceptedMessage, HeldMessage};

/// `message_agent` (RFC 8.2.8): send a message to a child session.
///
/// `steer` (default) puts it in front of the request in flight; `queue`
/// makes it the child's next turn; `interrupt` cancels the child's turn
/// first. While the child's delegator is suspended the message is held,
/// and `held: true` says so.
pub(crate) struct MessageAgent {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl MessageAgent {
    /// A messenger over the shared child registry.
    pub(crate) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    async fn deliver(
        &self,
        parent: SessionId,
        service: Arc<Service>,
        parent_key: SessionKey,
        child: SessionKey,
        text: String,
        delivery: MessageDelivery,
    ) -> ToolOutput {
        let Some(registered) = self.children.get(&parent_key, &child) else {
            return ToolOutput::error(format!(
                "CHILD_NOT_FOUND: no child `{child}` under this session"
            ));
        };
        if registered.is_terminal() {
            return ToolOutput::error(format!("CHILD_NOT_ACTIVE: child `{child}` is terminal"));
        }
        if registered.suspended {
            return match service.hold_delegator_message(&child, text).await {
                Ok(held) => ToolOutput::text(answer(&HeldMessage {
                    accepted: true,
                    delivery,
                    held: true,
                    held_count: held,
                })),
                Err(error) => ToolOutput::error(error.to_string()),
            };
        }
        let is_running = !registered.is_finished();
        if is_running && delivery == MessageDelivery::Steer {
            if phase_of(&service, &child) == Some(norte_code_wire::Phase::AwaitingTool) {
                return ToolOutput::error("CHILD_NOT_ACTIVE: child is waiting for a tool decision");
            }
            let nid = match service.require_record(&child) {
                Ok((nid, _, _)) => nid,
                Err(error) => return ToolOutput::error(format!("CHILD_NOT_FOUND: {error}")),
            };
            service.steering.push(
                nid,
                Steered::new(
                    uuid::Uuid::new_v4().to_string(),
                    format!("Message from delegator:\n{text}"),
                    Vec::new(),
                    Vec::new(),
                ),
            );
            return ToolOutput::text(answer(&AcceptedMessage {
                accepted: true,
                delivery: MessageDelivery::Steer,
                child_state: ChildState::Running,
                held: false,
            }));
        }
        let Some(start_worker) = self.children.queue_message(
            &parent_key,
            &child,
            format!("Message from delegator:\n{text}"),
        ) else {
            return ToolOutput::error(format!("CHILD_NOT_ACTIVE: child `{child}` is terminal"));
        };
        if is_running && delivery == MessageDelivery::Interrupt {
            if let Some(cancel) = &registered.cancel {
                cancel.cancel();
            }
            service.cancel_turn(&child);
        }
        if start_worker
            && let Some(message) = self.children.take_queued_message(&parent_key, &child)
        {
            schedule_child_turn(
                service,
                Arc::clone(&self.service),
                Arc::clone(&self.children),
                parent,
                child,
                message,
            );
        }
        ToolOutput::text(answer(&AcceptedMessage {
            accepted: true,
            delivery,
            child_state: ChildState::Running,
            held: false,
        }))
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(crate) fn into_declaration(self) -> norte::ToolDeclaration<MessageArgs> {
        norte::ToolDeclaration::new(
            "message_agent",
            "Send a message to a child session you spawned. `delivery: steer` (the default) puts it in front of what the child is doing now; `queue` makes it the child's next turn; `interrupt` stops the child's current turn first. While the child's delegator is suspended the message is held and the answer says so.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<MessageArgs, ToolOutput> for MessageAgent {
    async fn call(&self, _args: MessageArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("message_agent needs a session"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: MessageArgs,
    ) -> norte::Result<ToolOutput> {
        let (Some(parent), Some(service)) = (context.session, service_of(&self.service)) else {
            return Ok(ToolOutput::error(
                "message_agent needs a client attached: a child belongs to a parent session",
            ));
        };
        let Some(parent_key) = parent_key(&service, parent) else {
            return Ok(ToolOutput::error(
                "message_agent: parent session is unknown",
            ));
        };
        let Ok(child) = child_key(&args.child_session_id) else {
            return Ok(ToolOutput::error("message_agent needs a child_session_id"));
        };
        let text = match message_text(&args.content) {
            Ok(text) => text,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        if text.trim().is_empty() {
            return Ok(ToolOutput::error("message_agent: content is empty"));
        }
        let delivery = args.delivery.unwrap_or_default();
        Ok(self
            .deliver(parent, service, parent_key, child, text, delivery)
            .await)
    }
}

fn message_text(content: &serde_json::Value) -> Result<String, &'static str> {
    match content {
        serde_json::Value::String(text) => Ok(text.clone()),
        serde_json::Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                let Some(chunk) = part.get("text").and_then(serde_json::Value::as_str) else {
                    return Err("message_agent: only text content parts are carried");
                };
                text.push_str(chunk);
            }
            Ok(text)
        }
        _ => Err("message_agent: content must be a string or an array of text parts"),
    }
}
