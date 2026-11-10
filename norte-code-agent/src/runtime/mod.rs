#![forbid(unsafe_code)]
#![doc = "One reference agent harness over Norte's contracts: turns, the tool loop, approvals and streaming.

Norte's contracts live in [`norte`]: the [`Model`](norte::Model) port, the
[`Store`](norte::Store), the [`ToolRegistry`](norte::ToolRegistry), the
policies, the layers and the observers. This crate is one way to drive them:
[`Harness`] walks a session forward one turn at a time.

It is deliberately *a* harness and not *the* harness. Nothing in the core refers
to it, nothing forces a host to use it, and a host that wants a different turn
policy — a different loop, its own scheduling, a graph instead of a session —
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
    ProgressSink, ToolCallInfo, ToolContext, ToolMetadataProvider, ToolOutput, ToolProgress,
    ToolSpec,
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
/// invariant every wire and every replay depends on — a `tool_call` has its
/// `tool_result` — and the store keeps the break forever: the next turn appends
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
/// Fluent builder for an engine.
///
/// Three ports are required — a [`Store`], a [`BlobStore`] and a [`Model`] — and
/// [`build`](HarnessBuilder::build) fails with [`Error::InvalidInput`] naming the
/// one that is missing. Everything else has a default: an empty tool registry,
/// no policies, no layers, no observers and [`HarnessConfig::default`].
///
/// The ports are built once, at the edge of the program, from configuration; the
/// rest of the application sees [`Harness`] and never a vendor. A knob left unset
/// keeps its default behaviour, so a builder lists only what the host decided.
///
/// # Example
///
/// ```ignore
///
/// use norte::{Capabilities, Message, Modality, ToolRegistry};
/// /// use norte_openai::presets::openai;
///
/// let capabilities = Capabilities::builder()
///     .input(Modality::Text)
///     .output(Modality::Text)
///     .build();
///
/// // Built once, at the edge of the program; nothing above the `Model` port
/// // learns which vendor answered.
/// let key = std::env::var("OPENAI_API_KEY").expect("key");
/// let model = Arc::new(openai("gpt-5", key, capabilities).expect("model"));
///
/// let engine = HarnessBuilder::new()
///     .store(Arc::new(InMemoryStore::new()))
///     .blobs(Arc::new(InMemoryBlobStore::new()))
///     .model(model)
///     .tools(ToolRegistry::new())
///     .build()
///     .expect("engine");
///
/// let session = engine.create().await.expect("session");
///
/// // The outcome carries the turn's parts, its usage and why it stopped.
/// let outcome = engine
///     .send(session.id, Message::user("Hello!"))
///     .finish()
///     .await
///     .expect("turn");
/// ```
#[derive(Default)]
pub struct HarnessBuilder {
    store: Option<Arc<dyn Store>>,
    blobs: Option<Arc<dyn BlobStore>>,
    model: Option<Arc<dyn Model>>,
    tools: ToolRegistry,
    layers: LayerStack,
    interceptors: Vec<Arc<dyn TurnInterceptor>>,
    observers: Vec<Arc<dyn Observer>>,
    transforms: Vec<Arc<dyn StreamTransform>>,
    config: HarnessConfig,
    media: Option<Arc<dyn MediaPreprocessor>>,
    media_adapter: Option<Arc<dyn MediaAdapter>>,
}

/// Fluent builder for a single turn (FR-ENG-7, FR-ENG-8).
///
/// It borrows the engine; [`start`](TurnBuilder::start) hands back an owned,
/// already-running [`Turn`]. The borrow ends there. Every knob is optional and
/// overlays the inherited defaults per field: what a turn sets replaces that one
/// field of the effective [`GenerationParams`] and nothing else, so a turn that
/// only changes the temperature keeps the engine's other parameters.
///
/// # Example
///
/// ```ignore
/// use futures_util::StreamExt;
/// use norte::{Message, ToolName};
///
/// let mut turn = engine
///     .turn(session.id)
///     .input(Message::user("Summarize src/lib.rs"))
///     .temperature(0.2)
///     .max_output_tokens(512)
///     .tools([ToolName::new("read_file").expect("tool")])
///     .start();
/// while let Some(event) = turn.next().await {
///     // Handle each live event.
/// }
/// let outcome = turn.finish().await.expect("turn");
/// ```
pub struct TurnBuilder<'a> {
    engine: &'a Harness,
    session: SessionId,
    input: Option<Message>,
    options: TurnOptions,
    cancellation: Option<CancellationToken>,
}

impl<'a> TurnBuilder<'a> {
    fn new(engine: &'a Harness, session: SessionId) -> Self {
        Self {
            engine,
            session,
            input: None,
            options: TurnOptions::default(),
            cancellation: None,
        }
    }

