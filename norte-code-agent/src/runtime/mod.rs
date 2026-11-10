#![forbid(unsafe_code)]
#![doc = "One reference agent harness over Norte's contracts: turns, the tool loop, approvals and streaming.

Norte's contracts live in [`norte`]: the [`Model`](norte::Model) port, the
[`Store`](norte::Store), the [`ToolRegistry`](norte::ToolRegistry), the
policies, the layers and the observers. This crate is one way to drive them:
[`Harness`] walks a session forward one turn at a time.

It is deliberately *a* harness and not *the* harness. Nothing in the core refers
to it, nothing forces a host to use it, and a host that wants a different turn
policy: a different loop, its own scheduling, a graph instead of a session -
depends on `norte-core` alone and writes it. What the core owns is the contracts
and the durable state; what this crate owns is one order of operations over them.


[`Model`]: norte::Model
[`Store`]: norte::Store
[`ToolRegistry`]: norte::ToolRegistry
"]

/// The events the harness emits while a turn runs.
pub mod domain;
#[allow(missing_docs)]
pub mod event;
/// In-memory reference implementations of the ports.
pub mod interceptor;
/// Observers and transforms that watch a turn's events.
pub mod observer;
/// Building and reconstructing one request.
pub mod request;
/// Reading and committing a session's durable state.
pub mod session;
/// The ports the runtime persists through.
/// Running the calls a model asked for.
pub mod tools;
/// The turn loop.
pub mod turn;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use norte_code_protocol::Commit;

pub use crate::runtime::domain::conversation::{CONVERSATION_SCHEMA_VERSION, Conversation};
pub use crate::runtime::event::{EngineEvent, EventSink, StreamDelta};
pub use crate::runtime::interceptor::{
    ConsultationPoint, Interception, InterceptionContext, InterceptorSet, TurnInterceptor,
};
pub use crate::runtime::observer::{Observer, ObserverSet, StreamTransform};
mod builder;
pub use crate::runtime::builder::{HarnessBuilder, TurnBuilder};
pub use crate::runtime::turn::Turn;
use norte::BlobStore;
use norte::capability::ReasoningRequest;
use norte::domain::message::{Message, Usage};
use norte::domain::part::{Part, ToolCallPart, ToolResultPart};
use norte::domain::role::Role;
use norte::error::{Error, Result};
use norte::id::{CallId, Digest, SessionId, ToolName};
use norte::layer::{Continuation, LayerContext, LayerStack};
use norte::media_adapter::MediaAdapter;
use norte::model::{
    GenerationParams, Model, ModelRequest, ModelStream, StreamLimits, TURN_OPTIONS_KEY, TurnOptions,
};
use norte::preprocess::MediaPreprocessor;
use norte::stream::StreamEvent;
use norte::tool::registry::ToolRegistry;
use norte::tool::{
    ProgressSink, ToolContext, ToolMetadataProvider, ToolOutput, ToolProgress, ToolSpec,
};
pub use norte_code_protocol::Session;
pub use norte_code_protocol::Store;
pub use norte_code_protocol::{Node, NodeKind};
pub use norte_code_wire::Phase;

