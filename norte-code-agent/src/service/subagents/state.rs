//! Reading a child's state from the session records.

use std::sync::{Arc, RwLock};

use norte::Cancellation;
use norte::id::SessionId;
use norte_code_wire::SessionKey;

use crate::service::Service;
use crate::service::subagents::SubagentOutcome;
use crate::service::subagents::child_state_label;
use futures_util::StreamExt;
use norte_code_wire::{MessageRole, ReplayedKind, ServerEvent, ServerEventKind};

pub(crate) fn parent_key(service: &Service, parent: SessionId) -> Option<SessionKey> {
    let sessions = service.lock_sessions();
    sessions
        .iter()
        .find(|(_, record)| record.id == parent)
        .map(|(key, _)| key.clone())
}

/// The child `id` as a `SessionKey`, or an error naming the bad id.
pub(crate) fn child_key(id: &str) -> Result<SessionKey, String> {
    if id.trim().is_empty() {
        Err("child_session_id is required".to_owned())
    } else {
        Ok(SessionKey::new(id.to_owned()))
    }
}

/// The phase of a session, for `inspect_agent`'s `status` view.
pub(crate) fn phase_of(service: &Service, child: &SessionKey) -> Option<norte_code_wire::Phase> {
    service
        .lock_sessions()
        .get(child)
        .map(|record| record.phase)
}

pub(crate) fn registered_child_state(
    child: &crate::service::delegator::Child,
    phase: Option<norte_code_wire::Phase>,
) -> &'static str {
    if child.is_terminal() {
        return if child.is_failed() {
            "FAILED"
        } else {
            "STOPPED"
        };
    }
    if child.is_finished() {
        return "IDLE";
    }
    phase.map_or("RUNNING", |phase| child_state_label(phase, true))
}

/// The service behind a delegating tool, when one is installed.
pub(crate) fn service_of(slot: &Arc<RwLock<Option<Arc<Service>>>>) -> Option<Arc<Service>> {
    slot.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub(crate) async fn run_session_message(
    service: &Arc<Service>,
    child: &SessionKey,
    text: String,
    cancel: Option<&Cancellation>,
) -> SubagentOutcome {
    let request_id = service.next_auto_request_id();
    let mut stream = match service
        .start_turn(
            request_id,
            Some(child.clone()),
            Some(text),
            Vec::new(),
            None,
            None,
            false,
        )
        .await
    {
        Ok(stream) => stream,
        Err(error) => {
            return SubagentOutcome {
                text: error.to_string(),
                is_error: true,
                child: Some(child.clone()),
                output_schema_valid: None,
            };
        }
    };
    if let Some(parent) = service
        .lock_sessions()
        .get(child)
        .and_then(|record| record.parent_client.clone())
    {
        service.children.set_running(&parent, child);
    }
    let mut failed = false;
    let mut cancellation_requested = false;
    let cancel_wait: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> =
        match cancel {
            Some(cancel) => Box::pin(cancel.cancelled()),
            None => Box::pin(std::future::pending()),
        };
    tokio::pin!(cancel_wait);
    loop {
        enum Next {
            Event(Box<ServerEvent>),
            End,
            Cancel,
        }
        let next = tokio::select! {
            event = stream.next() => match event {
                Some(event) => Next::Event(Box::new(event)),
                None => Next::End,
            },
            () = &mut cancel_wait, if !cancellation_requested => Next::Cancel,
        };
        match next {
            Next::Event(event) => {
                failed |= matches!(event.kind, ServerEventKind::Error { .. });
            }
            Next::End => break,
            Next::Cancel => {
                cancellation_requested = true;
                service.cancel_turn(child);
            }
        }
    }
    let text = match service.session_history(child).await {
        Ok(history) => history
            .into_iter()
            .rev()
            .find_map(|(role, kind, text)| {
                (role == MessageRole::Assistant && kind == ReplayedKind::Text).then_some(text)
            })
            .unwrap_or_default(),
        Err(error) => {
            return SubagentOutcome {
                text: format!("could not read child history: {error}"),
                is_error: true,
                child: Some(child.clone()),
                output_schema_valid: None,
            };
        }
    };
    SubagentOutcome {
        is_error: failed || (!cancellation_requested && text.is_empty()),
        text: if text.is_empty() && cancellation_requested {
            "child turn cancelled".to_owned()
        } else if text.is_empty() {
            "child turn finished without an assistant message".to_owned()
        } else {
            text
        },
        child: Some(child.clone()),
        output_schema_valid: None,
    }
}