    /// Sets the user message that starts the turn. Without it the turn resumes
    /// the session from its durable phase.
    #[must_use]
    pub fn input(mut self, message: Message) -> Self {
        self.input = Some(message);
        self
    }

    /// Supplies the token that cancels this turn.
    ///
    /// A host that hears a cancel on another task hands its token here and
    /// signals it without holding the turn. The default is a fresh token.
    #[must_use]
    pub fn cancellation(mut self, token: CancellationToken) -> Self {
        self.cancellation = Some(token);
        self
    }

    /// Applies a whole set of overrides at once. A per-field knob called after
    /// this one wins.
    #[must_use]
    pub fn options(mut self, options: TurnOptions) -> Self {
        self.options = options;
        self
    }

    /// Overrides the generation-parameter delta (merged per field, FR-ENG-7).
    #[must_use]
    pub fn params(mut self, params: GenerationParams) -> Self {
        self.options.params = params;
        self
    }

    /// Sets the sampling temperature for this turn.
    #[must_use]
    pub fn temperature(mut self, value: f32) -> Self {
        self.options.params.temperature = Some(value);
        self
    }

    /// Sets the nucleus-sampling mass: the most probable tokens whose cumulative
    /// probability reaches this value are the ones sampled from.
    #[must_use]
    pub fn top_p(mut self, value: f32) -> Self {
        self.options.params.top_p = Some(value);
        self
    }

    /// Keeps only the `value` most probable tokens at each step.
    #[must_use]
    pub fn top_k(mut self, value: u32) -> Self {
        self.options.params.top_k = Some(value);
        self
    }

    /// Discards the tail of the distribution by its mass (`min-p`).
    ///
    /// A vendor knob: the model must accept it, and native `OpenAI` does not.
    #[must_use]
    pub fn min_p(mut self, value: f32) -> Self {
        self.options.params.min_p = Some(value);
        self
    }

    /// Truncates by the ratio to the most likely token (`top-a`).
    ///
    /// `OpenRouter` and compatible gateways; native `OpenAI` rejects it.
    #[must_use]
    pub fn top_a(mut self, value: f32) -> Self {
        self.options.params.top_a = Some(value);
        self
    }

    /// Penalises tokens already produced, by how often they appeared.
    #[must_use]
    pub fn frequency_penalty(mut self, value: f32) -> Self {
        self.options.params.frequency_penalty = Some(value);
        self
    }

    /// Penalises tokens that already appeared in the prompt or answer.
    #[must_use]
    pub fn presence_penalty(mut self, value: f32) -> Self {
        self.options.params.presence_penalty = Some(value);
        self
    }

    /// Penalises repeated tokens across both streams (`repetition_penalty`).
    ///
    /// A vendor knob; native `OpenAI` uses the frequency/presence penalties.
    #[must_use]
    pub fn repetition_penalty(mut self, value: f32) -> Self {
        self.options.params.repetition_penalty = Some(value);
        self
    }

    /// Biases the probability of specific tokens before sampling.
    ///
    /// Keyed by the vendor's token id as a string, in the `OpenAI` shape
    /// (`{"50256": -100}`).
    #[must_use]
    pub fn logit_bias(mut self, bias: BTreeMap<String, f32>) -> Self {
        self.options.params.logit_bias = Some(bias);
        self
    }

    /// Caps how many tokens the model may produce in this turn.
    #[must_use]
    pub fn max_output_tokens(mut self, value: u64) -> Self {
        self.options.params.max_output_tokens = Some(value);
        self
    }

    /// Stops generation when the model produces any of these sequences. The
    /// sequences themselves are not part of the answer.
    #[must_use]
    pub fn stop_sequences(mut self, values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.options.params.stop_sequences = values.into_iter().map(Into::into).collect();
        self
    }

    /// Fixes the sampling seed, where the vendor honours it.
    ///
    /// The same seed and parameters promise the same sampling, not the same
    /// answer: the model's own state and the provider's version still count.
    #[must_use]
    pub fn seed(mut self, value: u64) -> Self {
        self.options.params.seed = Some(value);
        self
    }

    /// Asks for structured output against this JSON Schema.
    #[must_use]
    pub fn output_schema(mut self, schema: Value) -> Self {
        self.options.params.output_schema = Some(schema);
        self
    }

    /// Requests token-level log probabilities for this turn: `top` runner-up
    /// tokens per position, or `0` for the chosen token's probability alone.
    /// The model must declare
    /// [`Feature::Logprobs`](norte::Feature::Logprobs); the engine rejects the
    /// turn before the request otherwise (FR-LP-1).
    #[must_use]
    pub fn logprobs(mut self, top: u8) -> Self {
        self.options.params.logprobs = Some(top);
        self
    }

