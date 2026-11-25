//! Subagents: a delegated turn on a child session.
//!
//! `subagent` is a tool with the same two shapes `shell` has: waited for, or
//! let go of. Waited for, it runs one turn on a child session and returns the
//! child's final message as its own result. Let go of (`detach: true`), it
//! becomes a job the registry holds, its result arrives as a steered message
//! when the child settles, and `manage_jobs` can read it and stop it.
//!
//! The child is a session like any other, created from the same service. The
//! constraints are the service's, not the engine's: the child never sees the
//! internal `subagent` spelling, while `spawn_agent` checks a bounded nesting
//! depth. How many may run at once is a semaphore, not a queue: past the cap,
//! the call is an error the model reads.
//!
//! The wrapper is here and not in the broker for the same reason `ShellCalls`
//! wraps the shell: what a job needs, a registration, an output that a reader
//! can find later, an end reported to a model that has no call left to answer
//!: is the host's business, and the engine knows nothing about it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use norte::{CallId, Cancellation, FnExecutor, SessionId, ToolContext, ToolOutput};
use norte_code_wire::{ChildState, SessionKey};
use norte_code_wire::{ServerEvent, ServerEventKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Service;
use super::delegator::SharedChildren;
use super::detach::DetachRegistry;
use super::jobs::JobRegistry;
use super::steering::{Steered, Steering};
use super::subagent_views::{ChildResult, DelegatorAnswer};

/// The child session id is the parent-visible name of a child: the
/// `SessionKey` the service created for it, which is what `children_of`
/// returns and what the delegating tools address.
type ChildId = SessionKey;

/// The arguments of `spawn_agent` (RFC 8.2.5).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SubagentArgs {
    /// The one task, with everything the child needs. The child starts blank.
    pub prompt: String,
    /// The child session's title.
    pub title: Option<String>,
    /// Whether the call is let go of as a job.
    pub detach: bool,
    /// The child must return a JSON value matching this schema, when set.
    pub output_schema: Option<serde_json::Value>,
    /// Whether the child is tied to the parent turn or session.
    pub lifetime: ChildLifetime,
    /// The configured child agent to use, when one is requested explicitly.
    /// It must be in the parent profile's `delegates`; when omitted, the
    /// profile's `subagent` default (or the parent's own) is used. `list_agents`
    /// shows the allowed `delegates` for this session.
    pub agent_id: Option<String>,
    /// Whether to include a redacted excerpt of the parent history.
    pub context_mode: Option<ContextMode>,
}

/// What one run of a child session produced.
#[derive(Debug)]
pub(super) struct SubagentOutcome {
    /// The child's final message, when it committed one.
    pub text: String,
    /// Whether the child ended with an error.
    pub is_error: bool,
    /// The child session this run happened on, when the runner created one.
    ///
    /// It is what `spawn_agent` returns and what the delegating tools
    /// address the child by (RFC 8.2.7): without it the parent has a result
    /// but no session to inspect, message or stop.
    pub child: Option<ChildId>,
    /// Whether the child's output validated against the caller's
    /// `output_schema`, when one was given (RFC 8.2.6).
    pub output_schema_valid: Option<bool>,
}

/// Input to one delegated run.
pub(super) struct SubagentRequest {
    pub(super) prompt: String,
    pub(super) parent: SessionId,
    pub(super) tool_call_id: String,
    pub(super) output_schema: Option<serde_json::Value>,
    pub(super) lifetime: ChildLifetime,
    pub(super) agent_id: Option<String>,
    pub(super) context_mode: Option<ContextMode>,
}

struct DetachedRun {
    parent: SessionId,
    call_id: CallId,
    tool_call_id: String,
    label: String,
    args: SubagentArgs,
    cancel: Arc<Cancellation>,
    token: Arc<Cancellation>,
}

/// Where a subagent's turn is run from.
///
/// The service implements this: create the child, build its engine, run one
/// turn, and read back what it committed. The tool does not reach into the
/// service's lock-guarded state: it asks through this trait.
#[async_trait]
pub(super) trait SubagentRunner: Send + Sync {
    /// Runs one turn of `prompt` on a fresh child session.
    ///
    /// `label` is what the job registry prints for the run; `cancel` stops it.
    async fn run(&self, request: SubagentRequest, cancel: Arc<Cancellation>) -> SubagentOutcome;
}

/// A gate that enforces a cap on concurrent subagents.
pub(super) struct Gate {
    cap: usize,
    running: AtomicUsize,
}

