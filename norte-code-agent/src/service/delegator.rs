//! Delegation plumbing (RFC 8.2): the child registry and the parent's view.
//!
//! `spawn_agent` creates a child session and runs one turn on it; this module
//! owns the *registry* of those children — who is whose child, whether a
//! delegator is suspended, and which messages are held while it is. The tools
//! themselves live in [`super::subagents`]; the service's session state lives
//! in [`super::sessions`]. This module sits between them so the tools can ask
//! "is this child mine, and is its delegator suspended?" without reaching
//! into the session lock.
//!
//! A child's identity is its `SessionKey`: the same value `children_of`
//! returns and the same value the RAEP stream carries as `child_session_id`.
//! There is no second id to keep in sync.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use norte::Cancellation;
use norte_code_wire::SessionKey;
use tokio::sync::Notify;

/// How a child's run ended.
///
/// The call site used to pass two booleans — `terminal` and `failed` — and the
/// registry decoded them into a third state. `(false, true)` was a failure that
/// was not terminal, a combination nothing ever meant but nothing rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RunEnd {
    /// The turn ended; the child can take another message.
    Idle,
    /// It failed and will not run again.
    Failed,
    /// It was stopped and will not run again.
    Stopped,
}

impl RunEnd {
    /// The state a finished run leaves the child in.
    fn into_state(self) -> ChildState {
        match self {
            Self::Idle => ChildState::Idle,
            Self::Failed => ChildState::Failed,
            Self::Stopped => ChildState::Stopped,
        }
    }
}

/// Lifecycle state of a child delegation.
///
/// It is the protocol's `ChildState`, not a fourth copy of it: the registry,
/// the event stream and the resource endpoint all describe one child, and a
/// state each of them spells differently is a state that drifts.
pub(super) use norte_code_wire::ChildState;

/// One delegation: a child session and what the parent knows about it.
#[derive(Debug)]
pub(super) struct Child {
    /// The child session, by the key the service created for it.
    #[allow(clippy::struct_field_names)]
    pub(super) child: SessionKey,
    /// The tool call that spawned it, so `CHILD_SESSION_STARTED` can carry
    /// it and a parent can see which call it came from.
    pub(super) tool_call_id: String,
    /// Whether the delegator is suspended (RFC 8.2.11). While it is, the
    /// parent's `message_agent` holds its message and `stop_agent` fails.
    pub(super) suspended: bool,
    /// Messages held while suspended, oldest first. Flushed on release.
    pub(super) held: Vec<String>,
    /// Messages accepted with `delivery: queue` or `interrupt`.
    pub(super) pending_messages: Vec<String>,
    /// Current lifecycle state.
    pub(super) state: ChildState,
    /// Runtime cancellation handle, absent after restoring a process.
    pub(super) cancel: Option<Arc<Cancellation>>,
    /// Wakes `wait_agent` when the child settles.
    pub(super) changed: Arc<Notify>,
}

impl Child {
    /// A freshly spawned child: running, not suspended, nothing held.
    pub(super) fn new(child: SessionKey, tool_call_id: String) -> Self {
        Self {
            child,
            tool_call_id,
            suspended: false,
            held: Vec::new(),
            pending_messages: Vec::new(),
            state: ChildState::Running,
            cancel: None,
            changed: Arc::new(Notify::new()),
        }
    }

    pub(super) fn is_finished(&self) -> bool {
        // `AwaitingApproval` is not progress: the child is parked until its
        // parent answers, which for the registry is finished with its turn.
        !matches!(self.state, ChildState::Running)
    }

    pub(super) fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    pub(super) fn is_failed(&self) -> bool {
        self.state == ChildState::Failed
    }
}

/// The children the service has spawned, by parent.
#[derive(Debug, Default)]
pub(super) struct ChildRegistry {
    /// `parent -> child` in spawn order, so `list_agents` is stable.
    pub(super) children: Mutex<HashMap<SessionKey, Vec<Child>>>,
}

impl ChildRegistry {
    /// An empty registry, for tests and for services that never delegate.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Registers a child under its parent. Returns the registration so the
    /// caller can emit the event that announces it.
    pub(super) fn register(&self, parent: SessionKey, child: Child) -> Child {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        children.entry(parent).or_default().push(Child {
            child: child.child.clone(),
            tool_call_id: child.tool_call_id.clone(),
            suspended: child.suspended,
            held: child.held.clone(),
            pending_messages: child.pending_messages.clone(),
            state: child.state,
            cancel: child.cancel.clone(),
            changed: Arc::clone(&child.changed),
        });
        child
    }

    /// The child `id` under `parent`, if the parent spawned it.
    pub(super) fn get(&self, parent: &SessionKey, id: &SessionKey) -> Option<Child> {
        let children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        children
            .get(parent)
            .and_then(|list| list.iter().find(|child| child.child == *id))
            .map(|child| Child {
                child: child.child.clone(),
                tool_call_id: child.tool_call_id.clone(),
                suspended: child.suspended,
                held: child.held.clone(),
                pending_messages: child.pending_messages.clone(),
                state: child.state,
                cancel: child.cancel.clone(),
                changed: Arc::clone(&child.changed),
            })
    }