    /// Overrides the reasoning ask for this turn.
    #[must_use]
    pub fn reasoning(mut self, request: ReasoningRequest) -> Self {
        self.options.reasoning = Some(request);
        self
    }
    /// Restricts the tools exposed to the model for this turn (FR-ENG-8). Names
    /// must exist in the registry; the subset is emitted in canonical order.
    #[must_use]
    pub fn tools(mut self, names: impl IntoIterator<Item = ToolName>) -> Self {
        self.options.tools = Some(names.into_iter().collect());
        self
    }

    /// Sets a vendor request extra for this turn, keyed `<wire-family>:<field>`
    /// (Section 12), for example `google.gemini:safetySettings`. A key whose
    /// family does not match the active wire is ignored.
    #[must_use]
    pub fn extension(mut self, key: impl Into<String>, value: Value) -> Self {
        self.options.extensions.insert(key.into(), value);
        self
    }

    /// Spawns the turn and returns its event stream.
    pub fn start(self) -> turn::Turn {
        let cancellation = self.cancellation.unwrap_or_default();
        turn::Turn::new(
            self.engine.clone(),
            self.session,
            self.input,
            self.options,
            cancellation,
        )
    }
}

impl HarnessBuilder {
    /// An empty builder: no store, no blob store, no model, and every optional
    /// piece at its default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the session store: sessions, nodes, messages and approval records.
    ///
    /// Required. [`InMemoryStore`](crate::runtime::InMemoryStore) is the reference
    /// implementation; a host whose sessions outlive the process implements
    /// [`Store`] over its own database.
    #[must_use]
    pub fn store(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// Sets the blob store that holds media bytes and large tool output by
    /// content hash.
    ///
    /// Required. [`InMemoryBlobStore`](crate::runtime::InMemoryBlobStore) is the
    /// reference implementation; the bytes live where the host puts them.
    #[must_use]
    pub fn blobs(mut self, blobs: Arc<dyn BlobStore>) -> Self {
        self.blobs = Some(blobs);
        self
    }

    /// Sets the model that answers.
    ///
    /// Required. One `Arc<dyn Model>` is built at the edge of the program and
    /// everything above the port stays independent of the vendor.
    #[must_use]
    pub fn model(mut self, model: Arc<dyn Model>) -> Self {
        self.model = Some(model);
        self
    }

    /// Sets the tool registry the turn draws declarations from.
    ///
    /// Empty by default: a turn that registers no tool is a plain chat.
    #[must_use]
    pub fn tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    /// Sets the layer stack that wraps each model call.
    ///
    /// Empty by default.
    #[must_use]
    pub fn layers(mut self, layers: LayerStack) -> Self {
        self.layers = layers;
        self
    }

    /// Sets the interceptors the engine consults before every model request of
    /// a turn (FR-ENG-10).
    ///
    /// Empty by default. An interceptor may return messages to append to the
    /// session's history; the engine appends them itself, so they are durable
    /// and visible to the policies.
    #[must_use]
    pub fn interceptors(mut self, interceptors: Vec<Arc<dyn TurnInterceptor>>) -> Self {
        self.interceptors = interceptors;
        self
    }

    /// Sets the observers that see every [`EngineEvent`].
    ///
    /// Empty by default. A slow observer never stalls a turn: the set buffers
    /// events and drops what does not fit, and
    /// [`ObserverSet::dropped`](norte::ObserverSet::dropped) counts the drops.
    #[must_use]
    pub fn observers(mut self, observers: Vec<Arc<dyn Observer>>) -> Self {
        self.observers = observers;
        self
    }

    /// Registers the stream transforms that project deltas for observers
    /// (FR-OBS-2). Their names are recorded in the session metadata (FR-OBS-3).
    #[must_use]
    pub fn transforms(mut self, transforms: Vec<Arc<dyn StreamTransform>>) -> Self {
        self.transforms = transforms;
        self
    }

    /// Adds one stream transform.
    #[must_use]
    pub fn transform(mut self, transform: Arc<dyn StreamTransform>) -> Self {
        self.transforms.push(transform);
        self
    }

    /// Sets the engine-level configuration. Fields left at their default keep
    /// their default behaviour.
    #[must_use]
    pub fn config(mut self, config: HarnessConfig) -> Self {
        self.config = config;
        self
    }

    /// Installs the host's per-call tool metadata provider (FR-TOOL-7).
    ///
    /// The callback runs immediately before each tool execution, including one
    /// that resumes a parked call, and its values land in
    /// [`ToolContext::metadata`]. Nothing is persisted.
    #[must_use]
    pub fn tool_metadata(
        mut self,
        provider: impl Fn(&ToolCallInfo) -> BTreeMap<String, Value> + Send + Sync + 'static,
    ) -> Self {
        self.config.tool_metadata = Some(ToolMetadataProvider::new(provider));
        self
    }

    /// Sets an optional media preprocessing step applied to inline input media
    /// before a model call.
    #[must_use]
    pub fn media_preprocessor(mut self, preprocessor: Arc<dyn MediaPreprocessor>) -> Self {
        self.media = Some(preprocessor);
        self
    }

    /// Sets the media materialization port used during reconstruction and on
    /// model output.
    #[must_use]
    pub fn media_adapter(mut self, adapter: Arc<dyn MediaAdapter>) -> Self {
        self.media_adapter = Some(adapter);
        self
    }

    /// Validates the three required ports and returns the engine.
    ///
    /// Everything optional is folded in here: the registry, the policy set and
    /// the layer stack are shared behind an `Arc`, and the observers and
    /// transforms become one [`ObserverSet`] bounded by
    /// [`HarnessConfig::limits`](HarnessConfig::limits).
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] naming the port that is missing.
    pub fn build(self) -> Result<Harness> {
        let store = self
            .store
            .ok_or_else(|| Error::InvalidInput("engine requires a store".to_owned()))?;
        let blobs = self
            .blobs
            .ok_or_else(|| Error::InvalidInput("engine requires a blob store".to_owned()))?;
        let model = self
            .model
            .ok_or_else(|| Error::InvalidInput("engine requires a model".to_owned()))?;
        let buffer = self.config.limits.max_buffered_events;
        Ok(Harness {
            store,
            blobs,
            model,
            tools: Arc::new(self.tools),
            layers: Arc::new(self.layers),
            interceptors: Arc::new(InterceptorSet::new(self.interceptors)),
            observers: Arc::new(ObserverSet::with_transforms(
                self.observers,
                self.transforms,
                buffer,
            )),
            config: Arc::new(self.config),
            media: self.media,
            media_adapter: self.media_adapter,
        })
    }
}

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
mod tests {
    use super::*;
    use norte::tool::ToolStream;