impl Gate {
    /// Takes a slot, or reports that there is none.
    pub(super) fn acquire(&self) -> std::result::Result<(), String> {
        let mut running = self.running.load(Ordering::Acquire);
        loop {
            if running >= self.cap {
                return Err(format!(
                    "too many subagents running ({} at once); stop one with manage_jobs or wait",
                    self.cap
                ));
            }
            match self.running.compare_exchange_weak(
                running,
                running + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(observed) => running = observed,
            }
        }
    }

    /// Gives a slot back when a run ends.
    pub(super) fn release_slot(&self) {
        // `load` followed by `fetch_sub` can underflow if two callers release
        // the last slot concurrently. Update atomically and keep the count
        // valid even if a caller accidentally releases twice.
        let mut running = self.running.load(Ordering::Acquire);
        while running > 0 {
            match self.running.compare_exchange_weak(
                running,
                running - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => running = observed,
            }
        }
    }
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("cap", &self.cap)
            .finish_non_exhaustive()
    }
}

/// The tool itself: a runner, plus where its jobs and its detach tokens live.
pub(super) struct SubagentTool {
    runner: Arc<dyn SubagentRunner>,
    jobs: Arc<JobRegistry>,
    steering: Arc<Steering>,
    detaches: Arc<DetachRegistry>,
    /// The children this service has spawned, by parent (RFC 8.2).
    children: SharedChildren,
    /// The service, so a finished subagent can start a turn of its own when
    /// the flag is on.
    service: Arc<RwLock<Option<Arc<Service>>>>,
    gate: Arc<Gate>,
}

#[cfg(test)]
impl SubagentTool {
    /// A tool backed by `runner`, capped at `max_concurrent` runs.
    pub(super) fn new(
        runner: Arc<dyn SubagentRunner>,
        jobs: Arc<JobRegistry>,
        steering: Arc<Steering>,
        detaches: Arc<DetachRegistry>,
        service: Arc<RwLock<Option<Arc<Service>>>>,
        max_concurrent: usize,
    ) -> Self {
        Self::with_children(
            runner,
            jobs,
            steering,
            detaches,
            Arc::new(super::delegator::ChildRegistry::new()),
            service,
            max_concurrent,
        )
    }
}

impl SubagentTool {
    /// A tool backed by `runner` and a shared child registry, capped at
    /// `max_concurrent` runs. The registry is what the delegating tools
    /// (inspect, message, stop, wait, list) read, so `spawn_agent` and the
    /// rest have to share it.
    pub(super) fn with_children(
        runner: Arc<dyn SubagentRunner>,
        jobs: Arc<JobRegistry>,
        steering: Arc<Steering>,
        detaches: Arc<DetachRegistry>,
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
        max_concurrent: usize,
    ) -> Self {
        Self {
            runner,
            jobs,
            steering,
            detaches,
            children,
            service,
            gate: Arc::new(Gate {
                cap: max_concurrent.max(1),
                running: AtomicUsize::new(0),
            }),
        }
    }

    /// The gate, for tests.
    #[cfg(test)]
    pub(super) fn gate_for_test(&self) -> &Gate {
        &self.gate
    }

    /// RAEP delegation equivalent: `spawn_agent`.
    ///
    /// A child created through it is a full session the parent can attach
    /// to and inspect. The other delegation tools (`inspect_agent`,
    /// `message_agent`, `stop_agent`, `wait_agent`, `list_agents`,
    /// `approve_child_call`, `ask_delegator`, `handoff_agent`) are separate
    /// declarations over the same child registry, registered alongside this
    /// one in `Service::tools`.
    pub(super) fn into_delegation_declaration(self) -> norte::ToolDeclaration<SpawnArgs> {
        norte::ToolDeclaration::new(
            "spawn_agent",
            "Create a child session (a sub-agent) and run `task` as its first turn, returning its final message ca. 2 KiB. The child is a full session: it has its own id, history and tools, and the parent can later inspect it, message it, stop it or wait for it (`inspect_agent`, `message_agent`, `stop_agent`, `wait_agent`). With `detach: true` the call becomes a job. A child cannot spawn past the delegation depth limit (3).",
            Arc::new(SpawnTool::new(self)),
        )
    }

    fn emit_child_update(&self, parent: SessionId, outcome: &SubagentOutcome) {
        emit_child_update(&self.children, &self.service, parent, outcome);
    }
}

