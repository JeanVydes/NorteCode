//! The delegation tools themselves.

use std::sync::Arc;

use async_trait::async_trait;
use norte::{FnExecutor, ToolContext, ToolOutput};

use crate::service::steering::Steered;
use norte::id::SessionId;
use norte_code_wire::SessionKey;

use super::{
    ApproveArgs, AskDelegatorArgs, ChildState, HandoffArgs, InspectArgs, ListArgs, MessageArgs,
    RwLock, Service, SharedChildren, StopArgs, WaitArgs, answer, answer_of, child_key, clip,
    emit_child_state, parent_key, persist_child_terminal, phase_of, registered_child_state,
    schedule_child_turn, service_of,
};
use crate::service::subagent_views::{
    AcceptedMessage, ChildStateView, ChildStatus, HandoffAccepted, HeldMessage, InspectView,
    JobView, ListedChild, ListedChildren, WaitView,
};

/// `inspect_agent` (RFC 8.2.7): look at a child the parent spawned.
pub(crate) struct InspectAgent {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl InspectAgent {
    /// An inspector over the shared child registry.
    pub(crate) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(crate) fn into_declaration(self) -> norte::ToolDeclaration<InspectArgs> {
        norte::ToolDeclaration::new(
            "inspect_agent",
            "Look at a child session you spawned: its state and what it last said. `view: status` (the default) is the one to reach for in a turn; `transcript` returns its history, `jobs` its background jobs.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<InspectArgs, ToolOutput> for InspectAgent {
    async fn call(&self, _args: InspectArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("inspect_agent needs a session"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: InspectArgs,
    ) -> norte::Result<ToolOutput> {
        let (Some(parent), Some(service)) = (context.session, service_of(&self.service)) else {
            return Ok(ToolOutput::error(
                "inspect_agent needs a client attached: a child belongs to a parent session",
            ));
        };
        let Some(parent_key) = parent_key(&service, parent) else {
            return Ok(ToolOutput::error(
                "inspect_agent: parent session is unknown",
            ));
        };
        let Ok(child) = child_key(&args.child_session_id) else {
            return Ok(ToolOutput::error("inspect_agent needs a child_session_id"));
        };
        let view = args.view.as_deref().unwrap_or("status");
        match view {
            "status" => {
                let Some(registered) = self.children.get(&parent_key, &child) else {
                    return Ok(ToolOutput::error(format!(
                        "CHILD_NOT_FOUND: no child `{}` under this session; use `list_agents`",
                        args.child_session_id
                    )));
                };
                let phase = phase_of(&service, &child);
                let state = registered_child_state(&registered, phase);
                let title = service
                    .lock_sessions()
                    .get(&child)
                    .and_then(|record| record.title.clone())
                    .unwrap_or_default();
                Ok(ToolOutput::text(answer(&InspectView::Status {
                    child_session_id: child.as_str(),
                    status: ChildStatus {
                        state,
                        title,
                        delegator_suspended: registered.suspended,
                    },
                })))
            }
            "transcript" => Ok(inspect_transcript(&service, &child, &args).await),
            "jobs" => Ok(inspect_jobs(&service, &child, &args)),
            "events" => Ok(ToolOutput::text(answer(&InspectView::Events {
                child_session_id: child.as_str(),
                items: Vec::new(),
                truncated: false,
                note: "the event view reads the RAEP replay window, which the service does not expose to the model yet",
            }))),
            other => Ok(ToolOutput::error(format!(
                "inspect_agent: unknown view `{other}`; use status, transcript, events or jobs"
            ))),
        }
    }
}

async fn inspect_transcript(
    service: &Service,
    child: &SessionKey,
    args: &InspectArgs,
) -> ToolOutput {
    let (nid, spec, _) = match service.require_record(child) {
        Ok(found) => found,
        Err(error) => return ToolOutput::error(format!("CHILD_NOT_FOUND: {error}")),
    };
    let Some(spec) = spec else {
        return ToolOutput::error("inspect_agent: child has no model");
    };
    let engine = match service.engine(&spec, Some(child)) {
        Ok(engine) => engine,
        Err(error) => return ToolOutput::error(format!("inspect_agent: {error}")),
    };
    let history = match engine.history(nid).await {
        Ok(history) => history,
        Err(error) => return ToolOutput::error(format!("inspect_agent: {error}")),
    };
    let limit = args.limit.unwrap_or(20).clamp(1, 200);
    let from = usize::try_from(args.from_seq.unwrap_or(0)).unwrap_or(usize::MAX);
    let items: Vec<String> = history
        .iter()
        .skip(from)
        .take(limit)
        .map(|message| {
            let text = norte::Message::text(message);
            format!("{:?}: {}", message.role, clip(&text, 400))
        })
        .collect();
    ToolOutput::text(answer(&InspectView::Transcript {
        child_session_id: child.as_str(),
        truncated: history.len() > from.saturating_add(items.len()),
        items,
    }))
}

fn inspect_jobs(service: &Service, child: &SessionKey, args: &InspectArgs) -> ToolOutput {
    let nid = match service.require_record(child) {
        Ok((nid, _, _)) => nid,
        Err(error) => return ToolOutput::error(format!("CHILD_NOT_FOUND: {error}")),
    };
    let jobs = service.jobs.list(Some(nid));
    let limit = args.limit.unwrap_or(20).clamp(1, 100);
    let jobs = jobs
        .iter()
        .take(limit)
        .map(|job| JobView {
            job_id: &job.id,
            command: &job.label,
            status: job.state.label(),
        })
        .collect();
    ToolOutput::text(answer(&InspectView::Jobs {
        child_session_id: child.as_str(),
        jobs,
    }))
}

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
        delivery: &str,
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
        if is_running && delivery == "steer" {
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
                ),
            );
            return ToolOutput::text(answer(&AcceptedMessage {
                accepted: true,
                delivery: "steer",
                child_state: "RUNNING",
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
        if is_running && delivery == "interrupt" {
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
            child_state: "RUNNING",
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
        let delivery = args.delivery.as_deref().unwrap_or("steer");
        if !matches!(delivery, "steer" | "queue" | "interrupt") {
            return Ok(ToolOutput::error(format!(
                "message_agent: unknown delivery `{delivery}`; use steer, queue or interrupt"
            )));
        }
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
        let mode = args.mode.as_deref().unwrap_or("graceful");
        if !matches!(mode, "graceful" | "kill") {
            return Ok(ToolOutput::error(format!(
                "stop_agent: unknown mode `{mode}`; use graceful or kill"
            )));
        }
        if registered.is_terminal() {
            return Ok(ToolOutput::text(answer(&ChildStateView {
                child_session_id: child.as_str(),
                state: registered_child_state(&registered, phase_of(&service, &child)),
            })));
        }
        let timeout = std::time::Duration::from_secs(args.timeout_s.unwrap_or(30).min(600));
        self.children.clear_pending_messages(&parent_key, &child);
        if mode == "kill" {
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
            state: "STOPPED",
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
        // The argument is the model's text, so it is validated here: once,
        // into a value: and the two waits below ask the same question of it.
        let until = match args.until.as_deref().unwrap_or("idle") {
            "idle" => WaitUntil::Idle,
            "finished" => WaitUntil::Finished,
            other => {
                return Ok(ToolOutput::error(format!(
                    "wait_agent: unknown until `{other}`; use idle or finished"
                )));
            }
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
        let state = final_child
            .as_ref()
            .map_or("IDLE", |child| registered_child_state(child, phase));
        Ok(ToolOutput::text(answer(&WaitView {
            child_session_id: child.as_str(),
            state,
            timed_out,
        })))
    }
}

/// `list_agents` (RFC 8.2.7): the children a session has spawned.
pub(crate) struct ListAgents {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl ListAgents {
    /// A lister over the shared child registry.
    pub(crate) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(crate) fn into_declaration(self) -> norte::ToolDeclaration<ListArgs> {
        norte::ToolDeclaration::new(
            "list_agents",
            "List the child sessions this session has spawned, with their state. Use it when you have lost a child id; the ids come from `spawn_agent`.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<ListArgs, ToolOutput> for ListAgents {
    async fn call(&self, _args: ListArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("list_agents needs a session"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: ListArgs,
    ) -> norte::Result<ToolOutput> {
        let (Some(parent), Some(service)) = (context.session, service_of(&self.service)) else {
            return Ok(ToolOutput::error(
                "list_agents needs a client attached: a child belongs to a parent session",
            ));
        };
        let Some(parent_key) = parent_key(&service, parent) else {
            return Ok(ToolOutput::error("list_agents: parent session is unknown"));
        };
        let limit = args.limit.unwrap_or(100).clamp(1, 100);
        let mut items: Vec<ListedChild> = Vec::new();
        for child in self.children.of(&parent_key) {
            let phase = phase_of(&service, &child.child);
            let state = registered_child_state(&child, phase);
            if let Some(filter) = &args.state
                && !filter.iter().any(|wanted| wanted == state)
            {
                continue;
            }
            items.push(ListedChild {
                child_session_id: child.child.as_str().to_owned(),
                state: state.to_owned(),
                tool_call_id: child.tool_call_id.clone(),
            });
            if items.len() >= limit {
                break;
            }
        }
        Ok(ToolOutput::text(answer(&ListedChildren {
            children: items,
        })))
    }
}

/// `approve_child_call` (RFC 8.2.9): approve or deny a tool call a child made.
///
/// Approvals in this host are rules, not interactive prompts (REQ 13.6 is out
/// of scope here): a child that needs an approval is refused by the rule
/// interceptor before the call runs, so there is nothing pending to approve.
/// The tool exists so the model can record a decision on the stream; it
/// answers `APPROVAL_NOT_PENDING` for any call id.
pub(crate) struct ApproveChildCall {
    children: SharedChildren,
    questions: Arc<norte_code_protocol::tools::QuestionBroker>,
}

impl ApproveChildCall {
    /// An approver over the shared child registry and the question broker.
    pub(crate) fn new(
        children: SharedChildren,
        questions: Arc<norte_code_protocol::tools::QuestionBroker>,
    ) -> Self {
        Self {
            children,
            questions,
        }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(crate) fn into_declaration(self) -> norte::ToolDeclaration<ApproveArgs> {
        norte::ToolDeclaration::new(
            "approve_child_call",
            "Approve or deny a tool call a child session made. Approvals in this host are deterministic rules (`/allow`, `/deny`), so a call that needed one was already refused before it ran: this answers `APPROVAL_NOT_PENDING` for any id.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<ApproveArgs, ToolOutput> for ApproveChildCall {
    async fn call(&self, _args: ApproveArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("approve_child_call needs a session"))
    }

    async fn call_with_context(
        &self,
        _context: &ToolContext,
        args: ApproveArgs,
    ) -> norte::Result<ToolOutput> {
        if !matches!(args.decision.as_str(), "approve" | "deny") {
            return Ok(ToolOutput::error(format!(
                "approve_child_call: unknown decision `{}`; use approve or deny",
                args.decision
            )));
        }
        let _ = &self.children;
        let _ = &self.questions;
        Ok(ToolOutput::error(format!(
            "APPROVAL_NOT_PENDING: no pending approval for `{}` in `{}`",
            args.tool_call_id, args.child_session_id
        )))
    }
}

/// `ask_delegator` (RFC 8.2.9): ask the parent (or a human, when the parent
/// is one) a question and wait for the answer.
///
/// It reuses the question broker: the question is asked on this session, so
/// whoever answers a parked question answers this one too.
pub(crate) struct AskDelegator {
    questions: Arc<norte_code_protocol::tools::QuestionBroker>,
}

impl AskDelegator {
    /// A delegator asker over the question broker.
    pub(crate) fn new(questions: Arc<norte_code_protocol::tools::QuestionBroker>) -> Self {
        Self { questions }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(crate) fn into_declaration(self) -> norte::ToolDeclaration<AskDelegatorArgs> {
        norte::ToolDeclaration::new(
            "ask_delegator",
            "Ask the parent a question and wait for its answer. Use it when a child needs a decision only the parent (or the person behind it) can make. The question is asked on this session, so whoever answers a parked question answers this one.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<AskDelegatorArgs, ToolOutput> for AskDelegator {
    async fn call(&self, _args: AskDelegatorArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("ask_delegator needs a session"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: AskDelegatorArgs,
    ) -> norte::Result<ToolOutput> {
        let options = args.options.unwrap_or_default();
        let multiple = options.len() > 1;
        match self
            .questions
            .ask(context, args.question, options, multiple)
            .await
        {
            Ok(answer) => Ok(ToolOutput::text(answer_of(answer))),
            Err(error) => Ok(ToolOutput::error(format!("DELEGATOR_TIMEOUT: {error}"))),
        }
    }
}

/// `handoff_agent` (RFC 8.3): hand the rest of the turn to another agent.
///
/// The handoff takes effect at the end of the turn (`takes_effect:
/// turn_end`). This host runs one configured agent, so a handoff to any
/// other name is refused with `HANDOFF_NOT_ALLOWED`.
pub(crate) struct HandoffAgent {
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl HandoffAgent {
    /// A handoff over the service.
    pub(crate) fn new(service: Arc<RwLock<Option<Arc<Service>>>>) -> Self {
        Self { service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(crate) fn into_declaration(self) -> norte::ToolDeclaration<HandoffArgs> {
        norte::ToolDeclaration::new(
            "handoff_agent",
            "Hand the rest of this turn to another agent. It takes effect when the turn ends. This host runs one configured agent, so a handoff to any other agent is refused.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<HandoffArgs, ToolOutput> for HandoffAgent {
    async fn call(&self, _args: HandoffArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("handoff_agent needs a session"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: HandoffArgs,
    ) -> norte::Result<ToolOutput> {
        let Some(service) = service_of(&self.service) else {
            return Ok(ToolOutput::error("handoff_agent: no service"));
        };
        let Some(parent) = context.session else {
            return Ok(ToolOutput::error("handoff_agent needs a session"));
        };
        let Some(parent_key) = parent_key(&service, parent) else {
            return Ok(ToolOutput::error(
                "handoff_agent: parent session is unknown",
            ));
        };
        let current = service
            .lock_sessions()
            .get(&parent_key)
            .and_then(|record| record.spec.clone())
            .map(|spec| spec.model)
            .unwrap_or_default();
        if args.agent_id == current || args.agent_id.is_empty() {
            return Ok(ToolOutput::text(answer(&HandoffAccepted {
                accepted: true,
                takes_effect: "turn_end",
            })));
        }
        let _ = &args.carry_context;
        Ok(ToolOutput::error(format!(
            "HANDOFF_NOT_ALLOWED: this host runs one agent ({current}); `{}` is not available",
            args.agent_id
        )))
    }
}
