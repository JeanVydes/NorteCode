//! Child stop and wait tools.

use std::sync::Arc;

use async_trait::async_trait;
use norte::{FnExecutor, ToolContext, ToolOutput};
use norte_code_wire::ChildState;

use super::super::{
    RwLock, Service, SharedChildren, StopArgs, StopMode, WaitArgs, WaitTarget, answer, child_key,
    emit_child_state, parent_key, persist_child_terminal, phase_of, registered_child_state,
    service_of,
};
use crate::service::subagent_views::{ChildStateView, WaitView};

/// `stop_agent` (RFC 8.2.7): stop a child session.
///
/// `graceful` (default) lets its turn end; `kill` cancels it. While the
/// child's delegator is suspended, stopping fails with
/// `DELEGATOR_SUSPENDED` (RFC 8.2.11): a human has taken the child over.
pub(crate) struct StopAgent {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl StopAgent {
    /// A stopper over the shared child registry.
    pub(crate) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(crate) fn into_declaration(self) -> norte::ToolDeclaration<StopArgs> {
        norte::ToolDeclaration::new(
            "stop_agent",
            "Stop a child session you spawned. `mode: graceful` (the default) lets its current turn finish; `kill` cancels it now. A child whose delegator is suspended cannot be stopped: a human has taken it over, and releasing the delegator is what makes it stoppable again.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<StopArgs, ToolOutput> for StopAgent {
    async fn call(&self, _args: StopArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("stop_agent needs a session"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: StopArgs,
    ) -> norte::Result<ToolOutput> {
        let (Some(parent), Some(service)) = (context.session, service_of(&self.service)) else {
            return Ok(ToolOutput::error(
                "stop_agent needs a client attached: a child belongs to a parent session",
            ));
        };
        let Some(parent_key) = parent_key(&service, parent) else {
            return Ok(ToolOutput::error("stop_agent: parent session is unknown"));
        };
        let Ok(child) = child_key(&args.child_session_id) else {
            return Ok(ToolOutput::error("stop_agent needs a child_session_id"));
        };
        let Some(registered) = self.children.get(&parent_key, &child) else {
            return Ok(ToolOutput::error(format!(
                "CHILD_NOT_FOUND: no child `{}` under this session",
                args.child_session_id
            )));
        };
        if registered.suspended {
            return Ok(ToolOutput::error(format!(
                "DELEGATOR_SUSPENDED: the delegator of `{}` is suspended; it cannot be stopped",
                args.child_session_id
            )));
        }
        let mode = args.mode.unwrap_or_default();
        if registered.is_terminal() {
            return Ok(ToolOutput::text(answer(&ChildStateView {
                child_session_id: child.as_str(),
                state: registered_child_state(&registered, phase_of(&service, &child)),
            })));
        }
        let timeout = std::time::Duration::from_secs(args.timeout_s.unwrap_or(30).min(600));
        self.children.clear_pending_messages(&parent_key, &child);
        if mode == StopMode::Kill {
            if let Some(cancel) = &registered.cancel {
                cancel.cancel();
            }
            service.cancel_turn(&child);
        } else if !registered.is_finished() {
            let deadline = tokio::time::Instant::now() + timeout;
            while !self
                .children
                .get(&parent_key, &child)
                .is_some_and(|child| child.is_finished())
            {
                let notified = registered.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self
                    .children
                    .get(&parent_key, &child)
                    .is_some_and(|child| child.is_finished())
                {
                    break;
                }
                if tokio::time::timeout_at(deadline, &mut notified)
                    .await
                    .is_err()
                {
                    if let Some(cancel) = &registered.cancel {
                        cancel.cancel();
                    }
                    service.cancel_turn(&child);
                    break;
                }
            }
        }
        if let Ok((nid, _, _)) = service.require_record(&child) {
            service.jobs.cancel_session(nid);
        }
        self.children.mark_stopped(&parent_key, &child);
        persist_child_terminal(&service, &child, false);
        emit_child_state(
            &self.service,
            parent,
            &child,
            ChildState::Stopped,
            args.reason.as_deref().unwrap_or("state_change"),
        );
        Ok(ToolOutput::text(answer(&ChildStateView {
            child_session_id: child.as_str(),
            state: ChildState::Stopped,
        })))
    }
}

/// What `wait_agent` waits for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WaitUntil {
    /// The current turn ended, and the child can take another message.
    Idle,
    /// The child will not run again.
    Finished,
}

impl WaitUntil {
    fn reached(self, child: &crate::service::delegator::Child) -> bool {
        match self {
            Self::Idle => child.is_finished(),
            Self::Finished => child.is_terminal(),
        }
    }
}

/// `wait_agent` (RFC 8.2.7): wait for a child to settle.
///
/// Waits for the child's turn-completion notification, or until `timeout_s`
/// (default 300). A child that already settled returns at once.
pub(crate) struct WaitAgent {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl WaitAgent {
    /// A waiter over the shared child registry.
    pub(crate) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(crate) fn into_declaration(self) -> norte::ToolDeclaration<WaitArgs> {
        norte::ToolDeclaration::new(
            "wait_agent",
            "Wait for a child session you spawned to settle. Returns when the child is idle, or times out after `timeout_s` seconds (default 300) and says so.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<WaitArgs, ToolOutput> for WaitAgent {
    async fn call(&self, _args: WaitArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("wait_agent needs a session"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: WaitArgs,
    ) -> norte::Result<ToolOutput> {
        let (Some(parent), Some(service)) = (context.session, service_of(&self.service)) else {
            return Ok(ToolOutput::error(
                "wait_agent needs a client attached: a child belongs to a parent session",
            ));
        };
        let Some(parent_key) = parent_key(&service, parent) else {
            return Ok(ToolOutput::error("wait_agent: parent session is unknown"));
        };
        let Ok(child) = child_key(&args.child_session_id) else {
            return Ok(ToolOutput::error("wait_agent needs a child_session_id"));
        };
        let Some(child_record) = self.children.get(&parent_key, &child) else {
            return Ok(ToolOutput::error(format!(
                "CHILD_NOT_FOUND: no child `{}` under this session",
                args.child_session_id
            )));
        };
        let until = match args.until.unwrap_or_default() {
            WaitTarget::Idle => WaitUntil::Idle,
            WaitTarget::Finished => WaitUntil::Finished,
        };
        let timeout = std::time::Duration::from_secs(args.timeout_s.unwrap_or(300).min(600));
        let deadline = tokio::time::Instant::now() + timeout;
        let changed = Arc::clone(&child_record.changed);
        let mut timed_out = false;
        loop {
            let current = self.children.get(&parent_key, &child);
            let complete = current.is_some_and(|child| until.reached(&child));
            if complete {
                break;
            }
            let notified = changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let current = self.children.get(&parent_key, &child);
            let complete = current.is_some_and(|child| until.reached(&child));
            if complete {
                break;
            }
            let cancellation = context.cancellation();
            tokio::select! {
                () = &mut notified => {}
                () = tokio::time::sleep_until(deadline) => {
                    timed_out = true;
                    break;
                }
                () = cancellation.cancelled() => {
                    return Ok(ToolOutput::error("CANCELLED: wait_agent was interrupted"));
                }
            }
        }
        let final_child = self.children.get(&parent_key, &child);
        let phase = phase_of(&service, &child);
        let state = final_child.as_ref().map_or(ChildState::Idle, |child| {
            registered_child_state(child, phase)
        });
        Ok(ToolOutput::text(answer(&WaitView {
            child_session_id: child.as_str(),
            state,
            timed_out,
        })))
    }
}