fn emit_child_update(
    children: &SharedChildren,
    service_slot: &Arc<RwLock<Option<Arc<Service>>>>,
    parent: SessionId,
    outcome: &SubagentOutcome,
) {
    if let Some(child) = &outcome.child
        && let Some(service) = service_of(service_slot)
        && let Some(parent_key) = parent_key(&service, parent)
    {
        let was_terminal = children
            .get(&parent_key, child)
            .is_some_and(|registered| registered.is_terminal());
        if was_terminal {
            return;
        }
        let next_message = children.finish_and_take_message(
            &parent_key,
            child,
            if outcome.is_error {
                super::delegator::RunEnd::Failed
            } else {
                super::delegator::RunEnd::Idle
            },
        );
        let registered = children.get(&parent_key, child);
        let (state, reason) = match registered.as_ref() {
            Some(child) if child.is_failed() => (ChildState::Failed, "turn_failed"),
            Some(child) if child.is_terminal() => (ChildState::Stopped, "state_change"),
            Some(child) if !child.is_finished() => (ChildState::Running, "turn_finished"),
            Some(_) => (ChildState::Idle, "turn_finished"),
            None => (ChildState::Failed, "turn_failed"),
        };
        if let Some(child) = registered.filter(super::delegator::Child::is_terminal) {
            persist_child_terminal(&service, &child.child, child.is_failed());
        }
        service.emit_delegation(ServerEvent::new(
            0,
            Some(parent_key),
            ServerEventKind::ChildSessionUpdated {
                child_session_id: child.as_str().to_owned(),
                state,
                reason: reason.to_owned(),
                result: Some(child_result(outcome)),
                output_schema_valid: outcome.output_schema_valid,
            },
        ));
        if let Some(message) = next_message {
            schedule_child_turn(
                service,
                Arc::clone(service_slot),
                Arc::clone(children),
                parent,
                child.clone(),
                message,
            );
        }
    }
}

fn persist_child_terminal(service: &Arc<Service>, child: &SessionKey, failed: bool) {
    if let Some(record) = service.lock_sessions().get_mut(child) {
        record.child_terminal = Some(if failed {
            super::session_record::ChildTerminalState::Failed
        } else {
            super::session_record::ChildTerminalState::Stopped
        });
    }
    let service = Arc::clone(service);
    let child = child.clone();
    tokio::spawn(async move {
        if let Err(error) = service.persist(&child).await {
            tracing::warn!(%error, session = %child, "could not persist child terminal state");
        }
    });
}

fn emit_child_state(
    service_slot: &Arc<RwLock<Option<Arc<Service>>>>,
    parent: SessionId,
    child: &SessionKey,
    state: ChildState,
    reason: &str,
) {
    if let Some(service) = service_of(service_slot)
        && let Some(parent_key) = parent_key(&service, parent)
    {
        service.emit_delegation(ServerEvent::new(
            0,
            Some(parent_key),
            ServerEventKind::ChildSessionUpdated {
                child_session_id: child.as_str().to_owned(),
                state,
                reason: reason.to_owned(),
                result: None,
                output_schema_valid: None,
            },
        ));
    }
}

/// What a detached subagent hands back before the child has anything to show.
///
/// It is a normal tool result, which is the point: the conversation stays
/// coherent: a `tool_call` always has its `tool_result`: and the child
/// reports separately when it has something to say.
fn started(id: &str, label: &str) -> ToolOutput {
    ToolOutput::text(format!(
        "Subagent `{id}` is running in the background: {label}\n\n\
         Its result will arrive as a message when it ends. Use `manage_jobs` with \
         `action: check` and this id to read what it has written so far, or `action: stop` \
         to end it."
    ))
}

/// What the model reads when a subagent job ends.
fn report(id: &str, outcome: &SubagentOutcome) -> String {
    let head = if outcome.is_error {
        "Subagent error"
    } else {
        "Subagent result"
    };
    let clipped: String = outcome.text.chars().take(4000).collect();
    let tail = if outcome.text.chars().count() > 4000 {
        "\n[result truncated; read the rest with manage_jobs]"
    } else {
        ""
    };
    format!("{head} from `{id}`.\n\n{clipped}{tail}")
}

/// The one line a listing has for the child.
fn subagent_label(args: &SubagentArgs) -> String {
    match &args.title {
        Some(title) if !title.trim().is_empty() => format!("subagent: {title}"),
        _ => {
            let prompt: String = args.prompt.chars().take(80).collect();
            format!("subagent: {prompt}")
        }
    }
}

/// A child's outcome as the tool result the parent reads.
fn outcome_to_output(outcome: SubagentOutcome) -> ToolOutput {
    if outcome.is_error {
        ToolOutput::error(outcome.text)
    } else {
        ToolOutput::text(outcome.text)
    }
}

/// `spawn_agent` wrapper: it forwards `SpawnArgs` onto `SubagentTool`.
pub(crate) struct SpawnTool(pub(crate) SubagentTool);