    /// Every child of `parent`, in spawn order.
    pub(super) fn of(&self, parent: &SessionKey) -> Vec<Child> {
        let children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        children
            .get(parent)
            .map(|list| {
                list.iter()
                    .map(|child| Child {
                        child: child.child.clone(),
                        tool_call_id: child.tool_call_id.clone(),
                        suspended: child.suspended,
                        held: child.held.clone(),
                        pending_messages: child.pending_messages.clone(),
                        state: child.state,
                        cancel: child.cancel.clone(),
                        changed: Arc::clone(&child.changed),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Suspends or releases the delegator of `child` under `parent`.
    ///
    /// Returns the new state and the messages that were held. A release is
    /// what flushes them: the caller hands them to the session's steering.
    pub(super) fn set_suspended(
        &self,
        parent: &SessionKey,
        id: &SessionKey,
        suspended: bool,
    ) -> Option<(bool, Vec<String>)> {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        let child = children
            .get_mut(parent)
            .and_then(|list| list.iter_mut().find(|child| child.child == *id))?;
        child.suspended = suspended;
        let held = if suspended {
            Vec::new()
        } else {
            std::mem::take(&mut child.held)
        };
        Some((suspended, held))
    }

    /// Marks a child as running again after a message starts another turn.
    pub(super) fn set_running(&self, parent: &SessionKey, id: &SessionKey) -> bool {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(child) = children
            .get_mut(parent)
            .and_then(|list| list.iter_mut().find(|child| child.child == *id))
        {
            if child.state.is_terminal() {
                return false;
            }
            child.state = ChildState::Running;
            return true;
        }
        false
    }

    /// Finishes a run and atomically claims its next queued message, if any.
    pub(super) fn finish_and_take_message(
        &self,
        parent: &SessionKey,
        id: &SessionKey,
        ended: RunEnd,
    ) -> Option<String> {
        let (changed, message) = {
            let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
            let child = children
                .get_mut(parent)
                .and_then(|list| list.iter_mut().find(|child| child.child == *id))?;
            if !child.state.is_terminal() {
                child.state = ended.into_state();
            }
            let message = if child.state.is_terminal() || child.pending_messages.is_empty() {
                None
            } else {
                child.state = ChildState::Running;
                Some(child.pending_messages.remove(0))
            };
            (Arc::clone(&child.changed), message)
        };
        if message.is_none() {
            changed.notify_waiters();
        }
        message
    }

    /// Queues a child message. Returns true when the child was idle and the
    /// caller should start the queue worker.
    pub(super) fn queue_message(
        &self,
        parent: &SessionKey,
        id: &SessionKey,
        message: String,
    ) -> Option<bool> {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        let child = children
            .get_mut(parent)
            .and_then(|list| list.iter_mut().find(|child| child.child == *id))?;
        if child.state.is_terminal() {
            return None;
        }
        let start = child.state == ChildState::Idle && child.pending_messages.is_empty();
        child.pending_messages.push(message);
        if start {
            child.state = ChildState::Running;
        }
        Some(start)
    }

    /// Takes the next queued message for a child whose current run just ended.
    pub(super) fn take_queued_message(
        &self,
        parent: &SessionKey,
        id: &SessionKey,
    ) -> Option<String> {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        let child = children
            .get_mut(parent)
            .and_then(|list| list.iter_mut().find(|child| child.child == *id))?;
        if child.state.is_terminal() || child.pending_messages.is_empty() {
            return None;
        }
        child.state = ChildState::Running;
        Some(child.pending_messages.remove(0))
    }

    pub(super) fn set_cancel(
        &self,
        parent: &SessionKey,
        id: &SessionKey,
        cancel: Arc<Cancellation>,
    ) {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(child) = children
            .get_mut(parent)
            .and_then(|list| list.iter_mut().find(|child| child.child == *id))
        {
            child.cancel = Some(cancel);
        }
    }

    pub(super) fn mark_stopped(&self, parent: &SessionKey, id: &SessionKey) -> bool {
        let child = {
            let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
            children
                .get_mut(parent)
                .and_then(|list| list.iter_mut().find(|child| child.child == *id))
                .map(|child| {
                    child.state = RunEnd::Stopped.into_state();
                    child.pending_messages.clear();
                    Arc::clone(&child.changed)
                })
        };
        if let Some(changed) = child {
            changed.notify_waiters();
            true
        } else {
            false
        }
    }

    /// Discards messages queued behind a run that is being stopped.
    pub(super) fn clear_pending_messages(&self, parent: &SessionKey, id: &SessionKey) {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(child) = children
            .get_mut(parent)
            .and_then(|list| list.iter_mut().find(|child| child.child == *id))
        {
            child.pending_messages.clear();
        }
    }

    /// Clears the registry before restoring it from durable session records.
    pub(super) fn clear(&self) {
        self.children
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Holds `text` for `child` under `parent` and returns how many are held.
    ///
    /// Only meaningful while the delegator is suspended, but it does not
    /// check: a message arriving for a child that is not suspended is the
    /// caller's to steer, not this registry's to hold.
    pub(super) fn hold(&self, parent: &SessionKey, id: &SessionKey, text: String) -> usize {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        match children
            .get_mut(parent)
            .and_then(|list| list.iter_mut().find(|child| child.child == *id))
        {
            Some(child) => {
                child.held.push(text);
                child.held.len()
            }
            None => 0,
        }
    }

    /// Forgets `child` under `parent`. Used when a child session is deleted.
    pub(super) fn remove(&self, parent: &SessionKey, id: &SessionKey) {
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(list) = children.get_mut(parent) {
            list.retain(|child| child.child != *id);
        }
    }
}

/// The registry behind an `Arc`, so tools can hold it without borrowing the
/// service.
pub(super) type SharedChildren = Arc<ChildRegistry>;

#[cfg(test)]
#[path = "delegator_tests.rs"]
mod tests;
