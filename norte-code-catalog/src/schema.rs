//! The catalogue's JSON shape.
//!
//! The catalogue is two files a person owns: `providers.json` says who serves
//! models and over which wires, and `models.json` is a flat list of the models
//! themselves. They are split so a model can be added without touching a
//! provider, and every model carries its own `provider_id` so the list needs no
//! nesting to say who serves what.
//!
//! A file is written by hand or built by a plugin — one that reads models.dev,
//! one that reads an `OpenRouter` listing — and both cast to this schema. Nothing
//! here fetches: the crate is a shape and the reader of that shape, and where
//! the bytes came from is the caller's business.

use std::collections::BTreeMap;
use std::path::PathBuf;

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

/// A failure while reading the catalogue.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// A file is larger than this build accepts.
    #[error("{path} is {size} bytes, over the {max}-byte cap")]
    TooLarge {
        /// The file.
        path: PathBuf,
        /// The size found.
        size: usize,
        /// The cap.
        max: usize,
    },
    /// Neither `XDG_CONFIG_HOME` nor `HOME` is set, so there is nowhere to look.
    #[error("{message}")]
    NoDirectory {
        /// What is missing, as the path helpers word it.
        message: String,
    },
    /// A file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A file is not valid JSON, or does not match the shape.
    #[error("{path} is not a valid catalogue: {source}")]
    Parse {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: serde_json::Error,
    },
    /// No providers are configured.
    #[error(
        "{path} defines no providers; Norte Code needs one with a wire before it can talk to a model"
    )]
    NoProviders {
        /// The path.
        path: PathBuf,
    },
    /// No models are configured.
    #[error(
        "{path} defines no models; Norte Code needs one to open a session on (add an entry, or write the file with a plugin)"
    )]
    NoModels {
        /// The path.
        path: PathBuf,
    },
    /// A model names a provider that is not configured.
    #[error("the model `{model}` names the provider `{provider}`, which {path} does not define")]
    UnknownProvider {
        /// The model.
        model: String,
        /// The provider it names.
        provider: String,
        /// The providers file.
        path: PathBuf,
    },
    /// A model is served by a provider with no wire of the requested schema.
    #[error(
        "the provider `{provider}` serves no `{wanted}` wire; it offers {offered}. Name one with --wire"
    )]
    NoSuchWire {
        /// The provider.
        provider: String,
        /// The schema that was wanted.
        wanted: String,
        /// The schemas it does offer, as a list.
        offered: String,
    },
}

/// The catalogue: the providers and the models, read from their two files.
///
/// It is built from bytes and holds no path and no way to refresh itself. What
/// it is read from, when it is re-read and what to do when it is missing are
/// the caller's questions, and keeping them out is what lets a plugin build the
/// same type in memory from an endpoint of its own.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    providers: BTreeMap<String, Provider>,
    /// Every model, in the order the file listed them.
    models: Vec<Model>,
    /// Model key — `provider/id` — to its place in `models`.
    index: BTreeMap<String, usize>,
}

impl Catalog {
    /// Reads the two files a person owns.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] when either file is missing, unreadable, too
    /// large, not valid JSON, or carries nothing: a catalogue with no providers
    /// or no models cannot open a session, and failing here says which file to
    /// write rather than failing later on the first message.
    pub fn load(
        providers: &std::path::Path,
        models: &std::path::Path,
    ) -> Result<Self, CatalogError> {
        let provider_bytes = read_limited(providers)?;
        let model_bytes = read_limited(models)?;
        let providers: Providers = parse(providers, &provider_bytes)?;
        let models: Models = parse(models, &model_bytes)?;
        Self::new(providers, models)
    }

