//! The Norte Code client wire: requests and events.
//!
//! These are the shapes a client sends and a server answers with. They live in
//! this crate, next to the payloads and the resources, so a client that cannot
//! compile the engine (the browser) still speaks the whole protocol instead of
//! a hand-copied subset of it.

#![forbid(unsafe_code)]
#![allow(clippy::wildcard_imports)]

use serde::{Deserialize, Serialize};

use crate::types::*;
use crate::{MediaPart, QueueMode, TelemetryRecord};

fn default_resume_replay() -> bool {
    true
}

/// One tool and the rule currently deciding whether it may run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolInfo {
    /// The tool name.
    pub name: String,
    /// Whether a call is allowed.
    pub allowed: bool,
    /// `session`, `global`, or `default`.
    pub scope: String,
}

/// Which attachments a session fork copies from the source session.
///
/// Every flag defaults to `false`: an old fork that asked for no copies keeps
/// copying none. Whether uploads and outputs are copied by default, and what
/// the host does with the flags, is decided by the host, not by the wire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkInclude {
    /// Whether the fork copies the source session's uploads.
    #[serde(default)]
    pub uploads: bool,
    /// Whether the fork copies the source session's produced outputs.
    #[serde(default)]
    pub outputs: bool,
    /// Whether the fork copies the source session's workspace.
    #[serde(default)]
    pub workspace: bool,
}

/// How a compaction replaces the history it removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactStrategy {
    /// Summarise the removed history and seed a fresh session with it.
    Summarize,
    /// Drop the removed history without summarising it.
    Truncate,
}

/// Why a compaction was requested.
///
/// Clients ask for manual compaction with [`CompactTrigger::Manual`].
/// `Auto` is reserved for the server itself: it names a compaction the
/// service started without a client request (for example the automatic
/// context-threshold compaction).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactTrigger {
    /// A client asked for compaction explicitly.
    #[default]
    Manual,
    /// The server started compaction on its own (automatic).
    Auto,
}

/// What happens to the workspace when history is rewound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindWorkspace {
    /// Leave the workspace files as they are.
    Keep,
    /// Restore the workspace to the snapshot at the rewind point.
    Restore,
}

/// The shape a service event's turn answer must take.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormatType {
    /// Plain text.
    Text,
    /// JSON validated against [`ResponseFormat::schema`].
    JsonSchema,
}

/// The shape a `SendMessage` turn's final answer must take.
///
/// The wire key is `type`, as the standard names it: `{"type":"text"}` or
/// `{"type":"json_schema","schema":...,"strict":...}`. The Rust field is
/// called `kind` because `type` is a keyword.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseFormat {
    /// Which shape the answer takes.
    #[serde(rename = "type")]
    pub kind: ResponseFormatType,
    /// The JSON Schema the `json_schema` answer must validate against.
    ///
    /// At most 16 KiB with local `$ref` only, as the standard requires; the
    /// host validates that, not the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    /// Whether a schema-invalid answer fails the turn, instead of earning a
    /// correction message to the provider.
    #[serde(default)]
    pub strict: bool,
}

/// A modality a model may produce for a turn.
///
/// The wire's name for what a client asks for in `SendMessage.output`: plain
/// text, rendered media, or a document. It mirrors [`Modality`] for output,
/// saying what the service may hand back rather than what it accepts as
/// input; input keeps using `Modality` and `Attachment`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputModality {
    /// Plain text.
    Text,
    /// An image.
    Image,
    /// Recorded audio.
    Audio,
    /// Recorded video.
    Video,
    /// A document, which is what a PDF is.
    Document,
}

/// A model chosen for a session and not yet applied.
///
/// A provider/model pair is never one string: a model id may carry slashes
/// of its own, so the two are never written as one to be taken apart again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingModel {
    /// The provider that serves it.
    pub provider: String,
    /// The model id.
    pub model: String,
}

/// A model, reasoning or budget change waiting for the next turn.
///
/// This is what a session configure with `apply: next_turn` carries: the
/// service stores it when the turn is busy and applies it when the next turn
/// starts. Every field is optional and means "change only this".
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingOverride {
    /// The model switch waiting for the next turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<PendingModel>,
    /// The reasoning setting waiting for the next turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    /// The budget waiting for the next turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
}

/// When a session configure is applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigureApply {
    /// Apply immediately; refused when a turn is running.
    #[default]
    Now,
    /// Queue for the next turn boundary.
    NextTurn,
}

