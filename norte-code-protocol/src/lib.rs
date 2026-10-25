//! The Norte Code client protocol.
//!
//! Versioned serde types only. Nothing here depends on Norte: the service maps
//! between these types and the engine, so a client does not compile the engine
//! and the wire can move on its own schedule.
//!
//! The two halves of the wire are [`ClientRequest`] and [`ServerEvent`]. Every
//! server event carries the request it answers and, when it is about one, the
//! session it belongs to.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol revision. It is bumped when a type changes shape in a way an
/// older client cannot read.
pub const PROTOCOL_VERSION: u32 = 1;

/// A stable identifier of a logical Norte Code session.
///
/// It is Norte Code's own id, not the engine's: a compaction replaces the
/// underlying Norte session while the logical id stays the same.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionKey(String);

impl SessionKey {
    /// Wraps `id`.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A client request: a numbered command for the service.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientRequest {
    /// Chosen by the client and echoed on every event that answers it.
    pub request_id: u64,
    /// What to do.
    pub kind: ClientRequestKind,
}

/// The command itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientRequestKind {
    /// Create a session and answer with [`ServerEventKind::SessionCreated`].
    CreateSession {
        /// A human label for the session.
        title: Option<String>,
    },
    /// List the known sessions.
    ListSessions,
    /// Make a session the caller's current one.
    SwitchSession {
        /// The session.
        session: SessionKey,
    },
    /// Copy a session's history into a new session.
    ForkSession {
        /// The session to copy.
        session: SessionKey,
        /// A label for the copy.
        title: Option<String>,
    },
    /// Rename a session.
    RenameSession {
        /// The session.
        session: SessionKey,
        /// The new label.
        title: String,
    },
    /// Delete a session.
    DeleteSession {
        /// The session.
        session: SessionKey,
    },
    /// Start a turn. With no session, one is created first.
    SendMessage {
        /// The session, or `None` to create one.
        session: Option<SessionKey>,
        /// The user message.
        text: String,
        /// A budget for this message only.
        budget: Option<Budget>,
        /// A reasoning override for this message only.
        reasoning: Option<Reasoning>,
        /// Whether the client is attaching to a session that already has a
        /// history, and wants it replayed before the answer.
        ///
        /// A client that starts blank cannot tell what the session holds, so it
        /// asks for the transcript. The service used to infer this from the
        /// request having no message, which is true for a resume and false for
        /// `run --session KEY`: a message sent to an existing session got no
        /// transcript at all.
        #[serde(default)]
        replay: bool,
    },
    /// Queue a message to apply at the next turn boundary.
    Steer {
        /// The session.
        session: SessionKey,
        /// The message.
        text: String,
        /// A name for the message, so the client can tell it apart when the
        /// engine reports it applied. An empty name is fine: the service names
        /// it then.
        #[serde(default)]
        id: String,
    },
    /// Ask the running turn to stop.
    CancelTurn {
        /// The session.
        session: SessionKey,
    },
    /// Continue a session that is parked or interrupted.
    Resume {
        /// The session.
        session: SessionKey,
    },
    /// Approve a parked tool call.
    Approve {
        /// The session.
        session: SessionKey,
        /// The approval record.
        approval: String,
        /// Replacement arguments, when the human edited the call.
        arguments: Option<Value>,
        /// How long the grant lasts.
        scope: Option<GrantScope>,
        /// Whether to remember the decision for the session.
        remember: Option<bool>,
    },
    /// Deny a parked tool call.
    Deny {
        /// The session.
        session: SessionKey,
        /// The approval record.
        approval: String,
        /// Why.
        reason: Option<String>,
        /// How long the denial lasts.
        scope: Option<DenyScope>,
    },
    /// Revoke a live grant.
    Revoke {
        /// The session.
        session: SessionKey,
        /// The approval record.
        approval: String,
        /// The grant to revoke.
        grant: String,
    },
    /// List the approval grants held by a session.
    ListGrants {
        /// The session.
        session: SessionKey,
    },
    /// Answer a question a running turn is waiting on.
    AnswerQuestion {
        /// The session.
        session: SessionKey,
        /// The question id.
        question: String,
        /// The human's answer.
        answer: String,
    },
    /// Switch the session's model at a turn boundary.
    SetModel {
        /// The session.
        session: SessionKey,
        /// A `provider/model` reference.
        model: String,
    },
    /// Set the reasoning setting for a scope.
    SetReasoning {
        /// The session.
        session: SessionKey,
        /// Whether the setting applies to the session or one message.
        scope: ReasoningScope,
        /// The setting.
        reasoning: Reasoning,
    },
    /// Set a session budget.
    SetBudget {
        /// The session.
        session: SessionKey,
        /// The budget.
        budget: Budget,
    },
    /// Compact the session now.
    Compact {
        /// The session.
        session: SessionKey,
    },
    /// List the models the catalogue knows.
    ListModels,
    /// Refresh the catalogue from the network.
    RefreshCatalog,
    /// List the installed Agent Skills.
    ListSkills,
    /// Re-scan the skills roots.
    RefreshSkills,
    /// Report the ledger's totals.
    CostReport {
        /// One session, or every session.
        session: Option<SessionKey>,
    },
}