/// The turn-level policy that is not a layer: what a turn inherits and the
/// bounds the engine enforces.
///
/// The inheritable fields ([`reasoning`](Self::reasoning),
/// [`params`](Self::params)) are defaults, not floors: a turn overlays its own
/// values per field. The rest are bounds the engine applies to every turn.
#[derive(Clone, Debug)]
pub struct HarnessConfig {
    /// The reasoning ask a turn inherits when it does not set one.
    ///
    /// [`ReasoningRequest::Automatic`] by default: the engine asks for whatever
    /// the model does on its own unless the host says otherwise.
    pub reasoning: ReasoningRequest,
    /// The generation parameters a turn inherits.
    ///
    /// A turn merges its own over these per field, so a value set here is the
    /// baseline the turn may override, never a value it cannot change.
    pub params: GenerationParams,
    /// The bounds on buffered observer events and on one stream's size.
    ///
    /// A slow observer never stalls a turn: the observer set drops what does not
    /// fit and counts it. [`StreamLimits::default`] sizes the buffer for
    /// ordinary use.
    pub limits: StreamLimits,
    /// Media above this many bytes is moved to the blob store instead of
    /// travelling inline in the history. 64 KiB by default.
    pub inline_threshold: usize,
    /// How many requests one turn may spend before it fails with
    /// [`Error::InvalidInput`].
    ///
    /// 16 by default. The count covers every round of the tool loop, and every
    /// request that ran was committed and billed before the error: a cost meter
    /// must keep counting past it. A coding agent that reads, edits and reads
    /// back raises it.
    pub max_iterations: usize,
    /// The host's per-call tool metadata provider (FR-TOOL-7).
    ///
    /// The engine consults it immediately before every execution, including one
    /// that resumes a parked call, and fills [`ToolContext::metadata`] with the
    /// result. `None` leaves the map empty. The values are execution-scoped:
    /// they are never written to the session, the graph or the wire.
    pub tool_metadata: Option<ToolMetadataProvider>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            reasoning: ReasoningRequest::default(),
            params: GenerationParams::default(),
            limits: StreamLimits::default(),
            inline_threshold: 64 * 1024,
            max_iterations: 16,
            tool_metadata: None,
        }
    }
}

/// What a finished turn leaves behind.
#[derive(Clone, Debug)]
pub struct TurnOutcome {
    /// The session as committed.
    pub session: Session,
    /// The assistant message the turn committed, when it committed one.
    pub message: Option<Message>,
}

/// The reconstruction output: the wire-ready message sequence and the positions
/// where the provider may cache the prefix (FR-RECON-7).
#[derive(Clone, Debug)]
struct Reconstructed {
    messages: Vec<Message>,
    cache_boundaries: Vec<u32>,
}

/// Materializes every media part through the adapter, including media nested in
/// a tool result. Deterministic by contract so the cached prefix does not move
/// (FR-RECON-4).
async fn materialize_message(
    adapter: &Arc<dyn MediaAdapter>,
    blobs: &dyn BlobStore,
    mut message: Message,
) -> Result<Message> {
    let mut parts = Vec::with_capacity(message.parts.len());
    for part in message.parts {
        match part {
            Part::Media(media) => {
                let media = adapter.resolve(media, blobs).await?;
                parts.push(Part::Media(media));
            }
            Part::ToolResult(mut result) => {
                let mut inner = Vec::with_capacity(result.parts.len());
                for part in result.parts {
                    match part {
                        Part::Media(media) => {
                            let media = adapter.resolve(media, blobs).await?;
                            inner.push(Part::Media(media));
                        }
                        other => inner.push(other),
                    }
                }
                result.parts = inner;
                parts.push(Part::ToolResult(result));
            }
            other => parts.push(other),
        }
    }
    message.parts = parts;
    Ok(message)
}