/// Which sections an export bundle carries.
/// The many bools here mirror the bundle's own include flags; the lint
/// that counts bools in a struct does not apply because unset sections
/// are worse (three-valued include state) than packed bools.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportInclude {
    /// Whether child session histories are included.
    #[serde(default)]
    pub children: bool,
    /// Whether uploaded blobs are included.
    #[serde(default)]
    pub uploads: bool,
    /// Whether detached job outputs are included.
    #[serde(default)]
    pub job_output: bool,
    /// Whether produced outputs are included.
    #[serde(default)]
    pub outputs: bool,
    /// Whether the workspace manifest is included.
    #[serde(default)]
    pub workspace: bool,
}

impl Default for ExportInclude {
    /// The host default: children, uploads, job output and outputs on.
    fn default() -> Self {
        Self {
            children: true,
            uploads: true,
            job_output: true,
            outputs: true,
            workspace: false,
        }
    }
}

/// What to do when an export is requested while a session is busy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportQuiesce {
    /// Refuse with `SESSION_BUSY` when the session is not idle.
    #[default]
    Reject,
    /// Wait for the session to become idle before exporting.
    Wait {
        /// How long to wait, in seconds (0 means the default).
        #[serde(default)]
        timeout_s: u64,
    },
    /// Stop the session's background jobs first, then export.
    KillJobs,
}