/// A service event: a numbered answer to a [`ClientRequest`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerEvent {
    /// The request this event answers.
    pub request_id: u64,
    /// The session it is about, when it is about one.
    pub session: Option<SessionKey>,
    /// What happened.
    pub kind: ServerEventKind,
}

/// The event itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ServerEventKind {
    /// A session now exists.
    SessionCreated {
        /// Its label.
        title: Option<String>,
    },
    /// The known sessions.
    SessionList {
        /// One entry per session.
        sessions: Vec<SessionInfo>,
    },
    /// A session was deleted.
    SessionDeleted,
    /// A session was renamed.
    SessionRenamed {
        /// The new label.
        title: String,
    },
    /// A session was copied.
    SessionForked {
        /// The session the copy was made from.
        from: SessionKey,
    },
    /// The caller's current session changed.
    SessionSwitched,
    /// A turn started.
    TurnStarted {
        /// The turn number.
        turn: u64,
    },
    /// A stream delta.
    Delta {
        /// The turn.
        turn: u64,
        /// The delta.
        delta: Delta,
    },
    /// A tool call started.
    ToolStarted {
        /// The turn.
        turn: u64,
        /// The call.
        call_id: String,
        /// The tool.
        tool: String,
        /// The arguments the call runs with, when they can be recovered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments: Option<serde_json::Value>,
        /// The file's contents before an `edit`, when the host captured them.
        ///
        /// The host snapshots the target immediately before the executor runs,
        /// so a preview can show the original lines even when the tool has
        /// already written the file by the time the client renders the event.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    /// A tool call finished.
    ToolFinished {
        /// The turn.
        turn: u64,
        /// The call.
        call_id: String,
        /// The tool.
        tool: String,
        /// Whether the tool reported an error.
        is_error: bool,
        /// The error text the model saw, when the tool failed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// The text the tool returned, when it succeeded and the host kept it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        /// Whether `output` was shortened for the wire.
        #[serde(default)]
        truncated: bool,
        /// How long it ran, in milliseconds.
        duration_ms: u64,
    },
    /// A tool call is parked for a human decision.
    ApprovalRequested {
        /// The turn.
        turn: u64,
        /// The approval record.
        approval: String,
        /// The call awaiting a decision.
        call_id: String,
        /// The tool.
        tool: String,
        /// The arguments the tool would run with, when they can be recovered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments: Option<serde_json::Value>,
        /// The OS sandbox that will wrap the call, when one will. The arguments
        /// still describe the command the model asked for, not the sandbox
        /// launcher.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sandbox: Option<String>,
    },
    /// A parked call was approved or denied.
    ApprovalResolved {
        /// The approval record.
        approval: String,
        /// Whether it was approved.
        approved: bool,
    },
    /// A model request was reconstructed: its usage and reported cost.
    RequestReconstructed {
        /// The turn.
        turn: u64,
        /// The request's index within the turn.
        index: u64,
        /// The tokens the provider reported.
        usage: Usage,
        /// The cost the provider reported, in USD.
        cost_usd: Option<f64>,
        /// Whether the answer's text was truncated by the output budget.
        truncated: bool,
        /// How long the request took, including streaming.
        #[serde(default)]
        duration_ms: u64,
        /// Time to the first answer token, when one arrived.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_token_ms: Option<u64>,
        /// Answer tokens per second of generation, when both are known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens_per_second: Option<f64>,
        /// Whether the provider served any prompt token from cache.
        #[serde(default)]
        cache_hit: bool,
        /// Tools the model called while producing this message, in order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools: Vec<String>,
    },
    /// A commit landed.
    Committed {
        /// The turn.
        turn: u64,
        /// The session version the commit reached.
        version: u64,
    },
    /// One past message of a session, sent when it is resumed so the client can
    /// show where the conversation was. Only the most recent messages are sent,
    /// and never across a compaction boundary.
    MessageReplayed {
        /// A monotonic index in the replay, starting at 1.
        index: u64,
        /// Who spoke: `user`, `assistant`, `tool`, `system` or `developer`.
        role: String,
        /// The message's text, with each part on its own line.
        text: String,
    },
    /// Compaction started.
    CompactionStarted {
        /// Why it started: `manual` or `threshold`.
        #[serde(default)]
        reason: String,
    },
    /// Compaction finished, possibly with a new Norte session behind the same
    /// logical one.
    CompactionFinished {
        /// Whether compaction happened.
        compacted: bool,
        /// The new Norte session id, when one was created.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        north_session: Option<String>,
        /// Why it failed, when it did. History is unchanged in that case.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A budget crossed its warning threshold.
    BudgetWarning {
        /// What was measured.
        scope: BudgetScope,
        /// The unit the figures are in.
        unit: BudgetUnit,
        /// What was spent.
        spent: f64,
        /// The limit.
        limit: f64,
    },
    /// A budget was exhausted.
    BudgetExceeded {
        /// What was measured.
        scope: BudgetScope,
        /// The unit the figures are in.
        unit: BudgetUnit,
        /// What was spent.
        spent: f64,
        /// The limit.
        limit: f64,
        /// What the service did about it.
        action: BudgetAction,
    },
    /// The session's budget changed.
    BudgetChanged {
        /// The budget now in force, or `None` when it was cleared.
        budget: Option<Budget>,
    },
    /// The session's accumulated usage, sent when a client attaches.
    ///
    /// A resume starts a fresh client, so its counters begin at zero; without
    /// this it would report the totals of the messages it happens to see rather
    /// than the ones the session already spent. Sent once, in the prefix of a
    /// resume, and never for an ordinary turn.
    SessionStats {
        /// Input tokens summed over the session's reported requests.
        input_tokens: u64,
        /// Output tokens summed over the session's reported requests.
        output_tokens: u64,
        /// Prompt tokens the provider served from its cache, summed.
        #[serde(default)]
        cache_read_tokens: u64,
        /// Prompt tokens the provider wrote to its cache, summed.
        #[serde(default)]
        cache_write_tokens: u64,
        /// Provider-reported cost, summed, in USD.
        cost_usd: f64,
        /// Model requests reported for usage.
        requests: u64,
        /// How much of the context window the current history occupies.
        context_used: u64,
        /// The context window, when the model declares one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    /// The running cost total changed.
    CostUpdated {
        /// The total the provider reported, in USD.
        total_usd: f64,
        /// How many requests are in the total.
        requests: u64,
    },
    /// A response carried no cost.
    CostNotReported {
        /// How many requests so far carried no cost.
        requests: u64,
    },
    /// The host ended the turn on purpose.
    TurnStopped {
        /// Why the turn ended.
        reason: String,
    },
    /// The engine applied a steering message to the request already in flight.
    SteerApplied {
        /// The names of the messages that reached the request.
        ids: Vec<String>,
    },
    /// The session's model changed.
    ModelChanged {
        /// The new `provider/model`.
        model: String,
    },
    /// A model switch was accepted while the session was busy and will be
    /// applied at the next turn boundary.
    ModelQueued {
        /// The requested `provider/model`.
        model: String,
    },
    /// The session's reasoning setting was lowered to fit the new model.
    ReasoningDowngraded {
        /// What was asked for.
        from: String,
        /// What was applied.
        to: String,
        /// Why.
        reason: String,
    },
    /// The catalogue changed on disk.
    CatalogUpdated {
        /// How many models it carries.
        models: usize,
    },
    /// The models the catalogue knows.
    ModelList {
        /// One entry per model.
        models: Vec<ModelInfo>,
    },
    /// The installed Agent Skills.
    SkillList {
        /// One entry per valid skill.
        skills: Vec<SkillInfo>,
        /// Skills that were skipped, with the reason.
        warnings: Vec<String>,
    },
    /// The skills roots were re-scanned.
    SkillsUpdated {
        /// How many valid skills were found.
        skills: usize,
    },
    /// An approval grant was revoked.
    GrantRevoked {
        /// The approval record.
        approval: String,
        /// The grant.
        grant: String,
    },
    /// The approval grants held by a session.
    GrantList {
        /// One entry per grant, including revoked ones.
        grants: Vec<GrantInfo>,
    },
    /// The running turn needs an answer before it can continue.
    QuestionAsked {
        /// The turn.
        turn: u64,
        /// The question id to answer.
        question: String,
        /// What the model wants to know.
        prompt: String,
        /// Closed choices, when the model offered any.
        ///
        /// A client should append its own “answer something else” entry and
        /// send the free-form text through [`ClientRequestKind::AnswerQuestion`].
        #[serde(default)]
        options: Vec<String>,
        /// Whether more than one option may be chosen.
        #[serde(default)]
        multiple: bool,
    },
    /// A question was answered and the turn resumed.
    QuestionAnswered {
        /// The question id.
        question: String,
    },
    /// The turn ended.
    TurnFinished {
        /// The turn.
        turn: u64,
        /// The session phase it ended in.
        phase: Phase,
        /// Whether the turn committed an assistant message.
        committed: bool,
    },
    /// A transient request failed and the host will retry it.
    ///
    /// The retry resumes the same session, so the user's message is not sent
    /// again. No `TurnFinished` follows until the retry succeeds or the host
    /// gives up.
    RetryScheduled {
        /// The turn.
        turn: u64,
        /// The retry that will run, starting at 1.
        attempt: u32,
        /// How long the host will wait before it retries.
        delay_ms: u64,
        /// Why the request failed, written for the person reading it.
        reason: String,
    },
    /// The wait before a retry is over and the request is running again.
    RetryStarted {
        /// The turn.
        turn: u64,
        /// The retry that is starting, counting from one.
        attempt: u32,
    },
    /// A request failed.
    Error {
        /// A machine-readable category.
        code: ErrorCode,
        /// A message written for the person reading it.
        message: String,
    },
}

/// A session as the client sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInfo {
    /// The logical id.
    pub session: SessionKey,
    /// The label.
    pub title: Option<String>,
    /// The model in use, as `provider/model`.
    pub model: Option<String>,
    /// When it was created, as RFC 3339.
    pub created_at: Option<String>,
    /// Where it currently is.
    pub phase: Phase,
}

/// One approval grant, as a client sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantInfo {
    /// The approval record that issued it.
    pub approval: String,
    /// The grant id to pass to [`ClientRequestKind::Revoke`].
    pub grant: String,
    /// The tool the approval covered.
    pub tool: String,
    /// How far the grant reaches.
    pub scope: GrantScope,
    /// When it was issued, as RFC 3339.
    pub issued_at: String,
    /// Whether it has already been revoked.
    pub revoked: bool,
}