    /// Builds a catalogue in memory, for a plugin that read the data itself.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] for the same reasons [`Catalog::load`] does,
    /// except that nothing was read from disk: the two shapes must still name
    /// providers that exist and carry at least one of each.
    pub fn new(providers: Providers, models: Models) -> Result<Self, CatalogError> {
        let source = PathBuf::from("the catalogue");
        if providers.providers.is_empty() {
            return Err(CatalogError::NoProviders { path: source });
        }
        if models.models.is_empty() {
            return Err(CatalogError::NoModels { path: source });
        }
        let mut catalog = Self {
            providers: providers.providers,
            models: models.models,
            index: BTreeMap::new(),
        };
        catalog.normalize()?;
        Ok(catalog)
    }

    /// Fills in the names a file left empty and indexes the models.
    fn normalize(&mut self) -> Result<(), CatalogError> {
        for (id, provider) in &mut self.providers {
            if provider.name.trim().is_empty() {
                provider.name.clone_from(id);
            }
        }
        self.index.clear();
        for (position, model) in self.models.iter_mut().enumerate() {
            if !self.providers.contains_key(&model.provider_id) {
                return Err(CatalogError::UnknownProvider {
                    model: model.id.clone(),
                    provider: model.provider_id.clone(),
                    path: PathBuf::from("providers.json"),
                });
            }
            if model.name.trim().is_empty() {
                model.name.clone_from(&model.id);
            }
            if model.canonical_slug.trim().is_empty() {
                model.canonical_slug = model.key();
            }
            self.index.insert(model.key(), position);
        }
        Ok(())
    }

    /// Checks the numbers and drops implausible ones, returning what changed.
    ///
    /// A missing context window makes compaction refuse for that model instead
    /// of guessing. Zero is the catalogue's way of saying "not applicable" for
    /// image, audio and embedding models, so it is cleared silently; only a
    /// genuinely absurd number is worth a warning.
    pub fn validate(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        for model in &mut self.models {
            if let Some(context) = model.limit.context {
                if context == 0 {
                    model.limit.context = None;
                } else if context > MAX_CONTEXT {
                    warnings.push(format!(
                        "{}: ignoring an implausible context window of {context}",
                        model.key()
                    ));
                    model.limit.context = None;
                }
            }
            if let Some(output) = model.limit.output {
                if output == 0 {
                    model.limit.output = None;
                } else if output > MAX_OUTPUT {
                    warnings.push(format!(
                        "{}: ignoring an implausible output limit of {output}",
                        model.key()
                    ));
                    model.limit.output = None;
                }
            }
        }
        warnings
    }

    /// Every model, with the provider that serves it.
    pub fn models(&self) -> impl Iterator<Item = ModelEntry<'_>> {
        self.models.iter().filter_map(|model| {
            self.providers
                .get(&model.provider_id)
                .map(|provider| ModelEntry { provider, model })
        })
    }

    /// One model, by provider id and model id.
    #[must_use]
    pub fn find(&self, provider: &str, model: &str) -> Option<&Model> {
        self.index
            .get(&format!("{provider}/{model}"))
            .map(|position| &self.models[*position])
    }

    /// Every model a provider serves, in file order.
    pub fn models_of(&self, provider: &str) -> impl Iterator<Item = &Model> {
        self.models
            .iter()
            .filter(move |model| model.provider_id == provider)
    }

    /// The models whose id is exactly this, from any provider.
    ///
    /// Two providers may serve the same id, and an id alone does not say which
    /// is meant: the caller that has only a name gets every candidate and is
    /// the one that knows whether a choice is needed.
    #[must_use]
    pub fn find_by_id(&self, model: &str) -> Vec<&Model> {
        self.models
            .iter()
            .filter(|entry| entry.id == model)
            .collect()
    }

    /// One provider, by id.
    #[must_use]
    pub fn provider(&self, provider: &str) -> Option<&Provider> {
        self.providers.get(provider)
    }

    /// Every provider, by id.
    #[must_use]
    pub fn providers(&self) -> &BTreeMap<String, Provider> {
        &self.providers
    }

    /// Every model, in file order.
    #[must_use]
    pub fn all_models(&self) -> &[Model] {
        &self.models
    }

    /// How many models the catalogue carries.
    #[must_use]
    pub fn model_count(&self) -> usize {
        self.models.len()
    }
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