/// The effective request options a session carries from a previous turn or an
/// import (FR-EXP-7). A missing or unreadable value means "no session defaults".
fn session_turn_options(session: &Session) -> TurnOptions {
    session
        .metadata
        .get(TURN_OPTIONS_KEY)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// Session metadata with the effective request options of the current turn
/// (FR-EXP-7), preserving every host key.
fn turn_metadata(session: &Session, effective: &TurnOptions) -> BTreeMap<String, Value> {
    let mut metadata = session.metadata.clone();
    if let Ok(value) = serde_json::to_value(effective) {
        metadata.insert(TURN_OPTIONS_KEY.to_owned(), value);
    }
    metadata
}

/// An error result for every call of `message` that has none, in the order the
/// message lists them.
///
/// A cancelled turn commits the assistant message it streamed so far, and that
/// message may already carry tool calls. Leaving them unanswered breaks the
/// invariant every wire and every replay depends on: a `tool_call` has its
/// `tool_result`: and the store keeps the break forever: the next turn appends
/// below an open call, so the history is one no provider accepts, and a replay
/// across models cannot repair it (the adapter refuses to drop a token while a
/// result is pending). Answering them with a cancelled error keeps the graph
/// closed and the session movable.
fn unanswered_call_errors(message: &Message) -> Vec<ToolResultPart> {
    let answered: BTreeSet<CallId> = message
        .tool_results()
        .into_iter()
        .map(|result| result.call_id)
        .collect();
    message
        .tool_calls()
        .into_iter()
        .filter(|call| !answered.contains(&call.call_id))
        .map(|call| ToolResultPart::error(call.call_id, CALL_CANCELLED_REASON))
        .collect()
}

/// The reason a call answered by the engine itself carries.
///
/// A person cancelled the turn before the call ran, and the model is told so in
/// the words it reads everything else in: one text part, an error result.
const CALL_CANCELLED_REASON: &str = "tool call cancelled before it ran";

/// Deterministic cache boundary positions, as exclusive end indices into the
/// message sequence: after the leading System/Developer run and at the end of
/// the critical prefix. Never depends on the tail (FR-RECON-7).
fn cache_boundaries(messages: &[Message]) -> Vec<u32> {
    let Ok(len) = u32::try_from(messages.len()) else {
        return Vec::new();
    };
    if len == 0 {
        return Vec::new();
    }
    let mut boundaries = Vec::new();
    let prefix = messages
        .iter()
        .take_while(|message| matches!(message.role, Role::System | Role::Developer))
        .count();
    if let Ok(prefix) = u32::try_from(prefix)
        && prefix > 0
        && prefix < len
    {
        boundaries.push(prefix);
    }
    boundaries.push(len);
    boundaries
}

/// The pure state machine over a session. It holds configuration only; every
/// piece of durable state lives in the store. One transition produces exactly
/// one atomic commit.
#[derive(Clone)]
pub struct Harness {
    store: Arc<dyn Store>,
    blobs: Arc<dyn BlobStore>,
    model: Arc<dyn Model>,
    tools: Arc<ToolRegistry>,
    layers: Arc<LayerStack>,
    interceptors: Arc<InterceptorSet>,
    observers: Arc<ObserverSet>,
    config: Arc<HarnessConfig>,
    media: Option<Arc<dyn MediaPreprocessor>>,
    media_adapter: Option<Arc<dyn MediaAdapter>>,
}

impl Harness {
    /// Starts a fluent builder.
    #[must_use]
    pub fn builder() -> HarnessBuilder {
        HarnessBuilder::default()
    }

    /// The engine-level configuration.
    #[must_use]
    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }

    /// The registry the engine consults.
    #[must_use]
    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    /// Starts a turn with a new user message, using the inherited options.
    pub fn send(&self, session: SessionId, message: Message) -> turn::Turn {
        self.turn(session).input(message).start()
    }

    /// Continues a session from its durable phase, using the inherited options.
    pub fn resume(&self, session: SessionId) -> turn::Turn {
        self.turn(session).start()
    }

    /// Opens the fluent builder for one turn (FR-ENG-7, FR-ENG-8).
    #[must_use]
    pub fn turn(&self, session: SessionId) -> TurnBuilder<'_> {
        TurnBuilder::new(self, session)
    }

    /// The engine-level defaults a turn inherits (FR-ENG-7).
    fn default_turn_options(&self) -> TurnOptions {
        TurnOptions {
            params: self.config.params.clone(),
            reasoning: Some(self.config.reasoning),
            tools: None,
            extensions: BTreeMap::new(),
        }
    }

    /// The tool specs to expose for a turn: every registered tool when the turn
    /// does not restrict them, otherwise the named subset in the registry's
    /// canonical order (FR-ENG-8).
    fn effective_tools(&self, options: &TurnOptions) -> Result<Vec<ToolSpec>> {
        let Some(names) = &options.tools else {
            return Ok(self.tools.specs());
        };
        let selected: BTreeSet<&ToolName> = names.iter().collect();
        for name in &selected {
            if self.tools.spec(name).is_none() {
                return Err(Error::ToolNotFound((*name).clone()));
            }
        }
        Ok(self
            .tools
            .specs()
            .into_iter()
            .filter(|spec| selected.contains(&spec.name))
            .collect())
    }

    async fn commit(&self, commit: Commit, sender: &mpsc::Sender<EngineEvent>) -> Result<Session> {
        let session_id = commit.session_id;
        match self.store.commit(commit).await {
            Ok(session) => {
                self.emit(
                    sender,
                    EngineEvent::CommitSucceeded {
                        session: session_id,
                        version: session.version,
                    },
                )
                .await;
                Ok(session)
            }
            Err(
                error @ Error::StoreConflict {
                    expected, actual, ..
                },
            ) => {
                self.emit(
                    sender,
                    EngineEvent::CommitConflict {
                        session: session_id,
                        expected,
                        actual,
                    },
                )
                .await;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    /// Emits one stream delta after running it through the registered transforms
    /// (FR-OBS-2). Returns `false` when the durable channel closed, which the
    /// caller treats as an interruption.
    async fn emit_delta(
        &self,
        sender: &mpsc::Sender<EngineEvent>,
        session: &Session,
        delta: StreamDelta,
    ) -> bool {
        for delta in self.observers.project(delta) {
            let event = EngineEvent::StreamDelta {
                session: session.id,
                turn: session.turn,
                delta,
            };
            if !self.emit(sender, event).await {
                return false;
            }
        }
        true
    }

    async fn emit(&self, sender: &mpsc::Sender<EngineEvent>, event: EngineEvent) -> bool {
        self.observers.emit(&event);
        sender.send(event).await.is_ok()
    }
}

fn into_result(call: &ToolCallPart, output: ToolOutput) -> ToolResultPart {
    let mut parts = output.parts;
    if parts.is_empty()
        && let Some(structured) = &output.structured
    {
        parts.push(Part::text(structured.to_string()));
    }
    ToolResultPart {
        call_id: call.call_id,
        parts,
        structured: output.structured,
        is_error: output.is_error,
    }
}

/// The invariant context of one tool turn: the session, the event channel, the
/// assistant message carrying the calls, the model's capabilities and the
/// request history a policy reads (FR-POL-4). Grouped so the per-call
/// arguments of the tool loop stay few.
#[derive(Clone, Copy)]
struct ToolTurn<'a> {
    session: &'a Session,
    sender: &'a mpsc::Sender<EngineEvent>,
}

/// The session and turn a tool call belongs to. Grouped so it can be moved into
/// an early-execution task (FR-STREAM-4).
#[derive(Clone, Copy)]
struct CallSite {
    session: SessionId,
    turn: u64,
}

/// The sink a running tool reports its output through.
///
/// It writes on the engine's own channel, so a chunk reaches the host in the
/// order the tool produced it and beside the call's other events. A full or
/// gone channel drops the chunk rather than stalling the tool: the final
/// `ToolCallFinished` still carries the whole result.
struct CallProgress {
    sender: mpsc::Sender<EngineEvent>,
    session: SessionId,
    turn: u64,
    call_id: CallId,
    tool: ToolName,
}

impl ProgressSink for CallProgress {
    fn send(&self, progress: ToolProgress) {
        let event = EngineEvent::ToolOutput {
            session: self.session,
            turn: self.turn,
            call_id: self.call_id,
            tool: self.tool.clone(),
            stream: progress.stream,
            bytes: progress.bytes,
            last: progress.last,
        };
        if self.sender.try_send(event).is_err() {
            tracing::debug!(call = %self.call_id, "tool output dropped: channel full or closed");
        }
    }
}

/// One tool call, past policy: everything `execute_call` needs about it, so the
/// free function keeps a short signature (FR-TOOL-1).
struct PreparedCall {
    call: ToolCallPart,
    tool: ToolName,
    arguments: Value,
    metadata: BTreeMap<String, Value>,
}

/// Executes one prepared tool call and emits its lifecycle events (FR-TOOL-1).
async fn execute_call(
    tools: Arc<ToolRegistry>,
    observers: Arc<ObserverSet>,
    sender: mpsc::Sender<EngineEvent>,
    site: CallSite,
    prepared: PreparedCall,
    cancellation: CancellationToken,
) -> ToolResultPart {
    let PreparedCall {
        call,
        tool,
        arguments,
        metadata,
    } = prepared;
    let started = EngineEvent::ToolCallStarted {
        session: site.session,
        turn: site.turn,
        call_id: call.call_id,
        tool: tool.clone(),
    };
    observers.emit(&started);
    let _ = sender.send(started).await;

    // A tool that streams reports what it reads as it reads it, on the same
    // channel the call's own events travel on. The engine treats them as
    // ordinary events, so a host that ignores them loses only the live view.
    let progress = Arc::new(CallProgress {
        sender: sender.clone(),
        session: site.session,
        turn: site.turn,
        call_id: call.call_id,
        tool: tool.clone(),
    });
    let mut context = ToolContext::new()
        .with_session(site.session)
        .with_call(call.call_id)
        .with_cancellation(cancellation.clone())
        .with_progress(progress);
    for (key, value) in metadata {
        context = context.with_metadata(key, value);
    }
    let clock = Instant::now();
    // The started event may have waited for channel capacity after approval.
    let output = if cancellation.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        tools.execute(&context, &tool, arguments).await
    };
    let duration_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (result, is_error) = match output {
        Ok(output) => {
            let is_error = output.is_error;
            (into_result(&call, output), is_error)
        }
        Err(error) => (
            ToolResultPart {
                call_id: call.call_id,
                parts: vec![Part::text(error.to_string())],
                structured: None,
                is_error: true,
            },
            true,
        ),
    };
    let finished = EngineEvent::ToolCallFinished {
        session: site.session,
        turn: site.turn,
        call_id: call.call_id,
        tool,
        is_error,
        duration_ms,
    };
    observers.emit(&finished);
    let _ = sender.send(finished).await;
    result
}

