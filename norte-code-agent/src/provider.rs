//! Provider construction: turning a catalogue entry into a Norte model.
//!
//! The catalogue is the only source of provider data: a provider's `wires` say
//! which API surfaces it serves and where each one lives, its
//! `environment_keys` name the variables that may hold the key, and the model
//! supplies the limits, modalities and reasoning it declares. Nothing here
//! hardcodes a vendor URL.
//!
//! The wire is chosen from the provider's own list: the schema a caller named,
//! or the preferred one when nobody said. The endpoint always comes from the
//! wire — a base URL a person owns — and every request goes through a
//! [`VendorProfile`], so there is one path for a vendor this build has never
//! heard of and one for the three everyone knows.

use std::collections::BTreeMap;
use std::sync::Arc;

use norte::anthropic::AnthropicMessages;
use norte::gemini::GeminiGenerate;
use norte::http::{AuthScheme, VendorProfile};
use norte::openai::OpenAiChat;
use norte::{Capabilities, Model, ModelId, ModelRef, ProviderId, WireFamily};

pub use norte_code_catalog::WireKind as Wire;
/// The catalogue's own wire: an API surface with its URL and schema.
pub use norte_code_catalog::schema::Wire as CatalogWire;
use norte_code_catalog::{
    Catalog, CatalogError, Model as CatalogModel, Provider as CatalogProvider, WireSchema,
};

use norte_code_config::{ApiKey, Config};

use crate::ServiceError;

/// Everything needed to build one model.
#[derive(Clone, Debug)]
pub struct ModelSpec {
    /// The catalogue's provider id, as in `openai`, `anthropic`, `google`,
    /// `openrouter` or `groq`.
    pub provider: String,
    /// The model id, as the vendor spells it.
    pub model: String,
    /// The wire the provider speaks.
    pub wire: Wire,
    /// The base URL the wire's requests go to, from the provider's own entry.
    pub endpoint: String,
    /// The API key. Its `Debug` output is redacted.
    pub api_key: ApiKey,
    /// The schema the provider serves for this model.
    ///
    /// It is kept beside the wire because it is what a person selected and what
    /// the UI shows; the wire is what the engine sends.
    pub schema: WireSchema,
    /// The capability declaration the catalogue entry and config produce.
    pub capabilities: Capabilities,
    /// The headers sent with every request to this provider.
    pub headers: BTreeMap<String, String>,
}

/// The headers Norte Code sends to `OpenRouter` so the app is attributed in
/// its rankings and analytics.
///
/// `HTTP-Referer` is the identifier `OpenRouter` gives the app, and the title
/// is the name it shows; without both there is no app page. The category is
/// the one `OpenRouter` lists for a terminal coding assistant.
fn openrouter_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "HTTP-Referer".to_owned(),
            "https://norte.vyde.dev".to_owned(),
        ),
        ("X-OpenRouter-Title".to_owned(), "Norte Code".to_owned()),
        (
            "X-OpenRouter-Categories".to_owned(),
            "cli-agent,cloud-agent".to_owned(),
        ),
    ])
}

impl ModelSpec {
    /// Builds a spec from a catalogue provider and one of its models.
    ///
    /// The provider supplies the wire, its base URL and the variables that may
    /// hold the key; the model supplies the limits, modalities, parameters and
    /// reasoning it declares. `schema` is the wire the caller named with
    /// `--wire`, and `None` takes the provider's preferred one.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the provider serves no wire of the named
    /// schema, when it serves none at all, or when no key can be resolved.
    pub fn from_entry(
        config: &Config,
        provider: &CatalogProvider,
        model: &CatalogModel,
        schema: Option<WireSchema>,
    ) -> Result<Self, ServiceError> {
        let wire = select_wire(provider, model, schema)?;
        let api_key = config.resolve_api_key(&model.provider_id, &provider.environment_keys)?;
        Ok(Self {
            provider: model.provider_id.clone(),
            model: model.id.clone(),
            wire: Wire::from(wire.schema),
            endpoint: wire.api.clone(),
            api_key,
            schema: wire.schema,
            capabilities: norte_code_catalog::capabilities(model, Wire::from(wire.schema)),
            headers: headers_for(&model.provider_id),
        })
    }

    /// Looks a `provider/model` reference up in the catalogue and builds it.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the reference is not `provider/model`, the
    /// provider or the model is not in the catalogue, or no key is available.
    pub fn from_catalog(
        config: &Config,
        catalog: &Catalog,
        reference: &str,
        schema: Option<WireSchema>,
    ) -> Result<Self, ServiceError> {
        let (provider_id, model_id) = reference
            .split_once('/')
            .ok_or_else(|| ServiceError::ModelReference(reference.to_owned()))?;
        let provider = catalog
            .provider(provider_id)
            .ok_or_else(|| ServiceError::UnknownProvider(provider_id.to_owned()))?;
        let model =
            catalog
                .find(provider_id, model_id)
                .ok_or_else(|| ServiceError::UnknownModel {
                    provider: provider_id.to_owned(),
                    model: model_id.to_owned(),
                })?;
        Self::from_entry(config, provider, model, schema)
    }

