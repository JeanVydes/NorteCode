use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime::Phase;
use norte::capability::ReasoningRequest;
use norte::domain::logprob::TokenLogprob;
use norte::domain::moderation::SafetyRating;
use norte::domain::part::Part;
use norte::id::{CallId, Digest, SessionId, ToolName};
use norte::model::GenerationParams;
use norte::tool::ToolStream;

/// A streamed delta as observers see it. Continuity material is reduced to its
/// length: token payloads never enter the event channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "delta")]
#[non_exhaustive]
pub enum StreamDelta {
    /// Answer text.
    Text {
        /// The index of the part in the message.
        index: u32,
        /// The text that arrived in this delta.
        text: String,
    },
    /// Reasoning text, where the provider separates it from the answer.
    Reasoning {
        /// The index of the part in the message.
        index: u32,
        /// The reasoning that arrived in this delta.
        text: String,
    },
    /// Refusal text.
    Refusal {
        /// The index of the part in the message.
        index: u32,
        /// The refusal that arrived in this delta.
        text: String,
    },
    /// A fragment of a tool call's arguments.
    ToolArguments {
        /// The index of the part in the message.
        index: u32,
        /// The call the arguments belong to.
        call_id: CallId,
        /// The fragment of the arguments document.
        arguments: String,
    },
    /// Generated media, as a byte count.
    Media {
        /// The index of the part in the message.
        index: u32,
        /// How many bytes this delta carries.
        bytes: usize,
        /// Whether the part ends with this delta.
        last: bool,
    },
    /// Continuity material, reduced to its length.
    Continuity {
        /// The index of the part in the message.
        index: u32,
        /// How many bytes the fragment carries.
        len: usize,
    },
    /// Safety ratings the provider reported for the answer.
    SafetyRatings {
        /// The ratings, as the provider reported them.
        ratings: Vec<SafetyRating>,
    },
    /// Token probabilities the provider reported for a chunk of the answer.
    Logprobs {
        /// The index of the part in the message.
        index: u32,
        /// One entry per token, in the order the provider reported them.
        entries: Vec<TokenLogprob>,
    },
}