/// One model the catalogue knows, flattened for a picker.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInfo {
    /// The provider id.
    pub provider: String,
    /// The model id the provider expects.
    pub model: String,
    /// The display name.
    pub name: String,
    /// A one-line description.
    pub description: Option<String>,
    /// The context window, when known and plausible.
    pub max_context: Option<u64>,
    /// The output limit, when known and plausible.
    pub max_output: Option<u64>,
    /// Whether the model calls host functions.
    pub tools: bool,
    /// Accepted input modalities.
    pub input: Vec<String>,
    /// Produced output modalities.
    pub output: Vec<String>,
    /// The reasoning controls the host may offer.
    pub reasoning: ReasoningInfo,
    /// Whether the catalogue marks the model deprecated.
    pub deprecated: bool,
}

/// One installed Agent Skill, as a client sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillInfo {
    /// The validated name, equal to its directory name.
    pub name: String,
    /// What the skill does and when to use it.
    pub description: String,
    /// The skill directory.
    pub root: String,
    /// Tools the skill declares as pre-approved, when the host honors them.
    pub allowed_tools: Vec<String>,
}

/// The reasoning controls a model declares.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningInfo {
    /// Whether the model reasons at all.
    pub supported: bool,
    /// Whether reasoning may be turned off.
    pub can_disable: bool,
    /// The effort names the host may offer, in Norte's spelling.
    pub efforts: Vec<String>,
    /// Whether a token budget is accepted.
    pub supports_budget: bool,
    /// The largest accepted budget.
    pub max_budget: Option<u32>,
}