fn stream_delta(event: &StreamEvent) -> Option<StreamDelta> {
    match event {
        StreamEvent::TextDelta { index, text } => Some(StreamDelta::Text {
            index: *index,
            text: text.clone(),
        }),
        StreamEvent::ReasoningDelta { index, text } => Some(StreamDelta::Reasoning {
            index: *index,
            text: text.clone(),
        }),
        StreamEvent::RefusalDelta { index, text } => Some(StreamDelta::Refusal {
            index: *index,
            text: text.clone(),
        }),
        StreamEvent::ToolCallDelta {
            index,
            call_id,
            arguments,
            ..
        } => Some(StreamDelta::ToolArguments {
            index: *index,
            call_id: *call_id,
            arguments: arguments.clone(),
        }),
        StreamEvent::MediaChunk { index, bytes, last } => Some(StreamDelta::Media {
            index: *index,
            bytes: bytes.len(),
            last: *last,
        }),
        StreamEvent::Continuity { index, fragment } => Some(StreamDelta::Continuity {
            index: *index,
            len: fragment.len(),
        }),
        StreamEvent::SafetyRatings { ratings } => Some(StreamDelta::SafetyRatings {
            ratings: ratings.clone(),
        }),
        StreamEvent::Logprobs { index, entries } => Some(StreamDelta::Logprobs {
            index: *index,
            entries: entries.clone(),
        }),
        _ => None,
    }
}

