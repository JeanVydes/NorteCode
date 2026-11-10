//! The catalogue's JSON shape.
//!
//! `providers.json` says who serves models and over which wires, and
//! `models.json` is a flat list of the models themselves. They are the shapes a
//! person edits, a plugin builds and every client reads, so they live here:
//! the server's loader adds validation and file access on top, and a client can
//! name a wire, a limit or an effort without bringing the server along.
//!
//! Nothing here fetches or validates beyond shape: a name the enum does not
//! know is a decode error, which is the point of an enum over a string.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// Reads a list, dropping null elements and tolerating a null list.
///
/// It is generic over the element so the enums use it too: a modality list and
/// an environment-key list are the same shape, and the difference between them
/// is the type the elements deserialize into. An element the type does not know
/// is still an error — which is the whole reason these are enums, and a typo in
/// a file has to be caught where the file is read.
fn list_or_empty<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let values = Option::<Vec<Option<T>>>::deserialize(deserializer)?;
    Ok(values.unwrap_or_default().into_iter().flatten().collect())
}

/// The largest catalogue file this build will parse.
pub const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;

/// The largest context window a sane model can claim.
pub const MAX_CONTEXT: u64 = 100_000_000;

/// The largest output limit a sane model can claim.
pub const MAX_OUTPUT: u64 = 10_000_000;

/// `providers.json`: who serves models, and over which wires.
///
/// Keyed by provider id, which is what a model's `provider_id` names. A
/// provider has a list of wires and not one, because the same vendor may serve
/// the same models over more than one API surface — an OpenAI-compatible
/// endpoint beside a native one — and which is in play is the wire's own
/// `schema`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Providers {
    /// Provider id to provider.
    pub providers: BTreeMap<String, Provider>,
}

/// One provider.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Provider {
    /// The display name, as a person reads it.
    pub name: String,
    /// A documentation URL.
    pub doc: Option<String>,
    /// The environment variables that may hold this provider's key, in the
    /// order they are tried.
    ///
    /// It is a list because a vendor ships several: an OpenAI-compatible
    /// gateway reads `OPENROUTER_API_KEY` and often `OPENAI_API_KEY` as the
    /// fallback a person already has exported.
    #[serde(deserialize_with = "list_or_empty")]
    pub environment_keys: Vec<String>,
    /// The API surfaces this provider serves.
    pub wires: Vec<Wire>,
}

/// One API surface of a provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    /// The full base URL requests go to, such as `https://api.openai.com/v1`.
    pub api: String,
    /// The shape of the requests and answers this surface speaks.
    pub schema: WireSchema,
    /// The credential header this surface expects, when it is not the one the
    /// schema implies.
    ///
    /// `None` is the common case and means the header follows from the schema,
    /// which is what a vendor that speaks its own shape expects. A gateway that
    /// serves another vendor's shape over its own credential — an Anthropic
    /// Messages surface reached with `Authorization: Bearer` — names one here,
    /// because the schema says how the body is shaped and not who is paying.
    pub auth: Option<AuthKind>,
}

/// The credential a wire's requests carry.
///
/// It is a closed set and it is deliberately small: these are the headers the
/// ecosystem converges on, and a vendor with a header of its own is a vendor
/// this host does not reach over a compatible wire. Naming one is override and
/// not default — a wire that says nothing carries what its schema implies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `x-api-key: <key>`.
    XApiKey,
    /// `x-goog-api-key: <key>`.
    XGoogApiKey,
    /// Send no credential, for a local or otherwise open endpoint.
    Disabled,
}

impl AuthKind {
    /// The header this kind sets, as a name a person reads.
    ///
    /// `Disabled` names none, and a message that had to print it says so rather
    /// than showing an empty header.
    #[must_use]
    pub fn header(self) -> Option<&'static str> {
        match self {
            Self::Bearer => Some("Authorization"),
            Self::XApiKey => Some("x-api-key"),
            Self::XGoogApiKey => Some("x-goog-api-key"),
            Self::Disabled => None,
        }
    }

    /// The kind a schema implies when a wire names none.
    #[must_use]
    pub fn for_schema(schema: WireSchema) -> Self {
        match schema {
            WireSchema::OpenAI => Self::Bearer,
            WireSchema::Anthropic => Self::XApiKey,
            WireSchema::Google => Self::XGoogApiKey,
        }
    }
}

