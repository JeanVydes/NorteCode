//! The execution of the tools a model asked for.
//!
//! A turn that ends with tool calls comes here: the calls are gathered from the
//! head message, each one runs, and the results are committed as one `Tool`
//! message.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::runtime::Commit;
use crate::runtime::Node;
use crate::runtime::Phase;
use crate::runtime::Session;
use crate::runtime::event::EngineEvent;
use crate::runtime::interceptor::{ConsultationPoint, Interception, InterceptionContext};
use norte::domain::message::Message;
use norte::domain::part::{ToolCallPart, ToolResultPart};
use norte::error::{Error, Result};
use norte::id::ToolName;
use norte::tool::ToolCallInfo;

use super::{CALL_CANCELLED_REASON, CallSite, Harness, PreparedCall, ToolTurn, execute_call};

impl Harness {
    /// Settles every call before returning, including on cancellation/hook error.
    pub(crate) async fn run_tools(
        &self,
        session: &mut Session,
        sender: &mpsc::Sender<EngineEvent>,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        let head = session
            .head
            .ok_or_else(|| Error::InvalidInput("AwaitingTool session has no head".to_owned()))?;
        let head_node = self.store.node(head).await?;
        let calls: Vec<ToolCallPart> = head_node
            .message
            .tool_calls()
            .into_iter()
            .cloned()
            .collect();
        if calls.is_empty() {
            return Err(Error::InvalidInput(
                "AwaitingTool head has no tool calls".to_owned(),
            ));
        }

        let mut results = Vec::with_capacity(calls.len());
        let execution: Result<()> = async {
            let turn = ToolTurn { session, sender };
            let history = if self.interceptors.is_empty() || cancellation.is_cancelled() {
                None
            } else {
                Some(self.store.history(Some(head)).await?)
            };
            for call in &calls {
                if cancellation.is_cancelled() {
                    break;
                }
                if let Some(reason) = self
                    .refuse_call(session, sender, history.as_deref(), call)
                    .await?
                {
                    results.push(self.refused_call(session, sender, call, reason).await);
                    continue;
                }
                // The approval may have waited while cancellation arrived.
                if cancellation.is_cancelled() {
                    break;
                }
                self.run_one_tool(&turn, call, &mut results, cancellation)
                    .await?;
            }
            Ok(())
        }
        .await;

        // Preserve completed effects even if a later approval fails. Returning
        // first would leave AwaitingTool at the old head and execute them again
        // on resume. Calls not executed receive an explicit error in order.
        let reason = execution.as_ref().err().map_or_else(
            || CALL_CANCELLED_REASON.to_owned(),
            |error| format!("tool call not run: {error}"),
        );
        for call in calls.iter().skip(results.len()) {
            results.push(ToolResultPart::error(call.call_id, reason.clone()));
        }
        let node = Node::message(Message::tool(results), Some(head));
        let node_id = node.id;
        let metadata = session.metadata.clone();
        *session = self
            .commit(
                Commit {
                    session_id: session.id,
                    expected_version: Some(session.version),
                    nodes: vec![node],
                    head: Some(node_id),
                    phase: Phase::Idle,
                    turn: session.turn,
                    metadata: Some(metadata),
                },
                sender,
            )
            .await?;
        execution
    }

    /// Answers a call the host refused, reported the way a call that ran is.
    ///
    /// The refusal is a tool result the model reads, so the turn carries on.
    /// A host watching only the events, though, would see the assistant
    /// announce a call and never hear of it again: no row opened, nothing to
    /// rewrite into the failure it is. The two events keep that view whole,
    /// with no duration because nothing ran.
    async fn refused_call(
        &self,
        session: &Session,
        sender: &mpsc::Sender<EngineEvent>,
        call: &ToolCallPart,
        reason: String,
    ) -> ToolResultPart {
        // A name the registry would reject never ran either, and there is no
        // tool name to report: the result is all the model gets.
        if let Ok(tool) = ToolName::new(call.name.clone()) {
            let started = EngineEvent::ToolCallStarted {
                session: session.id,
                turn: session.turn,
                call_id: call.call_id,
                tool: tool.clone(),
            };
            self.observers.emit(&started);
            let _ = sender.send(started).await;
            let finished = EngineEvent::ToolCallFinished {
                session: session.id,
                turn: session.turn,
                call_id: call.call_id,
                tool,
                is_error: true,
                duration_ms: 0,
            };
            self.observers.emit(&finished);
            let _ = sender.send(finished).await;
        }
        ToolResultPart::error(call.call_id, reason)
    }

    /// The host metadata for one execution, consulted immediately before the
    /// executor runs.
    pub(super) fn call_metadata(
        &self,
        session: &Session,
        call: &ToolCallPart,
        tool: &ToolName,
        arguments: &Value,
    ) -> BTreeMap<String, Value> {
        self.config
            .tool_metadata
            .as_ref()
            .map_or_else(BTreeMap::new, |provider| {
                provider.metadata(&ToolCallInfo {
                    session: session.id,
                    turn: session.turn,
                    call_id: call.call_id,
                    tool: tool.clone(),
                    arguments: arguments.clone(),
                })
            })
    }

    /// Runs one call.
    async fn run_one_tool(
        &self,
        turn: &ToolTurn<'_>,
        call: &ToolCallPart,
        results: &mut Vec<ToolResultPart>,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        let Ok(tool_name) = ToolName::new(call.name.clone()) else {
            results.push(ToolResultPart::error(
                call.call_id,
                format!("invalid tool name: {}", call.name),
            ));
            return Ok(());
        };

        let metadata = self.call_metadata(turn.session, call, &tool_name, &call.arguments);
        let result = execute_call(
            Arc::clone(&self.tools),
            Arc::clone(&self.observers),
            turn.sender.clone(),
            CallSite {
                session: turn.session.id,
                turn: turn.session.turn,
            },
            PreparedCall {
                call: call.clone(),
                tool: tool_name,
                arguments: call.arguments.clone(),
                metadata,
            },
            cancellation.clone(),
        )
        .await;
        results.push(result);
        Ok(())
    }

    /// Asks the host whether one call may run.
    ///
    /// This is the last point where the call's name, its id and its arguments
    /// are all known and nothing has run yet, which is what a decision about
    /// one call needs. A refusal is not an error: it is a result the model
    /// reads, so the call it asked for is answered and it can adapt.
    ///
    /// `Ok(None)` lets the call run.
    async fn refuse_call(
        &self,
        session: &Session,
        sender: &mpsc::Sender<EngineEvent>,
        history: Option<&[Message]>,
        call: &ToolCallPart,
    ) -> Result<Option<String>> {
        if self.interceptors.is_empty() {
            return Ok(None);
        }
        let context = InterceptionContext {
            point: ConsultationPoint::ToolCall { call: call.clone() },
            session: session.id,
            turn: session.turn,
            head: session.head,
            messages: history.unwrap_or_default(),
            events: sender,
        };
        for interceptor in self.interceptors.interceptors() {
            match interceptor.intercept(&context).await? {
                Interception::RefuseCall { reason } => return Ok(Some(reason)),
                // A host that injects, stops or continues at this point is
                // answering a question that was not asked: the two decisions
                // here are that the call runs or that it does not.
                Interception::Continue | Interception::Inject(_) | Interception::Stop { .. } => {}
            }
        }
        Ok(None)
    }
}