    /// The `provider/model` label.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// The wire a model is reached over.
///
/// A named schema resolves against the provider's list and a schema it does not
/// serve is an error that says what it does serve. With nothing named the
/// provider's preferred wire is taken, which is its `OpenAI` surface when it has
/// one. A provider with no wires at all has nowhere to send a request.
fn select_wire<'a>(
    provider: &'a CatalogProvider,
    model: &CatalogModel,
    schema: Option<WireSchema>,
) -> Result<&'a CatalogWire, ServiceError> {
    match schema {
        Some(schema) => provider.wire(schema).ok_or_else(|| {
            ServiceError::Catalog(CatalogError::NoSuchWire {
                provider: model.provider_id.clone(),
                wanted: schema.label().to_owned(),
                offered: provider.offered(),
            })
        }),
        None => provider.preferred_wire().ok_or_else(|| {
            ServiceError::Catalog(CatalogError::NoSuchWire {
                provider: model.provider_id.clone(),
                wanted: WireSchema::OpenAI.label().to_owned(),
                offered: "nothing".to_owned(),
            })
        }),
    }
}

/// The headers a provider's requests carry.
///
/// The built-in ones are for the one gateway that asks to be identified;
/// everybody else gets none.
fn headers_for(provider: &str) -> BTreeMap<String, String> {
    if provider == "openrouter" {
        openrouter_headers()
    } else {
        BTreeMap::new()
    }
}

/// Builds a Norte model from a [`ModelSpec`]. Tests implement this to inject a
/// scripted model.
pub trait ModelFactory: Send + Sync {
    /// Builds the model with the host's capability declaration.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the provider is unknown or its
    /// configuration is invalid.
    fn build(
        &self,
        spec: &ModelSpec,
        capabilities: &Capabilities,
    ) -> Result<Arc<dyn Model>, ServiceError>;
}

/// Builds every provider the catalogue names.
#[derive(Clone, Copy, Debug, Default)]
pub struct VendorModels;

impl ModelFactory for VendorModels {
    fn build(
        &self,
        spec: &ModelSpec,
        capabilities: &Capabilities,
    ) -> Result<Arc<dyn Model>, ServiceError> {
        let key = spec.api_key.expose().to_owned();
        let provider = ProviderId::new(spec.provider.clone())?;
        let model_id = ModelId::new(spec.model.clone())?;
        let reference = ModelRef::new(provider.clone(), model_id);
        // One path for every provider: the endpoint is the one its own entry
        // named, and the profile carries the credential header the schema
        // implies. There is no branch for a vendor this build has heard of,
        // because knowing the vendor is not what makes the request work — the
        // wire is.
        let profile = profile(spec.wire, provider, &spec.endpoint, key, &spec.headers)?;
        build_with_profile(spec.wire, reference, profile, capabilities.clone())
    }
}

/// Adds the headers a spec carries to a vendor profile.
fn with_headers(profile: VendorProfile, headers: &BTreeMap<String, String>) -> VendorProfile {
    let mut profile = profile;
    for (name, value) in headers {
        profile = profile.with_header(name.clone(), value.clone());
    }
    profile
}

/// Builds the vendor profile for a catalogue entry: wire family, base URL and
/// the credential header the wire expects.
fn profile(
    wire: Wire,
    provider: ProviderId,
    endpoint: &str,
    key: String,
    headers: &BTreeMap<String, String>,
) -> Result<VendorProfile, ServiceError> {
    let profile = VendorProfile::new(provider, WireFamily::new(wire.family())?, endpoint)?;
    // The credential header follows from the wire and nothing else: it is part
    // of what the schema *is*, so a configuration able to change it could only
    // make the request wrong.
    let profile = match wire {
        Wire::Chat => profile.with_auth(AuthScheme::bearer(key)),
        Wire::Messages => profile.with_auth(AuthScheme::header("x-api-key", key)),
        Wire::GenerateContent => profile.with_auth(AuthScheme::header("x-goog-api-key", key)),
    };
    let profile = if wire == Wire::Messages {
        profile.with_header("anthropic-version", "2023-06-01")
    } else {
        profile
    };
    Ok(with_headers(profile, headers))
}