/// The request/answer shape a wire speaks.
///
/// It is the interface and not the vendor: the three are what the ecosystem
/// converges on, and every provider serves one of them. Which Norte wire family
/// is used is derived from this — the crate of Norte that implements each one is
/// the authority on the family's name, and this enum only says which applies.
///
/// `OpenAI` is the chat completions surface, the one every compatible vendor
/// speaks. The Responses wire exists in Norte and is deliberately not reachable
/// from here: it is another shape entirely, and this host speaks the one the
/// three schemas share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireSchema {
    /// `OpenAI` chat completions, and every compatible vendor.
    ///
    /// The name is written out rather than derived: `rename_all` turns `OpenAI`
    /// into `open_a_i`, which is not what anybody types in a file.
    #[serde(rename = "openai")]
    OpenAI,
    /// Anthropic Messages.
    #[serde(rename = "anthropic")]
    Anthropic,
    /// Google `generateContent`.
    #[serde(rename = "google")]
    Google,
}

impl WireSchema {
    /// Where two wires rank when both are available.
    ///
    /// `OpenAI` first, because it is the surface the largest number of
    /// providers serve and the dialect the others borrow; then Anthropic, then
    /// Google. A person who cares which is used names it with `--wire`, and
    /// this is only what happens when nobody said.
    #[must_use]
    fn rank(self) -> u8 {
        match self {
            Self::OpenAI => 0,
            Self::Anthropic => 1,
            Self::Google => 2,
        }
    }

    /// The name a person reads in a message.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
            Self::Google => "google",
        }
    }

    /// Every schema, in preference order.
    pub const ALL: [Self; 3] = [Self::OpenAI, Self::Anthropic, Self::Google];
}

impl std::fmt::Display for WireSchema {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// `models.json`: every model, with the provider that serves it.
///
/// A flat list and not a map under each provider: a model is added by appending
/// to it, and two providers may serve the same model id without either
/// shadowing the other. `id` is what the vendor expects in a request and
/// `provider_id` is who to send it to, so the pair is the identity.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Models {
    /// The models, in the order the file lists them.
    pub models: Vec<Model>,
}

/// One model entry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    /// The model id the provider expects, as in `gpt-4o` or
    /// `anthropic/claude-sonnet-4`.
    pub id: String,
    /// The provider that serves it, naming an entry of `providers.json`.
    pub provider_id: String,
    /// The canonical slug, as in `openai/gpt-4o`.
    ///
    /// It is what a person writes when the vendor's own id is a path of its own
    /// or differs between gateways, and it is the name the UI shows.
    pub canonical_slug: String,
    /// The display name.
    pub name: String,
    /// A one-line description.
    pub description: Option<String>,
    /// The model family, as in `claude` or `gemini`.
    pub family: Option<String>,
    /// The reasoning controls the provider documents.
    pub reasoning: Reasoning,
    /// The knowledge cutoff.
    pub knowledge: Option<String>,
    /// The release date.
    pub release_date: Option<String>,
    /// The last update date.
    pub last_updated: Option<String>,
    /// Input and output modalities.
    pub modalities: Modalities,
    /// Whether the weights are open.
    pub open_weights: bool,
    /// Context and output limits.
    pub limit: Limit,
    /// Where the model is in its life: `deprecated`, `beta`, or absent for a
    /// model in general use.
    pub status: Option<ModelStatus>,
    /// The request fields the model accepts.
    ///
    /// It replaces a boolean per field: a list says what is accepted and grows
    /// without a new field each time a provider invents one, and a reader that
    /// asks for what it needs does not have to know every name in it.
    pub supported_params: Vec<SupportedParams>,
    /// What the provider charges, when it publishes it.
    ///
    /// It is kept for reference and is never a source of cost: Norte Code
    /// reports what the provider said a request cost and derives nothing from a
    /// list price, so a model whose entry has no pricing is not a model whose
    /// requests go uncounted.
    pub pricing: Option<Pricing>,
}

/// Where a model is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    /// Still served, and not recommended for new work.
    Deprecated,
    /// Offered before general availability.
    Beta,
}

impl std::fmt::Display for ModelStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Deprecated => "deprecated",
            Self::Beta => "beta",
        })
    }
}

/// Per-token pricing, in USD, as the provider publishes it.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pricing {
    /// USD per input token.
    pub input: Option<f64>,
    /// USD per output token.
    pub output: Option<f64>,
    /// USD per token read from the provider's prompt cache.
    pub cached: Option<f64>,
}