    /// A tool that reports output while it runs, then returns a result.
    #[norte::tool(name = "streaming", description = "Reports two chunks, then returns.")]
    #[derive(Clone)]
    struct Streaming;

    #[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct StreamingArgs {}

    #[async_trait::async_trait]
    impl norte::FnExecutor<StreamingArgs, ToolOutput> for Streaming {
        async fn call(&self, _args: StreamingArgs) -> Result<ToolOutput> {
            Ok(ToolOutput::text("done"))
        }

        async fn call_with_context(
            &self,
            context: &ToolContext,
            _args: StreamingArgs,
        ) -> Result<ToolOutput> {
            for chunk in [b"first".to_vec(), b"second".to_vec()] {
                context.emit(ToolProgress {
                    stream: ToolStream::Stdout,
                    bytes: chunk,
                    last: false,
                });
            }
            context.emit(ToolProgress {
                stream: ToolStream::Stdout,
                bytes: Vec::new(),
                last: true,
            });
            Ok(ToolOutput::text("done"))
        }
    }

    /// A tool sees a progress sink, and what it reports arrives as the engine's
    /// own events, in order, between the call's lifecycle events.
    #[tokio::test]
    async fn a_streaming_tool_reports_through_the_engine() {
        let name = ToolName::new("streaming").expect("tool name");
        let mut registry = ToolRegistry::new();
        registry.register_tool(&Streaming).expect("register");
        let tools = Arc::new(registry);

        let (sender, mut events) = mpsc::channel(32);
        let site = CallSite {
            session: SessionId::new(),
            turn: 1,
        };
        let prepared = PreparedCall {
            call: ToolCallPart {
                call_id: CallId::new(),
                name: name.as_str().to_owned(),
                arguments: serde_json::json!({}),
                continuity: None,
            },
            tool: name,
            arguments: serde_json::json!({}),
            metadata: BTreeMap::new(),
        };

        let result = execute_call(
            tools,
            Arc::new(ObserverSet::new(Vec::new(), 16)),
            sender,
            site,
            prepared,
            CancellationToken::new(),
        )
        .await;
        assert!(!result.is_error, "the tool succeeded");

        let mut kinds = Vec::new();
        let mut chunks = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                EngineEvent::ToolOutput { bytes, last, .. } => {
                    kinds.push("output");
                    chunks.push((bytes, last));
                }
                EngineEvent::ToolCallStarted { .. } => kinds.push("started"),
                EngineEvent::ToolCallFinished { .. } => kinds.push("finished"),
                _ => {}
            }
        }
        assert_eq!(
            kinds,
            ["started", "output", "output", "output", "finished"],
            "the chunks arrive between the call's own events"
        );
        assert_eq!(chunks[0], (b"first".to_vec(), false));
        assert_eq!(chunks[1], (b"second".to_vec(), false));
        assert!(chunks[2].1, "the last chunk closes the stream");
    }
}
