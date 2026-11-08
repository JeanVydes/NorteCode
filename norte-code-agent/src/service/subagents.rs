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
//! wraps the shell: what a job needs — a registration, an output that a reader
//! can find later, an end reported to a model that has no call left to answer
//! — is the host's business, and the engine knows nothing about it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use futures_util::StreamExt;
use norte::{CallId, Cancellation, FnExecutor, SessionId, ToolContext, ToolOutput};
use norte_code_protocol::{MessageRole, ReplayedKind, SessionKey};
use norte_code_protocol::{ServerEvent, ServerEventKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Service;
use super::delegator::SharedChildren;
use super::detach::DetachRegistry;
use super::jobs::JobRegistry;
use super::steering::{Steered, Steering};

/// How deep delegation may nest (RFC 8.2.4). A child at this depth cannot
/// spawn; the error names the limit, which is `BUDGET_EXCEEDED`-flavoured
/// in the RAEP tool error registry.
pub(super) const MAX_DELEGATION_DEPTH: u32 = 3;

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
    pub lifetime: String,
    /// The configured child agent to use, when one is requested explicitly.
    pub agent_id: Option<String>,
    /// Whether to include a redacted excerpt of the parent history.
    pub context_mode: Option<String>,
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
    pub(super) lifetime: String,
    pub(super) agent_id: Option<String>,
    pub(super) context_mode: Option<String>,
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
/// service's lock-guarded state — it asks through this trait.
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
        // One decrement, no loop: a lost race only ever under-counts, which
        // the next acquire retries against the value it sees.
        if self.running.load(Ordering::Acquire) > 0 {
            self.running.fetch_sub(1, Ordering::AcqRel);
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

    fn spawn_detached(&self, run: DetachedRun) -> ToolOutput {
        let DetachedRun {
            parent,
            call_id,
            tool_call_id,
            label,
            args,
            cancel,
            token,
        } = run;
        let id = match self
            .jobs
            .try_start(parent, call_id, &label, Arc::clone(&cancel))
        {
            Ok(id) => id,
            Err(error) => {
                self.gate.release_slot();
                self.detaches.release(parent, &token);
                return ToolOutput::error(error.to_string());
            }
        };
        let runner = Arc::clone(&self.runner);
        let jobs = Arc::clone(&self.jobs);
        let steering = Arc::clone(&self.steering);
        let children = Arc::clone(&self.children);
        let service_slot = Arc::clone(&self.service);
        let gate = Arc::clone(&self.gate);
        let request = SubagentRequest {
            prompt: args.prompt,
            parent,
            tool_call_id,
            output_schema: args.output_schema,
            lifetime: args.lifetime,
            agent_id: args.agent_id,
            context_mode: args.context_mode,
        };
        let job_id = id.clone();
        tokio::spawn(async move {
            let outcome = runner.run(request, Arc::clone(&cancel)).await;
            jobs.write_id(&job_id, &outcome.text);
            jobs.finish(&job_id, None);
            steering.push(
                parent,
                Steered::new(format!("subagent:{job_id}"), report(&job_id, &outcome)),
            );
            if let Some(service) = service_of(&service_slot) {
                service.schedule_auto_turn(parent);
            }
            emit_child_update(&children, &service_slot, parent, &outcome);
            gate.release_slot();
        });
        self.detaches.release(parent, &token);
        started(&id, &label)
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
            outcome.is_error,
            outcome.is_error,
        );
        let registered = children.get(&parent_key, child);
        let (state, reason) = match registered.as_ref() {
            Some(child) if child.is_failed() => ("FAILED", "turn_failed"),
            Some(child) if child.is_terminal() => ("STOPPED", "state_change"),
            Some(child) if !child.is_finished() => ("RUNNING", "turn_finished"),
            Some(_) => ("IDLE", "turn_finished"),
            None => ("FAILED", "turn_failed"),
        };
        if let Some(child) = registered.filter(super::delegator::Child::is_terminal) {
            persist_child_terminal(&service, &child.child, child.is_failed());
        }
        service.emit_delegation(ServerEvent::new(
            0,
            Some(parent_key),
            ServerEventKind::ChildSessionUpdated {
                child_session_id: child.as_str().to_owned(),
                state: state.to_owned(),
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
            super::sessions::ChildTerminalState::Failed
        } else {
            super::sessions::ChildTerminalState::Stopped
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
    state: &str,
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
                state: state.to_owned(),
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
/// coherent — a `tool_call` always has its `tool_result` — and the child
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

#[async_trait]
impl FnExecutor<SubagentArgs, ToolOutput> for SubagentTool {
    async fn call(&self, _args: SubagentArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error(
            "spawn_agent needs a client attached: a child session belongs to a parent one",
        ))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: SubagentArgs,
    ) -> norte::Result<ToolOutput> {
        let (Some(parent), Some(call_id)) = (context.session, context.call_id) else {
            return Ok(ToolOutput::error(
                "spawn_agent needs a client attached: a child session belongs to a parent one",
            ));
        };
        let tool_call_id = call_id.to_string();
        // The delegation depth is the parent's depth plus one (RFC 8.2.4):
        // past the limit the call is an error the model reads, not a child
        // that cannot do anything.
        if let Some(service) = service_of(&self.service)
            && let Some(depth) = parent_key(&service, parent).map(|key| service.depth_of(&key))
            && depth >= MAX_DELEGATION_DEPTH
        {
            return Ok(ToolOutput::error(format!(
                "BUDGET_EXCEEDED: maximum delegation depth ({MAX_DELEGATION_DEPTH}) reached at this session"
            )));
        }
        if let Some(error) = invalid_spawn_args(&args) {
            return Ok(ToolOutput::error(error));
        }
        if let Err(message) = self.gate.acquire() {
            return Ok(ToolOutput::error(message));
        }
        let label = subagent_label(&args);
        let cancel = Arc::new(Cancellation::new());
        // The detach token this call watches, published so `Ctrl-G` reaches
        // it, like any other call of the session.
        let token = Arc::new(Cancellation::new());
        self.detaches.attach(parent, Arc::clone(&token));

        if args.detach {
            return Ok(self.spawn_detached(DetachedRun {
                parent,
                call_id,
                tool_call_id,
                label,
                args,
                cancel,
                token,
            }));
        }

        // A waited call remains part of the parent turn and can be detached
        // by the user while it is running.
        let turn_cancelled = context.cancellation();
        let outcome = tokio::select! {
            outcome = self.runner.run(
                SubagentRequest {
                    prompt: args.prompt.clone(),
                    parent,
                    tool_call_id: tool_call_id.clone(),
                    output_schema: args.output_schema.clone(),
                    lifetime: args.lifetime.clone(),
                    agent_id: args.agent_id.clone(),
                    context_mode: args.context_mode.clone(),
                },
                Arc::clone(&cancel),
            ) => outcome,
            () = token.cancelled() => {
                cancel.cancel();
                SubagentOutcome {
                    text: String::new(),
                    is_error: false,
                    child: None,
                    output_schema_valid: None,
                }
            },
            () = turn_cancelled.cancelled() => {
                cancel.cancel();
                SubagentOutcome {
                    text: String::new(),
                    is_error: true,
                    child: None,
                    output_schema_valid: None,
                }
            },
        };
        self.detaches.release(parent, &token);
        self.gate.release_slot();

        self.emit_child_update(parent, &outcome);
        Ok(outcome_to_output(outcome))
    }
}

fn invalid_spawn_args(args: &SubagentArgs) -> Option<String> {
    if !matches!(args.lifetime.as_str(), "turn" | "session") {
        return Some("spawn_agent: lifetime must be `turn` or `session`".to_owned());
    }
    if args
        .context_mode
        .as_deref()
        .is_some_and(|mode| mode != "summary")
    {
        return Some("spawn_agent: context_mode must be `summary` when provided".to_owned());
    }
    None
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
pub(super) struct SpawnTool(pub super::SubagentTool);

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
                    lifetime: args.lifetime.clone(),
                    agent_id: args.agent_id.clone(),
                    context_mode: args.context_mode.clone(),
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
    #[serde(default = "default_lifetime")]
    pub lifetime: String,
    /// A JSON Schema the child's final output must validate against
    /// (RFC 8.2.6). Validated against the child's text as JSON when it
    /// parses, and against the text itself otherwise.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// `summary` adds a redacted, bounded excerpt of the parent history.
    #[serde(default)]
    pub context_mode: Option<String>,
}

fn default_lifetime() -> String {
    "turn".to_owned()
}

/// What a child is doing, as the model reads it (RFC 8.2.7, `child_status`).
pub(super) fn child_state_label(phase: norte_code_protocol::Phase, running: bool) -> &'static str {
    use norte_code_protocol::Phase;
    match phase {
        Phase::Idle if running => "RUNNING",
        Phase::Idle => "IDLE",
        Phase::Streaming => "RUNNING",
        Phase::AwaitingTool => "AWAITING_APPROVAL",
    }
}

pub(super) fn child_result(outcome: &SubagentOutcome) -> serde_json::Value {
    let summary: String = outcome.text.chars().take(2048).collect();
    let mut result = serde_json::json!({
        "status": if outcome.is_error { "failed" } else { "completed" },
        "summary": summary,
        "summary_truncated": outcome.text.chars().count() > 2048,
    });
    if let Some(valid) = outcome.output_schema_valid {
        result["output_schema_valid"] = serde_json::Value::Bool(valid);
        if valid && let Ok(output) = serde_json::from_str(&outcome.text) {
            result["output"] = output;
        }
    }
    result
}

/// Whether a JSON Schema validates `output` (RFC 8.2.6).
///
/// The schema is trusted input from the model, not from a file: a schema
/// that does not compile is not an invalid output, it is an invalid ask,
/// and the tool says so rather than marking the child failed. Validation
/// is a tiny subset — `type`, `required`, `properties`, `items`, `enum` —
/// enough for the schemas the registry carries, without a new dependency.
pub(super) fn validate_output_schema(schema: &serde_json::Value, output: &str) -> bool {
    let value = serde_json::from_str::<serde_json::Value>(output)
        .unwrap_or(serde_json::Value::String(output.to_owned()));
    schema_valid(schema, &value)
}

fn schema_valid(schema: &serde_json::Value, value: &serde_json::Value) -> bool {
    let Some(object) = schema.as_object() else {
        return true;
    };
    if let Some(kind) = object.get("type").and_then(serde_json::Value::as_str) {
        let ok = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.is_i64() || value.is_u64(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    if let Some(values) = object.get("enum").and_then(serde_json::Value::as_array)
        && !values.iter().any(|candidate| candidate == value)
    {
        return false;
    }
    if let Some(required) = object.get("required").and_then(serde_json::Value::as_array)
        && let Some(fields) = value.as_object()
    {
        for field in required {
            let Some(name) = field.as_str() else {
                continue;
            };
            if !fields.contains_key(name) {
                return false;
            }
        }
    }
    if let Some(properties) = object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        && let Some(fields) = value.as_object()
    {
        for (name, subschema) in properties {
            if let Some(field) = fields.get(name)
                && !schema_valid(subschema, field)
            {
                return false;
            }
        }
    }
    if let Some(items) = object.get("items")
        && let Some(list) = value.as_array()
    {
        for item in list {
            if !schema_valid(items, item) {
                return false;
            }
        }
    }
    true
}

/// Clips `text` to `max` words, for a summary the stream carries.
fn clip(text: &str, max: usize) -> String {
    let clipped: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        format!("{clipped}…")
    } else {
        clipped
    }
}

/// The arguments of `inspect_agent` (RFC 8.2.7).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InspectArgs {
    /// The child to inspect, by the id `spawn_agent` returned.
    pub child_session_id: String,
    /// What to look at: `status` (default), `transcript`, `events` or `jobs`.
    #[serde(default)]
    pub view: Option<String>,
    /// Only the events after this sequence.
    #[serde(default)]
    pub from_seq: Option<u64>,
    /// How many entries at most.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// The arguments of `message_agent` (RFC 8.2.8).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MessageArgs {
    /// The child to message.
    pub child_session_id: String,
    /// The message. A string, or an array of content parts (only text is
    /// carried here; a part that is not text is refused).
    pub content: serde_json::Value,
    /// How it is delivered: `steer` (default), `queue` or `interrupt`.
    #[serde(default)]
    pub delivery: Option<String>,
}

/// The arguments of `stop_agent` (RFC 8.2.7).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StopArgs {
    /// The child to stop.
    pub child_session_id: String,
    /// `graceful` (default) waits for its turn to end; `kill` cancels it.
    #[serde(default)]
    pub mode: Option<String>,
    /// Why it is being stopped.
    #[serde(default)]
    pub reason: Option<String>,
    /// Grace period before escalation to cancellation.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

/// The arguments of `wait_agent` (RFC 8.2.7).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WaitArgs {
    /// The child to wait for.
    pub child_session_id: String,
    /// `idle` (default) waits for its current turn to finish; `finished`
    /// waits for it to reach a terminal state.
    #[serde(default)]
    pub until: Option<String>,
    /// How long to wait, in seconds (default 300).
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

/// The arguments of `list_agents` (RFC 8.2.7).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ListArgs {
    /// Only the children in these states.
    #[serde(default)]
    pub state: Option<Vec<String>>,
    /// How many at most.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// The arguments of `approve_child_call` (RFC 8.2.9).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ApproveArgs {
    /// The child whose call is being approved.
    pub child_session_id: String,
    /// The tool call to approve or deny.
    pub tool_call_id: String,
    /// `approve` or `deny`.
    pub decision: String,
    /// Why.
    #[serde(default)]
    pub reason: Option<String>,
}

/// The arguments of `ask_delegator` (RFC 8.2.9).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AskDelegatorArgs {
    /// The question for the parent (or, when the parent is a human, for them).
    pub question: String,
    /// Closed choices, when the model offered any.
    #[serde(default)]
    pub options: Option<Vec<String>>,
    /// How long to wait, in seconds (default 60).
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

/// The arguments of `handoff_agent` (RFC 8.3).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct HandoffArgs {
    /// The agent the rest of the turn is handed to.
    pub agent_id: String,
    /// Why.
    #[serde(default)]
    pub reason: Option<String>,
    /// How much of the context the new agent carries: `full`, `summary` or
    /// `none` (default `summary`).
    #[serde(default)]
    pub carry_context: Option<String>,
}

// ---------------------------------------------------------------------------
// The delegating tools (RFC 8.2.7-8.2.9, 8.3)
//
// Each tool is a thin declaration over the shared services the parent and
// the child both live in: the child registry (who is whose child, and
// whether its delegator is suspended) and the service (to read the child's
// phase and to steer it). `ask_delegator` reuses the question broker, which
// is how the child already asks a human.
// ---------------------------------------------------------------------------

/// Resolves the parent's `SessionKey` from the `SessionId` a tool call runs
/// on, so a delegating tool can find its own children.
fn parent_key(service: &Service, parent: SessionId) -> Option<SessionKey> {
    let sessions = service.lock_sessions();
    sessions
        .iter()
        .find(|(_, record)| record.id == parent)
        .map(|(key, _)| key.clone())
}

/// The child `id` as a `SessionKey`, or an error naming the bad id.
fn child_key(id: &str) -> Result<SessionKey, String> {
    if id.trim().is_empty() {
        Err("child_session_id is required".to_owned())
    } else {
        Ok(SessionKey::new(id.to_owned()))
    }
}

/// The phase of a session, for `inspect_agent`'s `status` view.
fn phase_of(service: &Service, child: &SessionKey) -> Option<norte_code_protocol::Phase> {
    service
        .lock_sessions()
        .get(child)
        .map(|record| record.phase)
}

fn registered_child_state(
    child: &super::delegator::Child,
    phase: Option<norte_code_protocol::Phase>,
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
fn service_of(slot: &Arc<RwLock<Option<Arc<Service>>>>) -> Option<Arc<Service>> {
    slot.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub(super) async fn run_session_message(
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

/// `inspect_agent` (RFC 8.2.7): look at a child the parent spawned.
pub(super) struct InspectAgent {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl InspectAgent {
    /// An inspector over the shared child registry.
    pub(super) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<InspectArgs> {
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
                Ok(ToolOutput::text(
                    serde_json::json!({
                        "view": "status",
                        "child_session_id": child.as_str(),
                        "status": {
                            "state": state,
                            "title": title,
                            "delegator_suspended": registered.suspended,
                        },
                    })
                    .to_string(),
                ))
            }
            "transcript" => Ok(inspect_transcript(&service, &child, &args).await),
            "jobs" => Ok(inspect_jobs(&service, &child, &args)),
            "events" => Ok(ToolOutput::text(
                serde_json::json!({
                    "view": "events",
                    "child_session_id": child.as_str(),
                    "items": [],
                    "truncated": false,
                    "note": "the event view reads the RAEP replay window, which the service does not expose to the model yet",
                })
                .to_string(),
            )),
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
    ToolOutput::text(
        serde_json::json!({
            "view": "transcript",
            "child_session_id": child.as_str(),
            "items": items,
            "truncated": history.len() > from.saturating_add(items.len()),
        })
        .to_string(),
    )
}

fn inspect_jobs(service: &Service, child: &SessionKey, args: &InspectArgs) -> ToolOutput {
    let nid = match service.require_record(child) {
        Ok((nid, _, _)) => nid,
        Err(error) => return ToolOutput::error(format!("CHILD_NOT_FOUND: {error}")),
    };
    let jobs = service.jobs.list(Some(nid));
    let limit = args.limit.unwrap_or(20).clamp(1, 100);
    let items: Vec<serde_json::Value> = jobs
        .iter()
        .take(limit)
        .map(|job| {
            serde_json::json!({
                "job_id": job.id,
                "command": job.label,
                "status": job.state.label(),
            })
        })
        .collect();
    ToolOutput::text(
        serde_json::json!({
            "view": "jobs",
            "child_session_id": child.as_str(),
            "jobs": items,
        })
        .to_string(),
    )
}

/// `message_agent` (RFC 8.2.8): send a message to a child session.
///
/// `steer` (default) puts it in front of the request in flight; `queue`
/// makes it the child's next turn; `interrupt` cancels the child's turn
/// first. While the child's delegator is suspended the message is held,
/// and `held: true` says so.
pub(super) struct MessageAgent {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl MessageAgent {
    /// A messenger over the shared child registry.
    pub(super) fn new(
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
                Ok(held) => ToolOutput::text(
                    serde_json::json!({
                        "accepted": true,
                        "delivery": delivery,
                        "held": true,
                        "held_count": held,
                    })
                    .to_string(),
                ),
                Err(error) => ToolOutput::error(error.to_string()),
            };
        }
        let is_running = !registered.is_finished();
        if is_running && delivery == "steer" {
            if phase_of(&service, &child) == Some(norte_code_protocol::Phase::AwaitingTool) {
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
            return ToolOutput::text(
                serde_json::json!({
                    "accepted": true,
                    "delivery": "steer",
                    "child_state": "RUNNING",
                    "held": false,
                })
                .to_string(),
            );
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
        ToolOutput::text(
            serde_json::json!({
                "accepted": true,
                "delivery": delivery,
                "child_state": "RUNNING",
                "held": false,
            })
            .to_string(),
        )
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<MessageArgs> {
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
    emit_child_state(&service_slot, parent, &child, "RUNNING", "state_change");
    tokio::spawn(async move {
        let outcome = run_session_message(&service, &child, message, Some(&cancel)).await;
        emit_child_update(&children, &service_slot, parent, &outcome);
    });
}

/// `stop_agent` (RFC 8.2.7): stop a child session.
///
/// `graceful` (default) lets its turn end; `kill` cancels it. While the
/// child's delegator is suspended, stopping fails with
/// `DELEGATOR_SUSPENDED` (RFC 8.2.11): a human has taken the child over.
pub(super) struct StopAgent {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl StopAgent {
    /// A stopper over the shared child registry.
    pub(super) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<StopArgs> {
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
            return Ok(ToolOutput::text(
                serde_json::json!({
                    "child_session_id": child.as_str(),
                    "state": registered_child_state(
                        &registered,
                        phase_of(&service, &child),
                    ),
                })
                .to_string(),
            ));
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
            "STOPPED",
            args.reason.as_deref().unwrap_or("state_change"),
        );
        Ok(ToolOutput::text(
            serde_json::json!({
                "child_session_id": child.as_str(),
                "state": "STOPPED",
            })
            .to_string(),
        ))
    }
}

/// `wait_agent` (RFC 8.2.7): wait for a child to settle.
///
/// Waits for the child's turn-completion notification, or until `timeout_s`
/// (default 300). A child that already settled returns at once.
pub(super) struct WaitAgent {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl WaitAgent {
    /// A waiter over the shared child registry.
    pub(super) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<WaitArgs> {
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
        let until = args.until.as_deref().unwrap_or("idle");
        if !matches!(until, "idle" | "finished") {
            return Ok(ToolOutput::error(format!(
                "wait_agent: unknown until `{until}`; use idle or finished"
            )));
        }
        let timeout = std::time::Duration::from_secs(args.timeout_s.unwrap_or(300).min(600));
        let deadline = tokio::time::Instant::now() + timeout;
        let changed = Arc::clone(&child_record.changed);
        let mut timed_out = false;
        loop {
            let current = self.children.get(&parent_key, &child);
            let complete = current.is_some_and(|child| {
                if until == "idle" {
                    child.is_finished()
                } else {
                    child.is_terminal()
                }
            });
            if complete {
                break;
            }
            let notified = changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let current = self.children.get(&parent_key, &child);
            let complete = current.is_some_and(|child| {
                if until == "idle" {
                    child.is_finished()
                } else {
                    child.is_terminal()
                }
            });
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
        Ok(ToolOutput::text(
            serde_json::json!({
                "child_session_id": child.as_str(),
                "state": state,
                "timed_out": timed_out,
            })
            .to_string(),
        ))
    }
}

/// `list_agents` (RFC 8.2.7): the children a session has spawned.
pub(super) struct ListAgents {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl ListAgents {
    /// A lister over the shared child registry.
    pub(super) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<ListArgs> {
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
        let mut items: Vec<serde_json::Value> = Vec::new();
        for child in self.children.of(&parent_key) {
            let phase = phase_of(&service, &child.child);
            let state = registered_child_state(&child, phase);
            if let Some(filter) = &args.state
                && !filter.iter().any(|wanted| wanted == state)
            {
                continue;
            }
            items.push(serde_json::json!({
                "child_session_id": child.child.as_str(),
                "state": state,
                "tool_call_id": child.tool_call_id,
            }));
            if items.len() >= limit {
                break;
            }
        }
        Ok(ToolOutput::text(
            serde_json::json!({ "children": items }).to_string(),
        ))
    }
}

/// `approve_child_call` (RFC 8.2.9): approve or deny a tool call a child made.
///
/// Approvals in this host are rules, not interactive prompts (REQ 13.6 is out
/// of scope here): a child that needs an approval is refused by the rule
/// interceptor before the call runs, so there is nothing pending to approve.
/// The tool exists so the model can record a decision on the stream; it
/// answers `APPROVAL_NOT_PENDING` for any call id.
pub(super) struct ApproveChildCall {
    children: SharedChildren,
    questions: Arc<norte_code_protocol::tools::QuestionBroker>,
}

impl ApproveChildCall {
    /// An approver over the shared child registry and the question broker.
    pub(super) fn new(
        children: SharedChildren,
        questions: Arc<norte_code_protocol::tools::QuestionBroker>,
    ) -> Self {
        Self {
            children,
            questions,
        }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<ApproveArgs> {
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
pub(super) struct AskDelegator {
    questions: Arc<norte_code_protocol::tools::QuestionBroker>,
}

impl AskDelegator {
    /// A delegator asker over the question broker.
    pub(super) fn new(questions: Arc<norte_code_protocol::tools::QuestionBroker>) -> Self {
        Self { questions }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<AskDelegatorArgs> {
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
            Ok(answer) => Ok(ToolOutput::text(
                serde_json::json!({
                    "answered": true,
                    "answer": answer,
                    "answered_by": "parent",
                })
                .to_string(),
            )),
            Err(error) => Ok(ToolOutput::error(format!("DELEGATOR_TIMEOUT: {error}"))),
        }
    }
}

/// `handoff_agent` (RFC 8.3): hand the rest of the turn to another agent.
///
/// The handoff takes effect at the end of the turn (`takes_effect:
/// turn_end`). This host runs one configured agent, so a handoff to any
/// other name is refused with `HANDOFF_NOT_ALLOWED`.
pub(super) struct HandoffAgent {
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl HandoffAgent {
    /// A handoff over the service.
    pub(super) fn new(service: Arc<RwLock<Option<Arc<Service>>>>) -> Self {
        Self { service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<HandoffArgs> {
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
            return Ok(ToolOutput::text(
                serde_json::json!({
                    "accepted": true,
                    "takes_effect": "turn_end",
                })
                .to_string(),
            ));
        }
        let _ = &args.carry_context;
        Ok(ToolOutput::error(format!(
            "HANDOFF_NOT_ALLOWED: this host runs one agent ({current}); `{}` is not available",
            args.agent_id
        )))
    }
}