/// The declared side effects of one tool call that a rewind removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SideEffect {
    /// The tool call id.
    pub tool_call_id: String,
    /// The tool name the model called.
    pub tool_name: String,
    /// The declared side effects, in the wire's spelling.
    pub side_effects: String,
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
    /// Copy a session's history into a new session.
    ForkSession {
        /// The session to copy.
        session: SessionKey,
        /// A label for the copy.
        title: Option<String>,
        /// The terminal turn to fork from. Defaults to the latest terminal turn.
        #[serde(default)]
        from_turn_id: Option<String>,
        /// Whether the named turn is included. Defaults to true.
        #[serde(default)]
        inclusive: Option<bool>,
        /// Whether usage counters are carried to the fork. Defaults to true.
        #[serde(default)]
        carry_usage: Option<bool>,
        /// The agent the fork runs on, when it is not the source session's
        /// agent. `None` keeps the source session's agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        /// Which attachments the fork copies. `None` copies none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include: Option<ForkInclude>,
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
        /// The modalities this turn may produce, in the order it should
        /// produce them.
        ///
        /// An empty list produces text, as a turn always has. This is what
        /// the host validates against the negotiated output modalities; the
        /// service itself only answers with text.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        output: Vec<OutputModality>,
        /// The shape the turn's final answer must take.
        ///
        /// `None` is a plain text answer, the default. A host that cannot
        /// produce the shape refuses the request instead of ignoring it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_format: Option<ResponseFormat>,
        /// Caller metadata (RFC 12.4): an opaque object of at most 4 KiB,
        /// stored with the turn and never shown to the model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
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
    ///
    /// The `queue_mode` on the queued message decides what happens when
    /// the session is already running a turn: `Reject` answers
    /// `SessionBusy`, `Queue` puts the message in front of the request in
    /// flight, and `Interrupt` cancels that turn before queueing.
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
        /// What to do when the session is already running a turn.
        #[serde(default)]
        queue_mode: QueueMode,
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
        /// Whether to replay the committed transcript before continuing.
        #[serde(default = "default_resume_replay")]
        replay: bool,
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
    /// List the catalogue with its origin: what is editable in the files and
    /// what a plugin injected.
    ListCatalog,
    /// Re-read the catalogue files.
    ///
    /// It reads the two files the host owns — the providers and the models —
    /// and replaces what is in memory. Nothing is fetched: the files are a
    /// person's, and a plugin that writes them is what changes them.
    RefreshCatalog,
    /// Add or replace a provider in `providers.json`.
    ///
    /// Only catalogue files are written: a provider a plugin injects is the
    /// plugin's, and editing it here would be overwritten on the next start.
    CatalogUpsertProvider {
        /// The provider id, the key it lives under in the file.
        provider_id: String,
        /// The entry to write under that key.
        provider: crate::catalog::Provider,
    },
    /// Remove a provider and every model that names it.
    CatalogRemoveProvider {
        /// The provider id to remove.
        provider_id: String,
    },
    /// Add or replace a model in `models.json`, keyed by `provider_id` + `id`.
    CatalogUpsertModel {
        /// The entry to write.
        model: crate::catalog::Model,
    },
    /// Remove one model from `models.json`.
    CatalogRemoveModel {
        /// The provider the model belongs to.
        provider_id: String,
        /// The model id the provider expects.
        model_id: String,
    },
    /// List the plans of a session, newest first.
    ListPlans {
        /// The session whose plans are listed.
        session: SessionKey,
    },
    /// List the installed Agent Skills.
    ListSkills,
    /// Re-scan the skills roots.
    RefreshSkills,
    /// List plugins installed on the remote host.
    ListPlugins,
    /// Install one plugin from a remote git repository.
    InstallPlugin {
        /// Repository URL.
        url: String,
        /// Optional workspace package.
        package: Option<String>,
        /// Install every plugin package in the workspace.
        all: bool,
    },
    /// Update one plugin, or every plugin with a recorded source.
    UpdatePlugins {
        /// Optional plugin name.
        name: Option<String>,
    },
    /// Remove one plugin.
    RemovePlugin {
        /// Plugin name.
        name: String,
    },
    /// Enable or disable one plugin on the remote host.
    SetPluginEnabled {
        /// Plugin name.
        name: String,
        /// Desired state.
        enabled: bool,
    },
    /// Report the ledger's totals.
    CostReport {
        /// One session, or every session.
        session: Option<SessionKey>,
    },
    /// Return provider request telemetry from the service host.
    ListTelemetry {
        /// One session, or every session.
        session: Option<SessionKey>,
    },
    /// Return tool rules as evaluated by the service host.
    ListTools {
        /// The session whose rules should be shown.
        session: Option<SessionKey>,
    },
    /// Return MCP server state from the service host.
    ListMcp,
    /// Rebuild MCP connections from the service configuration.
    RefreshMcp,
    /// Enable or disable one connected MCP server.
    SetMcpActive {
        /// Server name.
        name: String,
        /// Desired state.
        active: bool,
    },
    /// Set an allow or deny rule.
    SetApprovalRule {
        /// The session rule belongs to, when it is not global.
        session: Option<SessionKey>,
        /// Tool name.
        tool: String,
        /// Whether the tool is allowed.
        allow: bool,
        /// Persist for every session.
        global: bool,
    },
    /// Remove an allow or deny rule.
    ForgetApprovalRule {
        /// The session rule belongs to, when it is not global.
        session: Option<SessionKey>,
        /// Tool name.
        tool: String,
        /// Remove the global rule.
        global: bool,
    },
    /// Compact a session now, with an explicit strategy.
    ///
    /// This is the wire form of `SESSION_COMPACT`; the plain [`Compact`]
    /// variant keeps the one-argument shape for clients that name no
    /// strategy. At most one compaction runs at a time: a request while one
    /// is running, or while a turn is active, is refused.
    SessionCompact {
        /// The session.
        session: SessionKey,
        /// How the removed history is replaced.
        strategy: CompactStrategy,
        /// How many of the most recent turns to keep verbatim, when stated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keep_last_turns: Option<u32>,
        /// Guidance for the summary, at most 1 KiB, when stated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
        /// Why compaction was requested. Clients send `manual`; `auto`
        /// names a compaction the server started on its own.
        #[serde(default)]
        trigger: CompactTrigger,
    },
    /// Pin or unpin turns so compaction keeps them verbatim.
    ContextPin {
        /// The session.
        session: SessionKey,
        /// The turns to pin or unpin.
        turn_ids: Vec<String>,
        /// Whether to pin them (`true`) or unpin them (`false`).
        pinned: bool,
    },
    /// Rewind a session to a turn, dropping everything after it.
    SessionRewind {
        /// The session.
        session: SessionKey,
        /// The turn to rewind to.
        to_turn_id: String,
        /// Whether to rewind that turn as well. Defaults to `false`.
        #[serde(default)]
        inclusive: bool,
        /// Whether to cancel a pending turn. Defaults to `false`.
        #[serde(default)]
        cancel_pending: bool,
        /// What happens to the workspace files.
        workspace: RewindWorkspace,
    },
    /// Fetch one page of a session's history.
    HistoryGet {
        /// The session.
        session: SessionKey,
        /// The page ends before this turn, when stated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_turn_id: Option<String>,
        /// How many items the page holds.
        limit: u32,
        /// Whether rewound items are included.
        include_rewound: bool,
    },
    /// Export a session as a bundle the host packages.
    SessionExport {
        /// The session to export.
        session: SessionKey,
        /// Which sections the bundle carries.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include: Option<ExportInclude>,
        /// What to do when the session is busy.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quiesce: Option<ExportQuiesce>,
    },
    /// Import a bundle produced by [`ClientRequestKind::SessionExport`].
    SessionImport {
        /// The bundle as JSON (the host unpacks the archive form).
        bundle: serde_json::Value,
        /// The JWK that decrypts an encrypted bundle, when one was supplied.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decryption_jwk: Option<serde_json::Value>,
    },
}

