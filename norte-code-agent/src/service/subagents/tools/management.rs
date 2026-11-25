//! Child listing, approval, question and handoff tools.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use norte::{FnExecutor, ToolContext, ToolOutput};

use super::super::{
    ApprovalDecision, ApproveArgs, AskDelegatorArgs, HandoffArgs, ListArgs, Service,
    SharedChildren, answer, answer_of, parent_key, phase_of, registered_child_state, service_of,
};
use crate::service::subagent_views::{HandoffAccepted, ListedChild, ListedChildren};

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
            "List the child sessions this session has spawned, with their state, plus the agent profiles this session may delegate to. Use it to find a lost child id (the ids come from `spawn_agent`) and to learn which `agent_id` values `spawn_agent` accepts for this session's profile.",
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
                && !filter.contains(&state)
            {
                continue;
            }
            items.push(ListedChild {
                child_session_id: child.child.as_str().to_owned(),
                state,
                tool_call_id: child.tool_call_id.clone(),
            });
            if items.len() >= limit {
                break;
            }
        }
        let delegates = service.delegates_for(&parent_key);
        Ok(ToolOutput::text(answer(&ListedChildren {
            children: items,
            delegates,
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
        let _ = &self.children;
        let _ = &self.questions;
        let action = match args.decision {
            ApprovalDecision::Approve => "approve",
            ApprovalDecision::Deny => "deny",
        };
        Ok(ToolOutput::error(format!(
            "APPROVAL_NOT_PENDING: cannot {action} call `{}` in `{}` because no approval is pending",
            args.tool_call_id, args.child_session_id,
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