/// Builds the adapter that speaks `wire` over `profile`.
fn build_with_profile(
    wire: Wire,
    reference: ModelRef,
    profile: VendorProfile,
    capabilities: Capabilities,
) -> Result<Arc<dyn Model>, ServiceError> {
    Ok(match wire {
        Wire::Chat => Arc::new(OpenAiChat::new(reference, profile, capabilities)?),
        Wire::Messages => Arc::new(AnthropicMessages::new(reference, profile, capabilities)?),
        Wire::GenerateContent => Arc::new(GeminiGenerate::new(reference, profile, capabilities)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider that serves one wire.
    fn provider(name: &str, schemas: &[WireSchema]) -> CatalogProvider {
        CatalogProvider {
            name: name.to_owned(),
            doc: None,
            environment_keys: Vec::new(),
            wires: schemas
                .iter()
                .map(|schema| CatalogWire {
                    api: format!("https://{name}.invalid/v1"),
                    schema: *schema,
                })
                .collect(),
        }
    }

    fn model(id: &str, provider_id: &str) -> CatalogModel {
        CatalogModel {
            id: id.to_owned(),
            provider_id: provider_id.to_owned(),
            canonical_slug: format!("{provider_id}/{id}"),
            name: id.to_owned(),
            modalities: norte_code_catalog::Modalities {
                input: vec![norte_code_catalog::Modality::Text],
                output: vec![norte_code_catalog::Modality::Text],
            },
            ..CatalogModel::default()
        }
    }

    #[test]
    fn openrouter_is_attributed() {
        // The one gateway that asks to be identified, so its requests carry the
        // app's page and its name. Every other provider gets nothing.
        let headers = headers_for("openrouter");
        assert_eq!(
            headers.get("HTTP-Referer").map(String::as_str),
            Some("https://norte.vyde.dev")
        );
        assert_eq!(
            headers.get("X-OpenRouter-Title").map(String::as_str),
            Some("Norte Code")
        );
        assert_eq!(
            headers.get("X-OpenRouter-Categories").map(String::as_str),
            Some("cli-agent,cloud-agent")
        );

        assert!(headers_for("openai").is_empty());
    }

    #[test]
    fn every_wire_builds_from_its_endpoint() {
        // One path for all three: the endpoint is the provider's own, and what
        // the adapter speaks is the schema it declared.
        let capabilities = Capabilities::default();
        for (name, schema) in [
            ("compatible", WireSchema::OpenAI),
            ("anthropic", WireSchema::Anthropic),
            ("google", WireSchema::Google),
        ] {
            let provider = provider(name, &[schema]);
            let model = model("m", name);
            let spec =
                ModelSpec::from_entry(&Config::default(), &provider, &model, None).expect("spec");
            assert_eq!(spec.schema, schema, "{name}");
            assert_eq!(spec.endpoint, format!("https://{name}.invalid/v1"));
            let built = VendorModels.build(&spec, &capabilities);
            assert!(built.is_ok(), "{name} did not build: {:?}", built.err());
        }
    }

    #[test]
    fn a_named_schema_is_honoured_and_an_absent_one_is_refused() {
        // A provider may serve several surfaces; naming one picks it, and
        // naming one it does not serve is an error that says what it has.
        let provider = provider("both", &[WireSchema::Anthropic, WireSchema::Google]);
        let model = model("m", "both");

        let google = ModelSpec::from_entry(
            &Config::default(),
            &provider,
            &model,
            Some(WireSchema::Google),
        )
        .expect("google");
        assert_eq!(google.schema, WireSchema::Google);
        assert_eq!(google.wire, Wire::GenerateContent);

        let error = ModelSpec::from_entry(
            &Config::default(),
            &provider,
            &model,
            Some(WireSchema::OpenAI),
        )
        .expect_err("no openai wire");
        let message = error.to_string();
        assert!(message.contains("openai"), "{message}");
        assert!(message.contains("anthropic"), "{message}");
    }

    #[test]
    fn the_preferred_wire_is_used_when_nobody_named_one() {
        // OpenAI first, then Anthropic, then Google: what a person gets when
        // they did not say, and what the picker describes.
        let provider = provider(
            "many",
            &[
                WireSchema::Google,
                WireSchema::Anthropic,
                WireSchema::OpenAI,
            ],
        );
        let model = model("m", "many");
        let spec =
            ModelSpec::from_entry(&Config::default(), &provider, &model, None).expect("spec");
        assert_eq!(spec.schema, WireSchema::OpenAI);
        assert_eq!(spec.wire, Wire::Chat);
    }

    #[test]
    fn a_provider_with_no_wire_cannot_be_reached() {
        let provider = provider("empty", &[]);
        let model = model("m", "empty");
        let error = ModelSpec::from_entry(&Config::default(), &provider, &model, None)
            .expect_err("no wire");
        assert!(error.to_string().contains("empty"), "{error}");
    }

    #[test]
    fn a_provider_that_names_no_variable_needs_no_key() {
        // A local endpoint is the case: there is nothing to look up, so nothing
        // is missing.
        let provider = provider("local", &[WireSchema::OpenAI]);
        let model = model("m", "local");
        let spec =
            ModelSpec::from_entry(&Config::default(), &provider, &model, None).expect("spec");
        assert_eq!(spec.api_key.expose(), "", "nothing to send");
    }

    #[test]
    fn the_label_is_the_reference_a_person_writes() {
        let provider = provider("p", &[WireSchema::OpenAI]);
        let model = model("m", "p");
        let spec =
            ModelSpec::from_entry(&Config::default(), &provider, &model, None).expect("spec");
        assert_eq!(spec.label(), "p/m");
    }
}