/// The reasoning controls the provider documents.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reasoning {
    /// Whether the model always reasons.
    ///
    /// A model that must think cannot be asked not to, so this is not a default
    /// but a fact about the model: the setting is what it is, and the answer
    /// reasons whatever the request said.
    pub mandatory: bool,
    /// Whether reasoning is on unless a request turns it off.
    pub default_enabled: bool,
    /// The effort the provider uses when a request names none.
    pub default_effort: Option<SupportedEffort>,
    /// The efforts the provider accepts.
    pub supported_efforts: Vec<SupportedEffort>,
    /// Whether the provider takes a token budget instead of, or beside, an
    /// effort.
    ///
    /// The file may name it `budget_supported` or `supports_budget`: both were
    /// written by hand, and a name that silently meant nothing turned a model
    /// that takes a budget into one that reasons never.
    #[serde(alias = "supports_budget")]
    pub budget_supported: bool,
}

/// Every request field a provider may accept.
///
/// It is a closed set and it is deliberately not exhaustive of the ecosystem:
/// it holds the fields this host can set, and a name a reader does not know is
/// not in the list. That is the point of an enum over a list of strings — a
/// typo in `models.json` is caught where the file is read instead of silently
/// meaning "not supported".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportedParams {
    /// `frequency_penalty`.
    FrequencyPenalty,
    /// The reasoning text is asked for and returned.
    IncludeReasoning,
    /// Token-level log probabilities.
    Logprobs,
    /// `max_tokens` and its per-vendor spellings.
    MaxTokens,
    /// `min_p`.
    MinP,
    /// `presence_penalty`.
    PresencePenalty,
    /// A reasoning block, as the dialect spells it.
    Reasoning,
    /// A reasoning effort.
    ReasoningEffort,
    /// `response_format`.
    ResponseFormat,
    /// `seed`.
    Seed,
    /// `stop`.
    Stop,
    /// A JSON schema the provider enforces.
    StructuredOutput,
    /// `temperature`.
    Temperature,
    /// `tool_choice`.
    ToolChoice,
    /// `tools`.
    Tools,
    /// `top_p`.
    TopP,
    /// `top_logprobs`.
    TopLogprobs,
    /// `top_k`.
    TopK,
    /// `top_a`.
    TopA,
    /// `logit_bias`.
    LogitBias,
    /// `repetition_penalty`.
    RepetitionPenalty,
    /// `verbosity`.
    Verbosity,
}

/// Input and output modalities.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Modalities {
    /// Accepted inputs.
    #[serde(deserialize_with = "list_or_empty")]
    pub input: Vec<Modality>,
    /// Produced outputs.
    #[serde(deserialize_with = "list_or_empty")]
    pub output: Vec<Modality>,
}

/// One kind of content a model accepts or produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// Plain text.
    Text,
    /// An image.
    Image,
    /// Recorded audio.
    Audio,
    /// Recorded video.
    Video,
    /// A document, which is what a PDF is.
    #[serde(alias = "pdf")]
    File,
}

/// Context and output token limits.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Limit {
    /// The context window.
    pub context: Option<u64>,
    /// The output limit.
    pub output: Option<u64>,
}

/// A reasoning effort a provider accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportedEffort {
    /// Reasoning off, where the provider spells it as an effort.
    None,
    /// The least effort.
    Minimal,
    /// A low effort.
    Low,
    /// A medium effort.
    Medium,
    /// A high effort.
    High,
    /// Above high, where a provider offers it.
    Xhigh,
    /// The greatest effort.
    Max,
}

/// A model together with its provider, for listing.
#[derive(Clone, Copy, Debug)]
pub struct ModelEntry<'a> {
    /// The provider.
    pub provider: &'a Provider,
    /// The model.
    pub model: &'a Model,
}

impl Model {
    /// The pair that identifies a model: who serves it, and what it is called.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}/{}", self.provider_id, self.id)
    }
}

impl Provider {
    /// The wire to use for this provider when nothing named one.
    ///
    /// The preferred schema wins: `OpenAI` first, then Anthropic, then Google,
    /// because the first is the surface the largest number of providers serve.
    /// A provider with no wire at all has nothing to send a request to, and the
    /// caller is where that is an error.
    #[must_use]
    pub fn preferred_wire(&self) -> Option<&Wire> {
        self.wires.iter().min_by_key(|wire| wire.schema.rank())
    }

    /// The wire of a named schema, when this provider serves one.
    #[must_use]
    pub fn wire(&self, schema: WireSchema) -> Option<&Wire> {
        self.wires.iter().find(|wire| wire.schema == schema)
    }

    /// The schemas this provider serves, for an error that says what it offers.
    #[must_use]
    pub fn schemas(&self) -> Vec<WireSchema> {
        self.wires.iter().map(|wire| wire.schema).collect()
    }

    /// The schemas as a list a message can print.
    #[must_use]
    pub fn offered(&self) -> String {
        if self.wires.is_empty() {
            return "nothing".to_owned();
        }
        self.schemas()
            .iter()
            .map(|schema| schema.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}