impl SpawnTool {
    fn new(inner: SubagentTool) -> Self {
        Self(inner)
    }
}

#[async_trait]
impl FnExecutor<SpawnArgs, ToolOutput> for SpawnTool {
    async fn call(&self, _args: SpawnArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("spawn_agent needs a session"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: SpawnArgs,
    ) -> norte::Result<ToolOutput> {
        self.0
            .call_with_context(
                context,
                SubagentArgs {
                    prompt: args.task.clone(),
                    title: args.title.clone(),
                    detach: args.detach,
                    output_schema: args.output_schema.clone(),
                    lifetime: args.lifetime,
                    agent_id: args.agent_id.clone(),
                    context_mode: args.context_mode,
                },
            )
            .await
    }
}

/// The arguments of `spawn_agent` (RFC 8.2.5).
///
/// The supported spawn arguments. Unsupported RFC fields are rejected rather
/// than silently accepted with no effect.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SpawnArgs {
    /// The configured child agent, which must be allowed by the parent profile.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// The task for the child.
    pub task: String,
    /// The child session's title.
    pub title: Option<String>,
    /// Whether the call is let go of as a job.
    #[serde(default)]
    pub detach: bool,
    /// How long the child lives: `turn` (default) or `session` (RFC 8.2.2).
    ///
    /// `session` keeps the child session available after its first turn.
    #[serde(default)]
    pub lifetime: ChildLifetime,
    /// A JSON Schema the child's final output must validate against
    /// (RFC 8.2.6). Validated against the child's text as JSON when it
    /// parses, and against the text itself otherwise.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// `summary` adds a redacted, bounded excerpt of the parent history.
    #[serde(default)]
    pub context_mode: Option<ContextMode>,
}

/// What a child is doing, as the model reads it (RFC 8.2.7, `child_status`).
pub(super) fn child_state_label(
    phase: norte_code_wire::Phase,
    running: bool,
) -> norte_code_wire::ChildState {
    use norte_code_wire::Phase;
    match phase {
        Phase::Idle | _ if running => ChildState::Running,
        Phase::Streaming => ChildState::Running,
        Phase::AwaitingTool => ChildState::AwaitingApproval,
        Phase::Idle | _ => ChildState::Idle,
    }
}

/// The answer to `ask_delegator`, which carries the delegation answer.
fn answer_of(text: String) -> String {
    answer(&DelegatorAnswer {
        answered: true,
        answer: text,
        answered_by: "parent",
    })
}

/// Serializes one tool answer.
///
/// A closed struct cannot fail to serialize; a `Null` here would mean the
/// payload stopped matching its own type, which the compiler already rules out.
fn answer<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
}

pub(super) fn child_result(outcome: &SubagentOutcome) -> serde_json::Value {
    let summary: String = outcome.text.chars().take(2048).collect();
    let output = outcome
        .output_schema_valid
        .filter(|valid| *valid)
        .and_then(|_| serde_json::from_str(&outcome.text).ok());
    serde_json::to_value(ChildResult {
        status: if outcome.is_error {
            "failed"
        } else {
            "completed"
        },
        summary,
        summary_truncated: outcome.text.chars().count() > 2048,
        output_schema_valid: outcome.output_schema_valid,
        output,
    })
    .unwrap_or(serde_json::Value::Null)
}

mod args;
mod run;
mod state;
mod tools;
mod validation;

pub(super) use args::*;
pub(super) use state::{
    child_key, parent_key, phase_of, registered_child_state, run_session_message, service_of,
};
pub(super) use tools::{
    ApproveChildCall, AskDelegator, HandoffAgent, InspectAgent, ListAgents, MessageAgent,
    StopAgent, WaitAgent,
};
pub(super) use validation::{clip, validate_output_schema};

fn schedule_child_turn(
    service: Arc<Service>,
    service_slot: Arc<RwLock<Option<Arc<Service>>>>,
    children: SharedChildren,
    parent: SessionId,
    child: SessionKey,
    message: String,
) {
    let cancel = Arc::new(Cancellation::new());
    if let Some(parent_key) = parent_key(&service, parent) {
        children.set_cancel(&parent_key, &child, Arc::clone(&cancel));
    }
    emit_child_state(
        &service_slot,
        parent,
        &child,
        ChildState::Running,
        "state_change",
    );
    tokio::spawn(async move {
        let outcome = run_session_message(&service, &child, message, Some(&cancel)).await;
        emit_child_update(&children, &service_slot, parent, &outcome);
    });
}