/// Where a session currently is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Nothing in flight.
    Idle,
    /// A turn is being produced.
    Streaming,
    /// A tool call is authorised and about to run.
    AwaitingTool,
    /// A tool call is parked for a human.
    AwaitingApproval,
}

/// A stream delta.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "delta", rename_all = "snake_case")]
pub enum Delta {
    /// Answer text.
    Text {
        /// The part index.
        index: u32,
        /// The text.
        text: String,
    },
    /// Reasoning text.
    Reasoning {
        /// The part index.
        index: u32,
        /// The text.
        text: String,
    },
    /// Refusal text.
    Refusal {
        /// The part index.
        index: u32,
        /// The text.
        text: String,
    },
    /// A fragment of a tool call's arguments.
    ToolArguments {
        /// The part index.
        index: u32,
        /// The call.
        call_id: String,
        /// The fragment.
        arguments: String,
    },
    /// Generated media, as a byte count.
    Media {
        /// The part index.
        index: u32,
        /// How many bytes.
        bytes: u64,
        /// Whether the part ends here.
        last: bool,
    },
}

/// Provider-reported token counters and cost.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    /// Input tokens.
    pub input_tokens: Option<u64>,
    /// Output tokens.
    pub output_tokens: Option<u64>,
    /// Reasoning tokens, counted inside the output.
    pub reasoning_tokens: Option<u64>,
    /// Tokens read from the provider's cache.
    pub cache_read_tokens: Option<u64>,
    /// Tokens written to the provider's cache.
    pub cache_write_tokens: Option<u64>,
    /// The cost the provider reported, in USD, when it reported one.
    pub cost_usd: Option<f64>,
    /// Provider-specific counters, kept verbatim.
    pub vendor: BTreeMap<String, Value>,
}