/// Reads the two catalogue files from where they belong.
///
/// It is the one place the paths and the reading meet, so the host and the
/// service cannot disagree about which file is which: both call this, and a
/// file that is missing is an error naming it rather than an empty catalogue.
///
/// # Errors
///
/// Returns [`CatalogError`] when the configuration directory cannot be located,
/// when either file is missing or unreadable, or when either is malformed.
pub fn load_catalog_files() -> Result<Catalog, CatalogError> {
    let providers =
        crate::source::providers_path().map_err(|message| CatalogError::NoDirectory { message })?;
    let models =
        crate::source::models_path().map_err(|message| CatalogError::NoDirectory { message })?;
    Catalog::load(&providers, &models)
}

/// Reads a file with a hard size cap, so a pathological file cannot be read
/// into memory before it is rejected.
fn read_limited(path: &std::path::Path) -> Result<Vec<u8>, CatalogError> {
    let metadata = std::fs::metadata(path).map_err(|source| CatalogError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_CATALOG_BYTES as u64 {
        return Err(CatalogError::TooLarge {
            path: path.to_path_buf(),
            size: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            max: MAX_CATALOG_BYTES,
        });
    }
    let bytes = std::fs::read(path).map_err(|source| CatalogError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(CatalogError::TooLarge {
            path: path.to_path_buf(),
            size: bytes.len(),
            max: MAX_CATALOG_BYTES,
        });
    }
    Ok(bytes)
}

/// Parses one of the two files, naming it in the error.
fn parse<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<T, CatalogError> {
    serde_json::from_slice(bytes).map_err(|source| CatalogError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider that serves one `OpenAI` wire, for a test that needs a shape
    /// and not a file.
    fn provider(name: &str) -> Provider {
        Provider {
            name: name.to_owned(),
            doc: None,
            environment_keys: vec![format!("{}_API_KEY", name.to_uppercase())],
            wires: vec![Wire {
                api: "https://example.invalid/v1".to_owned(),
                schema: WireSchema::OpenAI,
            }],
        }
    }

    fn model(id: &str, provider_id: &str) -> Model {
        Model {
            id: id.to_owned(),
            provider_id: provider_id.to_owned(),
            canonical_slug: format!("{provider_id}/{id}"),
            name: id.to_owned(),
            limit: Limit {
                context: Some(128_000),
                output: Some(8_192),
            },
            modalities: Modalities {
                input: vec![Modality::Text],
                output: vec![Modality::Text],
            },
            ..Model::default()
        }
    }

    fn catalog() -> Catalog {
        let mut providers = BTreeMap::new();
        providers.insert("openai".to_owned(), provider("openai"));
        providers.insert("anthropic".to_owned(), provider("anthropic"));
        Catalog::new(
            Providers { providers },
            Models {
                models: vec![model("gpt-4o", "openai"), model("claude-4-5", "anthropic")],
            },
        )
        .expect("catalogue")
    }

    #[test]
    fn a_flat_list_keeps_two_providers_apart() {
        // The reason the list is flat: the same name from two providers is two
        // models, and neither shadows the other.
        let catalog = catalog();
        assert_eq!(
            catalog.find("openai", "gpt-4o").expect("model").id,
            "gpt-4o"
        );
        assert_eq!(catalog.model_count(), 2);
        assert_eq!(catalog.models_of("openai").count(), 1);
        let same = catalog.find_by_id("gpt-4o");
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].provider_id, "openai");
    }

    #[test]
    fn a_model_naming_an_unknown_provider_is_rejected() {
        let mut providers = BTreeMap::new();
        providers.insert("openai".to_owned(), provider("openai"));
        let error = Catalog::new(
            Providers { providers },
            Models {
                models: vec![model("m", "nowhere")],
            },
        )
        .expect_err("unknown provider");
        assert!(
            matches!(error, CatalogError::UnknownProvider { .. }),
            "{error}"
        );
    }

    #[test]
    fn an_empty_catalogue_is_rejected() {
        let error = Catalog::new(Providers::default(), Models::default()).expect_err("empty");
        assert!(matches!(error, CatalogError::NoProviders { .. }), "{error}");
    }

    #[test]
    fn implausible_limits_are_dropped() {
        let mut cat = catalog();
        cat.models[0].limit.context = Some(MAX_CONTEXT + 1);
        let warnings = cat.validate();
        assert_eq!(warnings.len(), 1);
        assert!(
            cat.find("openai", "gpt-4o")
                .expect("model")
                .limit
                .context
                .is_none()
        );
    }

    #[test]
    fn the_preferred_wire_is_the_openai_one() {
        // The order a person gets when they named nothing.
        let mut entry = provider("many");
        entry.wires = vec![
            Wire {
                api: "https://example.invalid/google".to_owned(),
                schema: WireSchema::Google,
            },
            Wire {
                api: "https://example.invalid/anthropic".to_owned(),
                schema: WireSchema::Anthropic,
            },
            Wire {
                api: "https://example.invalid/openai".to_owned(),
                schema: WireSchema::OpenAI,
            },
        ];
        assert_eq!(
            entry.preferred_wire().expect("wire").schema,
            WireSchema::OpenAI
        );
        assert_eq!(
            entry.wire(WireSchema::Google).expect("google").api,
            "https://example.invalid/google"
        );
        assert!(entry.wire(WireSchema::Anthropic).is_some());
        assert_eq!(entry.offered(), "google, anthropic, openai");
    }

    #[test]
    fn a_slug_and_a_name_default_to_the_id() {
        let catalog = Catalog::new(
            Providers {
                providers: BTreeMap::from([("p".to_owned(), provider("p"))]),
            },
            Models {
                models: vec![Model {
                    id: "m".to_owned(),
                    provider_id: "p".to_owned(),
                    ..Model::default()
                }],
            },
        )
        .expect("catalogue");
        let model = catalog.find("p", "m").expect("model");
        assert_eq!(model.name, "m");
        assert_eq!(model.canonical_slug, "p/m");
        assert_eq!(model.key(), "p/m");
    }

    #[test]
    fn the_enums_read_the_names_a_file_spells() {
        // The names are the contract with whoever writes the file, so they are
        // read as they are written and not as Rust spells them.
        let models: Models = serde_json::from_str(
            r#"{"models":[{"id":"m","provider_id":"p","modalities":{"input":["text","pdf"]},
                "supported_params":["tools","temperature","structured_output"],
                "reasoning":{"supported_efforts":["low","xhigh"]},
                "status":"deprecated"}]}"#,
        )
        .expect("models");
        let model = &models.models[0];
        assert_eq!(model.modalities.input, vec![Modality::Text, Modality::File]);
        assert!(model.supported_params.contains(&SupportedParams::Tools));
        assert!(
            model
                .supported_params
                .contains(&SupportedParams::Temperature)
        );
        assert_eq!(
            model.reasoning.supported_efforts,
            vec![SupportedEffort::Low, SupportedEffort::Xhigh]
        );
        assert_eq!(model.status, Some(ModelStatus::Deprecated));
    }

    #[test]
    fn an_unknown_name_in_a_list_is_refused() {
        // The reason these are enums: a typo has to be caught where the file is
        // read, not silently become "the model does not support that".
        let error = serde_json::from_str::<Models>(
            r#"{"models":[{"id":"m","provider_id":"p","supported_params":["temperatur"]}]}"#,
        )
        .expect_err("typo");
        assert!(error.to_string().contains("temperatur"), "{error}");
    }
}