/// Typed notification channel. Durable events are written to the store before
/// they are emitted; observers may lose non-critical events under back-pressure
/// but never durability ordering.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum EngineEvent {
    /// A session was created and committed.
    SessionCreated {
        /// The session.
        session: SessionId,
        /// The phase it was created in.
        phase: Phase,
    },
    /// The session moved from one phase to another.
    PhaseChanged {
        /// The session.
        session: SessionId,
        /// The phase it left.
        from: Phase,
        /// The phase it entered.
        to: Phase,
    },
    /// A delta arrived from the model.
    StreamDelta {
        /// The session.
        session: SessionId,
        /// The turn that is running.
        turn: u64,
        /// The delta.
        delta: StreamDelta,
    },
    /// A part of the assistant message is settled.
    PartCompleted {
        /// The session.
        session: SessionId,
        /// The turn that is running.
        turn: u64,
        /// The part's index in the assistant message.
        index: u32,
        /// The settled part.
        part: Part,
    },
    /// An assistant generation and any interrupted-call results are durable.
    ///
    /// Emitted after the commit, before executing tools or consulting end-of-turn
    /// interceptors. The stored head may be a tool result on interruption.
    GenerationCommitted {
        /// The session.
        session: SessionId,
        /// The turn the generation belongs to.
        turn: u64,
    },
    /// A tool call started.
    ToolCallStarted {
        /// The session.
        session: SessionId,
        /// The turn that is running.
        turn: u64,
        /// The call.
        call_id: CallId,
        /// The tool that is running.
        tool: ToolName,
    },
    /// A chunk of a running tool's output, as it arrives.
    ///
    /// A tool that streams — the shell executor does — reports what it reads
    /// while it reads it, so a host can show the output live instead of waiting
    /// for the call to finish. The chunks are not the result: the result is the
    /// `ToolCallFinished` that follows, and a host that ignores these loses
    /// nothing but the live view.
    ToolOutput {
        /// The session.
        session: SessionId,
        /// The turn that is running.
        turn: u64,
        /// The call the output belongs to.
        call_id: CallId,
        /// The tool that is running, as its sisters report it.
        tool: ToolName,
        /// The stream the bytes came from.
        stream: ToolStream,
        /// The bytes themselves, already redacted by the tool.
        bytes: Vec<u8>,
        /// Whether this chunk ends that stream.
        last: bool,
    },
    /// A tool call finished.
    ToolCallFinished {
        /// The session.
        session: SessionId,
        /// The turn that is running.
        turn: u64,
        /// The call.
        call_id: CallId,
        /// The tool that ran.
        tool: ToolName,
        /// Whether the tool reported an error.
        is_error: bool,
        /// How long the tool ran, in milliseconds.
        duration_ms: u64,
    },
    /// A host ended the turn on purpose (FR-ENG-10).
    TurnStopped {
        /// The session.
        session: SessionId,
        /// The interceptor that stopped it.
        interceptor: String,
        /// Why the turn ended.
        reason: String,
    },
    /// A host seeded durable history onto the session head without generating
    /// (FR-ENG-6).
    HistoryAppended {
        /// The session.
        session: SessionId,
        /// How many messages were appended.
        messages: usize,
    },
    /// The engine applied the messages an interceptor returned (FR-ENG-10).
    InterceptionApplied {
        /// The session.
        session: SessionId,
        /// How many messages were appended.
        messages: usize,
    },
    /// A commit landed.
    CommitSucceeded {
        /// The session.
        session: SessionId,
        /// The version the session reached.
        version: u64,
    },
    /// A commit was refused because the stored session moved.
    CommitConflict {
        /// The session.
        session: SessionId,
        /// The version the commit was built from.
        expected: Option<u64>,
        /// The version the store holds.
        actual: u64,
    },
    /// A layer answered without calling the provider.
    LayerShortCircuited {
        /// The session.
        session: SessionId,
        /// The layer's name.
        layer: String,
        /// Why it answered.
        reason: norte::layer::ShortCircuitReason,
    },
    /// The engine reconstructed a request: the digest over the exact wire, the
    /// deterministic cache boundary positions and the effective request
    /// configuration (FR-ENG-9, FR-EVT-4).
    RequestReconstructed {
        /// The session.
        session: SessionId,
        /// The turn the request belongs to.
        turn: u64,
        /// A content hash of the assembled request.
        digest: Digest,
        /// The prefix positions the provider may cache up to.
        cache_boundaries: Vec<u32>,
        /// The generation parameters the request carries.
        params: GenerationParams,
        /// The reasoning ask the request carries.
        reasoning: ReasoningRequest,
        /// The tools the request exposes, by name.
        tools: Vec<ToolName>,
    },
    /// A host action invalidated the cached prefix; the reason names the knob
    /// that caused it (FR-EVT-4, FR-RECON-9).
    PrefixInvalidated {
        /// The session.
        session: SessionId,
        /// What invalidated the prefix.
        reason: String,
    },
    /// The turn failed.
    Error {
        /// The session, when the error is about one.
        session: Option<SessionId>,
        /// What went wrong.
        message: String,
    },
    /// An event a layer published under its own namespace.
    Extension {
        /// The namespace the publisher chose.
        namespace: String,
        /// The payload.
        data: Value,
    },
}

impl EngineEvent {
    /// The session the event is about, when it is about one.
    #[must_use]
    pub fn session(&self) -> Option<SessionId> {
        match self {
            EngineEvent::SessionCreated { session, .. }
            | EngineEvent::PhaseChanged { session, .. }
            | EngineEvent::StreamDelta { session, .. }
            | EngineEvent::PartCompleted { session, .. }
            | EngineEvent::GenerationCommitted { session, .. }
            | EngineEvent::ToolCallStarted { session, .. }
            | EngineEvent::ToolOutput { session, .. }
            | EngineEvent::ToolCallFinished { session, .. }
            | EngineEvent::HistoryAppended { session, .. }
            | EngineEvent::TurnStopped { session, .. }
            | EngineEvent::InterceptionApplied { session, .. }
            | EngineEvent::CommitSucceeded { session, .. }
            | EngineEvent::CommitConflict { session, .. }
            | EngineEvent::LayerShortCircuited { session, .. }
            | EngineEvent::RequestReconstructed { session, .. }
            | EngineEvent::PrefixInvalidated { session, .. } => Some(*session),
            EngineEvent::Error { session, .. } => *session,
            EngineEvent::Extension { .. } => None,
        }
    }
}

/// Minimal sink layers use to publish side-channel events without depending on
/// the engine.
pub trait EventSink: Send + Sync {
    /// Publishes one event; a sink never blocks the turn on back-pressure.
    fn emit(&self, event: EngineEvent);
}

impl EventSink for tokio::sync::mpsc::Sender<EngineEvent> {
    fn emit(&self, event: EngineEvent) {
        if self.try_send(event).is_err() {
            tracing::debug!("engine event dropped: sink full or closed");
        }
    }
}

impl EventSink for std::sync::Arc<dyn Fn(EngineEvent) + Send + Sync> {
    fn emit(&self, event: EngineEvent) {
        self(event);
    }
}