/// What a budget limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetUnit {
    /// US dollars, provider-reported.
    Usd,
    /// Tokens, provider-reported.
    Tokens,
    /// Model requests.
    Requests,
}

/// What to do when a budget is exhausted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetAction {
    /// Report it and keep going.
    Warn,
    /// Park for approval.
    Ask,
    /// Stop the turn.
    Stop,
}

/// A spending limit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// What it measures.
    pub unit: BudgetUnit,
    /// The limit.
    pub limit: f64,
    /// What to do at the limit.
    pub action: BudgetAction,
}

/// What a budget applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    /// One message.
    Message,
    /// One session.
    Session,
    /// The day, across sessions.
    GlobalDaily,
}

/// How long an approval grant lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantScope {
    /// This call only.
    Once,
    /// This tool for the rest of this session.
    Tool,
    /// Every tool in this session.
    Session,
    /// Every session.
    Always,
}

/// How long a denial lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyScope {
    /// This call only.
    Once,
    /// This tool for the rest of this session.
    ToolSession,
    /// This tool in every session, stored in the user config.
    ToolGlobal,
}

/// A reasoning setting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reasoning", rename_all = "snake_case")]
pub enum Reasoning {
    /// Let the model decide.
    Automatic,
    /// Turn reasoning off, when the model allows it.
    Disabled,
    /// A named effort.
    Effort {
        /// The effort name.
        effort: String,
    },
    /// A token budget for reasoning.
    Budget {
        /// The budget.
        tokens: u32,
    },
}

