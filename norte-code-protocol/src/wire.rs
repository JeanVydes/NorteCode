//! The Norte Code client wire: requests and events.
//!
//! Re-exported from the crate root.

#![forbid(unsafe_code)]
#![allow(clippy::wildcard_imports)]

use serde::{Deserialize, Serialize};

use crate::types::*;

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
        /// Media to send beside the text, in the order it should appear.
        ///
        /// The bytes are the host's to store: a client puts them in the blob
        /// store it shares with the service and names the blob here, which is
        /// what the engine does with media the vendor returns. That keeps a
        /// pasted photo out of the wire *and* out of the session: what is
        /// stored is one blob, named from both sides.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
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
    /// Stop a session's background jobs.
    StopJobs {
        /// The session.
        session: SessionKey,
        /// One job by id, or every job of the session when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Stop waiting for the running call and leave it running.
    ///
    /// The turn keeps its process and gives up its hold on it: the call answers
    /// as a background job does, and its output arrives as a message.
    DetachTurn {
        /// The session.
        session: SessionKey,
    },
    /// List the background jobs of a session.
    ListJobs {
        /// The session.
        session: SessionKey,
    },
    /// Continue a session that is parked or interrupted.
    Resume {
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
        /// The provider that serves it.
        provider: String,
        /// The model id.
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
    /// Re-read the catalogue files.
    ///
    /// It reads the two files the host owns — the providers and the models —
    /// and replaces what is in memory. Nothing is fetched: the files are a
    /// person's, and a plugin that writes them is what changes them.
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
    /// A chunk of a running tool's output, as it arrives.
    ///
    /// A tool that streams — the shell executor does — reports what it reads
    /// while it reads it. The chunks are a view, not the result: the result is
    /// `ToolFinished`, and a client that ignores these only loses the live
    /// view.
    ToolOutput {
        /// The turn.
        turn: u64,
        /// The call the output belongs to.
        call_id: String,
        /// The tool.
        tool: String,
        /// Which stream the bytes came from: `stdout` or `stderr`.
        stream: OutputStream,
        /// The text, already redacted by the tool and decoded as UTF-8.
        text: String,
        /// Whether this chunk closes that stream.
        #[serde(default)]
        last: bool,
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
        /// Whether the call was let go of rather than finished.
        ///
        /// A detached call is reported as an error like any other — it did not
        /// run to the end, and the model is told so — but it did not fail: it
        /// was asked to stop waiting. A client that draws every error as a
        /// failure needs the difference, and reading it out of the text would be
        /// guessing at a string the tool owns.
        #[serde(default)]
        detached: bool,
        /// How long it ran, in milliseconds.
        duration_ms: u64,
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
        /// Who spoke.
        role: MessageRole,
        /// The message's text, with each part on its own line.
        text: String,
    },
    /// The session's task list, as the last `todo` call left it.
    ///
    /// The list is the model's, written through the `todo` tool and part of the
    /// conversation, but a client that just attached has not seen that call —
    /// the replay shows messages, and the list is a widget above the input. This
    /// is sent once in a resume's prefix, from the same call the model made, so
    /// the widget comes back with the session instead of starting empty until
    /// the model happens to write it again.
    SessionTodos {
        /// One entry per task, in the model's order.
        todos: Vec<TodoInfo>,
    },
    /// Compaction started.
    CompactionStarted {
        /// Why it started.
        #[serde(default)]
        reason: CompactionReason,
    },
    /// Compaction finished, possibly with a new Norte session behind the same
    /// logical one.
    CompactionFinished {
        /// Whether compaction happened.
        compacted: bool,
        /// The new Norte session id, when one was created.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        north_session: Option<String>,
        /// The summary the model produced, which is what the fresh session is
        /// seeded with. Sent so a client can show what the history was replaced
        /// by, which is the only way to judge a summary after the fact; a
        /// client that does not care ignores it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
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
    /// The background jobs of one session.
    JobList {
        /// One entry per job, oldest first.
        jobs: Vec<JobInfo>,
    },
    /// Background jobs were stopped on request.
    ///
    /// It answers `StopJobs` rather than reporting a turn: stopping a job is
    /// not stopping the turn that started it, and a client that asked for one
    /// needs to know how many it reached.
    JobsStopped {
        /// How many jobs the request stopped.
        stopped: usize,
    },
    /// The engine applied a steering message to the request already in flight.
    SteerApplied {
        /// The names of the messages that reached the request.
        ids: Vec<String>,
        /// The messages themselves, in the order they were applied.
        ///
        /// A client shows them where the conversation reached, so the text has
        /// to travel with the notice: without it the client learns that its own
        /// message landed but has nothing to print, and a message it never saw
        /// (a finished job's report, which the human did not type) can only be
        /// shown by the client that is told its text.
        #[serde(default)]
        messages: Vec<String>,
    },
    /// The session's model changed.
    ModelChanged {
        /// The provider that serves it.
        provider: String,
        /// The new model id.
        model: String,
    },
    /// A model switch was accepted while the session was busy and will be
    /// applied at the next turn boundary.
    ModelQueued {
        /// The provider that will serve it.
        provider: String,
        /// The requested model id.
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
    /// The catalogue was re-read: how many models it now carries.
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

/// One piece of media travelling with a message.
///
/// It is a path, not the bytes: the client and the service share a filesystem,
/// and a pasted photo is a file either way. The service reads it where its blob
/// store is and files it there, so the bytes never travel on the wire.
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
