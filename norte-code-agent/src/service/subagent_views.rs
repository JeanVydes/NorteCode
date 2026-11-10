//! What the delegation tools answer the model with.
//!
//! Each tool returns one of these, serialized. They are structs rather than
//! built JSON so a field cannot be renamed in one arm and missed in another,
//! and so the values the model reads have a shape the compiler checks.

use serde::Serialize;

/// The payload of `inspect_agent`.
#[derive(Serialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub(super) enum InspectView<'a> {
    /// Where the child stands.
    Status {
        /// The child's id.
        child_session_id: &'a str,
        /// Its state.
        status: ChildStatus<'a>,
    },
    /// The child's recent messages.
    Transcript {
        /// The child's id.
        child_session_id: &'a str,
        /// One line per message.
        items: Vec<String>,
        /// Whether older messages were left out.
        truncated: bool,
    },
    /// The child's background jobs.
    Jobs {
        /// The child's id.
        child_session_id: &'a str,
        /// One entry per job.
        jobs: Vec<JobView<'a>>,
    },
    /// The child's event stream.
    Events {
        /// The child's id.
        child_session_id: &'a str,
        /// The events, empty until the service exposes its replay window.
        items: Vec<()>,
        /// Whether any were left out.
        truncated: bool,
        /// Why the view is empty.
        note: &'a str,
    },
}

/// Where one child session stands.
#[derive(Serialize)]
pub(super) struct ChildStatus<'a> {
    /// Its phase, as the wire spells it.
    pub(super) state: &'a str,
    /// Its title.
    pub(super) title: String,
    /// Whether its delegator is suspended.
    pub(super) delegator_suspended: bool,
}

/// One child job, as a listing shows it.
#[derive(Serialize)]
pub(super) struct JobView<'a> {
    /// The job's id.
    pub(super) job_id: &'a str,
    /// The command or label.
    pub(super) command: &'a str,
    /// Where it stands.
    pub(super) status: &'a str,
}

/// The answer to `message_agent` when the message was held.
#[derive(Serialize)]
pub(super) struct HeldMessage<'a> {
    /// Whether the message was taken.
    pub(super) accepted: bool,
    /// How it will be delivered.
    pub(super) delivery: &'a str,
    /// Whether the delegator is holding it.
    pub(super) held: bool,
    /// How many messages the delegator holds.
    pub(super) held_count: usize,
}

/// The answer to `message_agent` when the message was taken.
#[derive(Serialize)]
pub(super) struct AcceptedMessage<'a> {
    /// Whether the message was taken.
    pub(super) accepted: bool,
    /// How it will be delivered.
    pub(super) delivery: &'a str,
    /// What the child is doing now.
    pub(super) child_state: &'a str,
    /// Whether the delegator is holding it.
    pub(super) held: bool,
}

/// The state of one child, as `stop_agent` reports it.
#[derive(Serialize)]
pub(super) struct ChildStateView<'a> {
    /// The child's id.
    pub(super) child_session_id: &'a str,
    /// Where it stands.
    pub(super) state: &'a str,
}

/// The answer to `wait_agent`.
#[derive(Serialize)]
pub(super) struct WaitView<'a> {
    /// The child's id.
    pub(super) child_session_id: &'a str,
    /// Where it stands now.
    pub(super) state: &'a str,
    /// Whether the wait gave up before the child changed.
    pub(super) timed_out: bool,
}

/// The result of one delegated run.
#[derive(Serialize)]
pub(super) struct ChildResult<'a> {
    /// `completed` or `failed`.
    pub(super) status: &'a str,
    /// What the child answered, clipped.
    pub(super) summary: String,
    /// Whether the summary was clipped.
    pub(super) summary_truncated: bool,
    /// Whether the answer validated against the requested schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) output_schema_valid: Option<bool>,
    /// The answer parsed as JSON, when the schema asked for structured output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) output: Option<serde_json::Value>,
}

/// One child, as `list_agents` names it.
#[derive(Serialize)]
pub(super) struct ListedChild {
    /// The child's id.
    pub(super) child_session_id: String,
    /// Where it stands.
    pub(super) state: String,
    /// The call that spawned it.
    pub(super) tool_call_id: String,
}

/// The answer to `list_agents`.
#[derive(Serialize)]
pub(super) struct ListedChildren {
    /// One entry per child.
    pub(super) children: Vec<ListedChild>,
}

/// The answer to `ask_delegator`.
#[derive(Serialize)]
pub(super) struct DelegatorAnswer {
    /// Whether a person answered.
    pub(super) answered: bool,
    /// What they answered.
    pub(super) answer: String,
    /// Who answered.
    pub(super) answered_by: &'static str,
}

/// The answer to `handoff_agent`.
#[derive(Serialize)]
pub(super) struct HandoffAccepted {
    /// Whether the handoff was taken.
    pub(super) accepted: bool,
    /// When it takes effect.
    pub(super) takes_effect: &'static str,
}