/// Whether a reasoning setting applies to a session or one message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningScope {
    /// The session default.
    Session,
    /// The next message only.
    Message,
}

/// A machine-readable error category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request was malformed.
    InvalidRequest,
    /// The named session, model or approval does not exist.
    NotFound,
    /// The session moved and the action could not be applied.
    Conflict,
    /// The model cannot do what was asked.
    Capability,
    /// A budget stopped the action.
    Budget,
    /// The provider failed.
    Provider,
    /// The service failed internally.
    Internal,
    /// The feature is not implemented yet.
    Unsupported,
}

impl ServerEvent {
    /// Builds an event answering `request_id`, about `session`.
    #[must_use]
    pub fn new(request_id: u64, session: Option<SessionKey>, kind: ServerEventKind) -> Self {
        Self {
            request_id,
            session,
            kind,
        }
    }

    /// A failure event.
    #[must_use]
    pub fn error(
        request_id: u64,
        session: Option<SessionKey>,
        code: ErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            request_id,
            session,
            ServerEventKind::Error {
                code,
                message: message.into(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips() {
        let request = ClientRequest {
            request_id: 3,
            kind: ClientRequestKind::SendMessage {
                session: Some(SessionKey::new("abc")),
                text: "hi".to_owned(),
                budget: Some(Budget {
                    unit: BudgetUnit::Usd,
                    limit: 1.0,
                    action: BudgetAction::Stop,
                }),
                reasoning: Some(Reasoning::Effort {
                    effort: "high".to_owned(),
                }),
                replay: false,
            },
        };
        let json = serde_json::to_string(&request).expect("serialize");
        let back: ClientRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.request_id, 3);
    }

    #[test]
    fn an_event_round_trips() {
        let event = ServerEvent::new(
            1,
            Some(SessionKey::new("s")),
            ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Text {
                    index: 0,
                    text: "hi".to_owned(),
                },
            },
        );
        let json = serde_json::to_string(&event).expect("serialize");
        let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.session.expect("session").as_str(), "s");
    }

    #[test]
    fn a_steer_applied_event_round_trips() {
        let event = ServerEvent::new(
            1,
            Some(SessionKey::new("s")),
            ServerEventKind::SteerApplied {
                ids: vec!["one".to_owned()],
            },
        );
        let json = serde_json::to_string(&event).expect("serialize");
        let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            back.kind,
            ServerEventKind::SteerApplied { ids } if ids == ["one"]
        ));
    }

    #[test]
    fn a_retry_event_round_trips() {
        let event = ServerEvent::new(
            1,
            Some(SessionKey::new("s")),
            ServerEventKind::RetryScheduled {
                turn: 3,
                attempt: 2,
                delay_ms: 1_500,
                reason: "rate limited".to_owned(),
            },
        );
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"event\":\"retry_scheduled\""));
        let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
        match back.kind {
            ServerEventKind::RetryScheduled {
                turn,
                attempt,
                delay_ms,
                reason,
            } => {
                assert_eq!((turn, attempt, delay_ms), (3, 2, 1_500));
                assert_eq!(reason, "rate limited");
            }
            other => panic!("expected a retry, got {other:?}"),
        }
    }

    #[test]
    fn a_retry_start_round_trips() {
        let event = ServerEvent::new(
            1,
            Some(SessionKey::new("s")),
            ServerEventKind::RetryStarted {
                turn: 3,
                attempt: 2,
            },
        );
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"event\":\"retry_started\""));
        let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            back.kind,
            ServerEventKind::RetryStarted {
                turn: 3,
                attempt: 2
            }
        ));
    }

    #[test]
    fn an_approval_without_arguments_still_deserializes() {
        let json = r#"{
            "request_id": 1,
            "session": "s",
            "kind": {
                "event": "approval_requested",
                "turn": 1,
                "approval": "a",
                "call_id": "c",
                "tool": "write"
            }
        }"#;
        let event: ServerEvent = serde_json::from_str(json).expect("deserialize");
        match event.kind {
            ServerEventKind::ApprovalRequested { arguments, .. } => {
                assert!(arguments.is_none());
            }
            other => panic!("expected an approval, got {other:?}"),
        }
    }
}