/// The digest of a request, over its canonical JSON text.
fn digest_of(request: &ModelRequest) -> Digest {
    Digest::of_json(&serde_json::to_value(request).unwrap_or(Value::Null))
}

/// The terminal that reports the request the model actually received.
///
/// A layer may alter the request before it reaches the model, so the digest
/// the generation records is the one of the wire, not of the reconstruction
/// (FR-ENG-4).
struct Digesting<'a> {
    model: &'a dyn Model,
    digest: &'a Mutex<Option<Digest>>,
}

#[async_trait::async_trait]
impl Continuation for Digesting<'_> {
    async fn call(&self, _context: &LayerContext, request: ModelRequest) -> Result<ModelStream> {
        *self.digest.lock().unwrap_or_else(PoisonError::into_inner) = Some(digest_of(&request));
        self.model.stream(request).await
    }
}

/// The agent-facing handle on a running turn: a typed event stream plus the
/// final outcome. Dropping it cancels the turn at the next event and commits
/// A running turn driven by a spawned task, with a streaming event channel.
/// Utility for tests and hosts: token accounting of a finished turn.
#[must_use]
pub fn usage_of(message: &Message) -> Usage {
    message
        .origin
        .as_ref()
        .map(|origin| origin.usage.clone())
        .unwrap_or_default()
}

pub use norte::blob::InMemoryBlobStore;

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