/// What a replayed message carries, so the client renders it the way the
/// conversation originally drew it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayedKind {
    /// Plain text the model or the human said.
    Text,
    /// Reasoning the model produced.
    Reasoning,
    /// A tool call the model asked for.
    ToolCall,
    /// The result of a tool call.
    ToolResult,
    /// A refusal the model produced.
    Refusal,
}

/// Who spoke, as a history page renders an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistorySpeaker {
    /// The human.
    User,
    /// The model.
    Assistant,
    /// A tool result.
    Tool,
    /// A system event.
    SystemEvent,
}

/// One item of a history page.
///
/// The wire's plain form of one history entry: who spoke, what they said, and
/// when. A host that keeps more — a rewound flag, an origin, a tool call —
/// maps it onto these fields; the data the wire has no field for stays in
/// `provenance`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryItem {
    /// The item's stable id.
    pub id: String,
    /// What kind of item this is: `user`, `assistant`, `tool`,
    /// `system_event`, `reasoning`, `tool_call`, `tool_result` or `refusal`.
    pub kind: String,
    /// The item's text.
    pub content: String,
    /// When it was created, as Unix milliseconds.
    pub created_at: i64,
    /// The turn it belongs to, when it belongs to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Who spoke.
    pub role: HistorySpeaker,
    /// What the host knows about where the item came from, when it knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<serde_json::Value>,
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
        /// The terminal turn copied into the fork.
        #[serde(default)]
        from_turn_id: Option<String>,
        /// The title of the fork.
        #[serde(default)]
        title: Option<String>,
    },
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
    /// A complete generated media part, ready for durable host storage.
    MediaPartCompleted {
        /// The turn.
        turn: u64,
        /// The part's index in the assistant message.
        index: u32,
        /// The generated media and its metadata.
        media: MediaPart,
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
        /// The provider that served the request, when the host reports it.
        ///
        /// A session can switch models between turns, so the transcript's
        /// footer names what actually answered each one rather than the
        /// session's current model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// The model that served the request, when the host reports it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
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
        /// What kind of content this carries, so the client renders it as the
        /// conversation originally drew it rather than as one block of text.
        kind: ReplayedKind,
        /// The content's text.
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
    /// Provider request telemetry from the service host.
    Telemetry {
        /// Request rows, oldest first.
        records: Vec<TelemetryRecord>,
    },
    /// Tool approval state from the service host.
    ToolList {
        /// Tool rules, sorted by tool name.
        tools: Vec<ToolInfo>,
    },
    /// MCP server state from the service host.
    McpList {
        /// Connected servers.
        servers: Vec<McpInfo>,
        /// Servers that failed to start.
        failures: Vec<McpFailure>,
    },
    /// The plugins installed on the remote host.
    PluginList {
        /// Plugin metadata.
        plugins: Vec<PluginInfo>,
    },
    /// A plugin lifecycle operation completed.
    PluginChanged {
        /// Operation description.
        operation: String,
        /// Affected plugin name.
        name: String,
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
    /// A detached tool call became a background job.
    JobStarted {
        /// The job's stable id.
        job_id: String,
        /// The tool call that started it.
        tool_call_id: String,
        /// The command or task description.
        command: String,
        /// Unix timestamp in milliseconds when the job started.
        #[serde(default)]
        started_at: u64,
        /// Effective runtime and output limits.
        #[serde(default)]
        limits: JobLimits,
    },
    /// A detached job wrote a chunk of output.
    JobProgressDelta {
        /// The job the output belongs to.
        job_id: String,
        /// Which output stream produced the chunk.
        stream: OutputStream,
        /// The text in this chunk.
        delta: String,
        /// Byte offset in the job's combined output.
        index: u64,
    },
    /// A background job reached a terminal state.
    JobFinished {
        /// Its final summary.
        job: JobInfo,
        /// Whether this result started a new turn.
        triggered_new_turn: bool,
        /// The turn started for this result, when one was started.
        #[serde(default)]
        new_turn_id: Option<u64>,
        /// The active turn that received this result, when one was injected.
        #[serde(default)]
        injected_into_turn_id: Option<u64>,
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
    /// The editable catalogue: the providers and models written in the files,
    /// with which of them came from a plugin and so cannot be edited here.
    CatalogDetail {
        /// One entry per provider, file and plugin alike.
        providers: Vec<CatalogProviderInfo>,
        /// One entry per model.
        models: Vec<CatalogModelInfo>,
    },
    /// The plans of a session.
    PlanList {
        /// One entry per plan.
        plans: Vec<PlanInfo>,
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
        /// Whether the final text validated against the turn's
        /// `response_format`, when one was requested.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_schema_valid: Option<bool>,
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
    /// A session was exported (RFC 11.1).
    SessionExportReady {
        /// The export id.
        export_id: String,
        /// The bundle as JSON (this host does not build ZIP archives yet).
        bundle: serde_json::Value,
    },
    /// A session was imported (RFC 11.2).
    ImportResult {
        /// The new session id.
        session_id: String,
        /// Old id → new id.
        id_map: serde_json::Value,
    },
    /// A child session was started (RFC 8.2.10).
    ChildSessionStarted {
        /// The child session id.
        child_session_id: String,
        /// The child agent id.
        agent_id: String,
        /// The tool call that spawned it.
        tool_call_id: String,
        /// The delegation lifetime.
        lifetime: String,
        /// Task summary (up to 512 chars).
        task_summary: String,
    },
    /// A child session changed state (RFC 8.2.10).
    ChildSessionUpdated {
        /// The child session id.
        child_session_id: String,
        /// The new child state.
        state: crate::ChildState,
        /// Why it changed.
        reason: String,
        /// The child's final result, when it reached a terminal state.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        /// Whether the child's output validated against `output_schema`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_schema_valid: Option<bool>,
    },
    /// Delegator state changed (RFC 8.2.11).
    DelegatorStateChanged {
        /// The child session id.
        child_session_id: String,
        /// Whether the delegator is suspended.
        delegator_suspended: bool,
        /// Who changed it: `client`, `server` or the parent agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
        /// Messages held while the delegator is suspended.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queued_messages: Option<usize>,
    },
    /// A child session was asked a question and is waiting for its answer
    /// (RFC 8.2.9). The question is the same one the child's `ask_user`
    /// tool asked; a human answers it on the child's session.
    ChildAwaitingInput {
        /// The child session id.
        child_session_id: String,
        /// The question id the child asked.
        question: String,
        /// The prompt the child asked.
        prompt: String,
    },
    /// One page of a session's history, answering [`ClientRequestKind::HistoryGet`].
    HistoryPage {
        /// The page's items, oldest first.
        items: Vec<HistoryItem>,
        /// Whether an earlier page remains.
        has_more: bool,
    },
    /// History was rewound to a turn.
    HistoryRewound {
        /// The turn rewound to, in the wire's spelling.
        to_turn_id: String,
        /// Whether that turn itself was removed.
        inclusive: bool,
        /// The turns that were removed, oldest first.
        rewound_turn_ids: Vec<String>,
        /// The tool calls that ran in the removed turns, with their declared
        /// side effects. Calls whose declaration is `side_effects: none` are
        /// not repeated here.
        side_effects: Vec<SideEffect>,
        /// Whether the workspace was restored to the rewind point.
        workspace_restored: bool,
    },
    /// A non-text output part started on a turn.
    OutputPartStarted {
        /// The turn.
        turn: u64,
        /// The output id (the host's `part_id`).
        part_id: String,
        /// The part index in the assistant message.
        index: u32,
        /// The modality in the wire's spelling.
        modality: OutputModality,
        /// The MIME type, when the part names one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        /// Whether the bytes arrived with the stream.
        streamed: bool,
        /// How many bytes are expected, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes_expected: Option<u64>,
    },
    /// A non-text output part finished on a turn.
    OutputPartFinished {
        /// The turn.
        turn: u64,
        /// The output id.
        part_id: String,
        /// The part index in the assistant message.
        index: u32,
        /// The modality in the wire's spelling.
        modality: OutputModality,
        /// How many bytes were stored.
        bytes: u64,
        /// The SHA-256 of the stored bytes.
        sha256: String,
        /// The blob id the bytes live in, when the service stored them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blob: Option<String>,
        /// Why the part failed, when it did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
