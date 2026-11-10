//! The fluent builders for an engine and for one turn.

use std::collections::BTreeMap;
use std::sync::Arc;

use norte::id::ToolName;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use norte::BlobStore;
use norte::capability::ReasoningRequest;
use norte::domain::message::Message;
use norte::error::{Error, Result};
use norte::id::SessionId;
use norte::layer::LayerStack;
use norte::media_adapter::MediaAdapter;
use norte::model::{GenerationParams, Model, TurnOptions};
use norte::preprocess::MediaPreprocessor;
use norte::tool::registry::ToolRegistry;
use norte::tool::{ToolCallInfo, ToolMetadataProvider};

use crate::runtime::interceptor::{InterceptorSet, TurnInterceptor};
use crate::runtime::observer::{Observer, ObserverSet, StreamTransform};
use crate::runtime::turn::Turn;
use crate::runtime::{Harness, HarnessConfig, Store};

/// Fluent builder for an engine.
///
/// Three ports are required: a [`Store`], a [`BlobStore`] and a [`Model`]: and
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
    pub(crate) fn new(engine: &'a Harness, session: SessionId) -> Self {
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
    pub fn start(self) -> Turn {
        let cancellation = self.cancellation.unwrap_or_default();
        Turn::new(
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
