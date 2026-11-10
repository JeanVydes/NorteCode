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
//! wire: a base URL a person owns: and every request goes through a
//! [`VendorProfile`], so there is one path for a vendor this build has never
//! heard of and one for the three everyone knows.

use std::collections::BTreeMap;
use std::sync::Arc;

use norte::anthropic::AnthropicMessages;
use norte::gemini::GeminiGenerate;
use norte::http::{AuthScheme, VendorProfile};
use norte::openai::OpenAiChat;
use norte::{Capabilities, Model, ModelId, ModelRef, ProviderId, WireFamily};

use norte_code_protocol::catalog::{Catalog, CatalogError};
/// A `providers.json` entry's wire: a base URL and the schema it speaks.
pub use norte_code_wire::catalog::Wire as CatalogWire;
use norte_code_wire::catalog::{
    AuthKind, Model as CatalogModel, Provider as CatalogProvider, WireSchema,
};

use norte_code_protocol::config::{ApiKey, Config};

use crate::ServiceError;

/// Everything needed to build one model.
#[derive(Clone, Debug)]
pub struct ModelSpec {
    /// The catalogue's provider id, as in `openai`, `anthropic`, `google`,
    /// `openrouter` or `groq`.
    pub provider: String,
    /// The model id, as the vendor spells it.
    pub model: String,
    /// The base URL the wire's requests go to, from the provider's own entry.
    pub endpoint: String,
    /// The API key. Its `Debug` output is redacted.
    pub api_key: ApiKey,
    /// The schema the provider serves for this model.
    ///
    /// It is what the wire *is*: which adapter speaks it and whether the prompt
    /// cache is explicit follow from it, and so does the credential header -
    /// unless the wire named one of its own, which is `auth`.
    pub schema: WireSchema,
    /// The credential header, when the wire named one its schema does not
    /// imply.
    ///
    /// A gateway that serves another vendor's shape over its own credential -
    /// an Anthropic Messages surface reached with `Authorization: Bearer` -
    /// says so on its wire, and the header here is that. `None` is the common
    /// case: the header follows from the schema.
    pub auth: Option<AuthKind>,
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
            endpoint: wire.api.clone(),
            api_key,
            schema: wire.schema,
            auth: wire.auth,
            capabilities: norte_code_protocol::catalog::capabilities(model, wire.schema),
            headers: headers_for(&model.provider_id),
        })
    }

    /// Looks a provider and a model up in the catalogue and builds it.
    ///
    /// The two arrive apart: a provider is its own key and a model id may carry
    /// slashes of its own, so the pair is never carried as one string that has
    /// to be taken apart again.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the provider or the model is not in the
    /// catalogue, or no key is available.
    pub fn from_catalog(
        config: &Config,
        catalog: &Catalog,
        provider_id: &str,
        model_id: &str,
        schema: Option<WireSchema>,
    ) -> Result<Self, ServiceError> {
        if catalog.providers().is_empty() {
            // The catalogue files are a person's, not a download: an empty
            // one is a setup step that has not been done. A catalogue plugin
            // may still have fed the service an in-memory catalogue, in which
            // case this branch is unreachable: the service tries that first.
            return Err(ServiceError::Unsupported(
                "no provider is configured. Add one to providers.json (or install a \
                 catalogue plugin that offers one), then re-read the catalogue.",
            ));
        }
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
        if spec.provider.trim().is_empty() || spec.model.trim().is_empty() {
            return Err(ServiceError::Unsupported("no model is configured"));
        }
        let provider = ProviderId::new(spec.provider.clone())?;
        let model_id = ModelId::new(spec.model.clone())?;
        let reference = ModelRef::new(provider.clone(), model_id);
        // One path for every provider: the endpoint is the one its own entry
        // named, and the profile carries the credential header the schema
        // implies. There is no branch for a vendor this build has heard of,
        // because knowing the vendor is not what makes the request work: the
        // wire is.
        let profile = profile(
            spec.schema,
            spec.auth,
            provider,
            &spec.endpoint,
            key,
            &spec.headers,
        )?;
        build_with_profile(spec.schema, reference, profile, capabilities.clone())
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
///
/// The header follows from the schema, which is what a vendor that speaks its
/// own shape expects; a wire that named one of its own overrides it, because a
/// gateway may serve another vendor's body over its own credential. The wire is
/// the only place that may say so: the configuration cannot, and a value it
/// could set would only make the request wrong.
fn profile(
    schema: WireSchema,
    auth: Option<AuthKind>,
    provider: ProviderId,
    endpoint: &str,
    key: String,
    headers: &BTreeMap<String, String>,
) -> Result<VendorProfile, ServiceError> {
    let profile = VendorProfile::new(
        provider,
        WireFamily::new(norte_code_protocol::catalog::map::WireSchemaExt::family(
            schema,
        ))?,
        endpoint,
    )?;
    let profile = match auth.unwrap_or_else(|| AuthKind::for_schema(schema)) {
        AuthKind::Bearer => profile.with_auth(AuthScheme::bearer(key)),
        AuthKind::XApiKey => profile.with_auth(AuthScheme::header("x-api-key", key)),
        AuthKind::XGoogApiKey => profile.with_auth(AuthScheme::header("x-goog-api-key", key)),
        AuthKind::Disabled => profile,
    };
    let profile = if schema == WireSchema::Anthropic {
        profile.with_header("anthropic-version", "2023-06-01")
    } else {
        profile
    };
    Ok(with_headers(profile, headers))
}

/// Builds the adapter that speaks `schema` over `profile`.
fn build_with_profile(
    schema: WireSchema,
    reference: ModelRef,
    profile: VendorProfile,
    capabilities: Capabilities,
) -> Result<Arc<dyn Model>, ServiceError> {
    Ok(match schema {
        WireSchema::OpenAI => Arc::new(OpenAiChat::new(reference, profile, capabilities)?),
        WireSchema::Anthropic => {
            Arc::new(AnthropicMessages::new(reference, profile, capabilities)?)
        }
        WireSchema::Google => Arc::new(GeminiGenerate::new(reference, profile, capabilities)?),
    })
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
