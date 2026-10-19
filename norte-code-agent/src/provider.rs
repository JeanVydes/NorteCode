//! Provider construction: turning a catalogue entry into a Norte model.
//!
//! The models.dev catalogue is the source of provider data: its `npm` package
//! selects the wire, its `api` URL is the endpoint, and its `env` list names the
//! environment variables that hold the key. Nothing here hardcodes a vendor
//! URL, and every `OpenAI`-compatible provider takes the same path as
//! `OpenRouter`.
//!
//! Native `OpenAI`, Anthropic and Google use Norte's presets when the catalogue
//! carries no URL; every other provider uses its catalogue `api` URL through a
//! [`VendorProfile`].

use std::sync::Arc;

use norte::anthropic::AnthropicMessages;
use norte::gemini::GeminiGenerate;
use norte::http::{AuthScheme, VendorProfile};
use norte::openai::{OpenAiChat, OpenAiResponses};
use norte::{
    Capabilities, Modality, Model, ModelId, ModelRef, ProviderId, StreamingGranularity, ToolKind,
    WireFamily,
};

pub use norte_code_catalog::Wire;
use norte_code_catalog::{Catalog, Model as CatalogModel, Provider as CatalogProvider};
use norte_code_config::{ApiKey, Config, ModelOverride};

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
    /// The provider's API URL, from the catalogue.
    pub endpoint: Option<String>,
    /// The API key. Its `Debug` output is redacted.
    pub api_key: ApiKey,
    /// The capability declaration the catalogue entry and config produce.
    pub capabilities: Capabilities,
}

impl ModelSpec {
    /// Builds a spec from a catalogue provider and model entry.
    ///
    /// The provider entry supplies the wire (`npm`), the endpoint (`api`) and
    /// the key variables (`env`); the model entry supplies the limits,
    /// modalities and reasoning options. Config wins for the key, the endpoint
    /// and the model overrides.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when no key can be resolved.
    pub fn from_entry(
        config: &Config,
        provider: &CatalogProvider,
        model: &CatalogModel,
        endpoint_override: Option<String>,
    ) -> Result<Self, ServiceError> {
        let api_key = config.resolve_api_key(&provider.id, &provider.env)?;
        let configured = config
            .providers
            .get(&provider.id)
            .and_then(|entry| entry.endpoint.clone());
        let wire = Wire::from_npm(provider.npm.as_deref().unwrap_or_default());
        let endpoint = endpoint_override
            .or(configured)
            .or(expand_endpoint(provider)?);
        let overrides = config
            .model_overrides
            .get(&format!("{}/{}", provider.id, model.id));
        let capabilities = norte_code_catalog::capabilities(model, wire, overrides);
        Ok(Self {
            provider: provider.id.clone(),
            model: model.id.clone(),
            wire,
            endpoint,
            api_key,
            capabilities,
        })
    }

    /// Looks a `provider/model` reference up in the catalogue and builds its
    /// spec.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the reference is not `provider/model`,
    /// the provider or model is not in the catalogue, or no key is available.
    pub fn from_catalog(
        config: &Config,
        catalog: &Catalog,
        reference: &str,
        endpoint_override: Option<String>,
    ) -> Result<Self, ServiceError> {
        let (provider_id, model_id) = reference
            .split_once('/')
            .ok_or_else(|| ServiceError::ModelReference(reference.to_owned()))?;
        let provider = catalog
            .provider(provider_id)
            .ok_or_else(|| ServiceError::UnknownProvider(provider_id.to_owned()))?;
        let model = provider
            .models
            .get(model_id)
            .ok_or_else(|| ServiceError::UnknownModel {
                provider: provider_id.to_owned(),
                model: model_id.to_owned(),
            })?;
        Self::from_entry(config, provider, model, endpoint_override)
    }

    /// Builds a spec for a model the catalogue does not list but whose provider
    /// it does, such as a fresh release. The provider still supplies the wire,
    /// the endpoint and the key variables; the capabilities are conservative.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the reference is not `provider/model`,
    /// the provider is not in the catalogue, or no key can be resolved.
    pub fn from_provider(
        config: &Config,
        catalog: &Catalog,
        reference: &str,
        endpoint_override: Option<String>,
    ) -> Result<Self, ServiceError> {
        let (provider_id, model_id) = reference
            .split_once('/')
            .ok_or_else(|| ServiceError::ModelReference(reference.to_owned()))?;
        let provider = catalog
            .provider(provider_id)
            .ok_or_else(|| ServiceError::UnknownProvider(provider_id.to_owned()))?;
        let api_key = config.resolve_api_key(&provider.id, &provider.env)?;
        let configured = config
            .providers
            .get(&provider.id)
            .and_then(|entry| entry.endpoint.clone());
        let wire = Wire::from_npm(provider.npm.as_deref().unwrap_or_default());
        let endpoint = endpoint_override
            .or(configured)
            .or(expand_endpoint(provider)?);
        let overrides = config.model_overrides.get(reference);
        Ok(Self {
            provider: provider.id.clone(),
            model: model_id.to_owned(),
            wire,
            endpoint,
            api_key,
            capabilities: fallback_capabilities(overrides),
        })
    }

    /// Splits a `provider/model` reference, resolves its key and endpoint, and
    /// picks a wire without the catalogue.
    ///
    /// This is the fallback for a provider the catalogue does not list. Without
    /// a catalogue `api` URL, only the three native providers (or a configured
    /// endpoint) can be built.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the reference is not `provider/model` or
    /// when no key can be resolved.
    pub fn resolve(
        config: &Config,
        reference: &str,
        endpoint: Option<String>,
        default_env: &[String],
    ) -> Result<Self, ServiceError> {
        let (provider, model) = reference
            .split_once('/')
            .ok_or_else(|| ServiceError::ModelReference(reference.to_owned()))?;
        let api_key = config.resolve_api_key(provider, default_env)?;
        let configured = config
            .providers
            .get(provider)
            .and_then(|entry| entry.endpoint.clone());
        let wire = Wire::for_provider(provider);
        let overrides = config.model_overrides.get(reference);
        Ok(Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
            wire,
            endpoint: endpoint.or(configured),
            api_key,
            capabilities: fallback_capabilities(overrides),
        })
    }

    /// The `provider/model` label.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}/{}", self.provider, self.model)
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
        let capabilities = capabilities.clone();
        let provider = ProviderId::new(spec.provider.clone())?;
        let model_id = ModelId::new(spec.model.clone())?;
        let reference = ModelRef::new(provider.clone(), model_id);

        let model: Arc<dyn Model> = match (spec.wire, spec.endpoint.as_deref()) {
            // No endpoint override: the native presets carry the vendor URL.
            (Wire::Responses, None) if spec.provider == "openai" => {
                Arc::new(norte::presets::openai(&spec.model, key, capabilities)?)
            }
            (Wire::Messages, None) if spec.provider == "anthropic" => {
                Arc::new(norte::presets::anthropic(&spec.model, key, capabilities)?)
            }
            (Wire::GenerateContent, None)
                if matches!(spec.provider.as_str(), "google" | "gemini") =>
            {
                Arc::new(norte::presets::gemini(&spec.model, key, capabilities)?)
            }
            // Everything else comes from the catalogue's `api` URL.
            (wire, Some(endpoint)) => {
                let profile = profile(wire, provider, endpoint, key)?;
                build_with_profile(wire, reference, profile, capabilities)?
            }
            (wire, None) => {
                return Err(ServiceError::MissingEndpoint {
                    provider: spec.provider.clone(),
                    wire,
                });
            }
        };
        Ok(model)
    }
}

/// Builds the vendor profile for a catalogue entry: wire family, base URL and
/// the credential header the wire expects.
fn profile(
    wire: Wire,
    provider: ProviderId,
    endpoint: &str,
    key: String,
) -> Result<VendorProfile, ServiceError> {
    let profile = VendorProfile::new(provider, WireFamily::new(wire.family())?, endpoint)?;
    Ok(match wire {
        Wire::Responses | Wire::Chat => profile.with_auth(AuthScheme::bearer(key)),
        Wire::Messages => profile
            .with_auth(AuthScheme::header("x-api-key", key))
            .with_header("anthropic-version", "2023-06-01"),
        Wire::GenerateContent => profile.with_auth(AuthScheme::header("x-goog-api-key", key)),
    })
}

/// Builds the adapter that speaks `wire` over `profile`.
fn build_with_profile(
    wire: Wire,
    reference: ModelRef,
    profile: VendorProfile,
    capabilities: Capabilities,
) -> Result<Arc<dyn Model>, ServiceError> {
    Ok(match wire {
        Wire::Responses => Arc::new(OpenAiResponses::new(reference, profile, capabilities)?),
        Wire::Chat => Arc::new(OpenAiChat::new(reference, profile, capabilities)?),
        Wire::Messages => Arc::new(AnthropicMessages::new(reference, profile, capabilities)?),
        Wire::GenerateContent => Arc::new(GeminiGenerate::new(reference, profile, capabilities)?),
    })
}

/// Expands the `${VAR}` placeholders some catalogue endpoints use, such as
/// Snowflake's account host and Cloudflare's account id.
fn expand_endpoint(provider: &CatalogProvider) -> Result<Option<String>, ServiceError> {
    let Some(template) = &provider.api else {
        return Ok(None);
    };
    let mut endpoint = String::with_capacity(template.len());
    let mut rest = template.as_str();
    while let Some(start) = rest.find("${") {
        endpoint.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(ServiceError::EndpointTemplate {
                provider: provider.id.clone(),
                template: template.clone(),
            });
        };
        let name = &after[..end];
        let value = std::env::var(name).map_err(|_| ServiceError::MissingEndpointVariable {
            provider: provider.id.clone(),
            variable: name.to_owned(),
        })?;
        endpoint.push_str(&value);
        rest = &after[end + 1..];
    }
    endpoint.push_str(rest);
    Ok(Some(endpoint))
}

/// The conservative capability declaration for a model the catalogue does not
/// list: text in, text out, streamed deltas and function tools.
fn fallback_capabilities(overrides: Option<&ModelOverride>) -> Capabilities {
    let mut builder = Capabilities::builder()
        .input(Modality::Text)
        .output(Modality::Text)
        .streaming(StreamingGranularity::Delta)
        .tool_kind(ToolKind::Function);
    if let Some(overrides) = overrides {
        if let Some(max_context) = overrides.max_context {
            builder = builder.max_context(max_context);
        }
        if let Some(max_output) = overrides.max_output {
            builder = builder.max_output(max_output);
        }
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(provider: &str, wire: Wire, endpoint: Option<&str>) -> ModelSpec {
        ModelSpec {
            provider: provider.to_owned(),
            model: "test-model".to_owned(),
            wire,
            endpoint: endpoint.map(str::to_owned),
            api_key: ApiKey::new("test-key"),
            capabilities: fallback_capabilities(None),
        }
    }

    #[test]
    fn every_wire_builds_from_an_endpoint() {
        let capabilities = fallback_capabilities(None);
        for (provider, wire, endpoint) in [
            ("lilac", Wire::Chat, "https://api.getlilac.com/v1"),
            ("openai", Wire::Responses, "https://api.openai.com/v1"),
            ("anthropic", Wire::Messages, "https://api.anthropic.com/v1"),
            (
                "google",
                Wire::GenerateContent,
                "https://generativelanguage.googleapis.com/v1beta",
            ),
        ] {
            let built = VendorModels.build(&spec(provider, wire, Some(endpoint)), &capabilities);
            assert!(built.is_ok(), "{provider} did not build: {:?}", built.err());
        }
    }

    #[test]
    fn a_compatible_provider_without_an_endpoint_is_refused() {
        let capabilities = fallback_capabilities(None);
        let error = VendorModels
            .build(&spec("lilac", Wire::Chat, None), &capabilities)
            .err()
            .expect("an error");
        assert!(matches!(error, ServiceError::MissingEndpoint { .. }));
    }

    #[test]
    fn a_catalogue_entry_supplies_wire_endpoint_key_and_limits() {
        let catalog = Catalog::parse(
            br#"{
                "providers": {
                    "lilac": {
                        "id": "lilac",
                        "env": ["LILAC_API_KEY"],
                        "npm": "@ai-sdk/openai-compatible",
                        "api": "https://api.getlilac.com/v1",
                        "models": {
                            "m": {
                                "id": "m",
                                "name": "M",
                                "tool_call": true,
                                "modalities": {"input": ["text", "image"], "output": ["text"]},
                                "limit": {"context": 128000, "output": 8192}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("catalog");
        let key_file =
            std::env::temp_dir().join(format!("norte-code-provider-{}-key", std::process::id()));
        std::fs::write(&key_file, "sk-test\n").expect("key file");
        let config = Config::from_json(&format!(
            r#"{{"providers":{{"lilac":{{"api_key_file":"{}"}}}}}}"#,
            key_file.display()
        ))
        .expect("config")
        .config;
        let provider = catalog.provider("lilac").expect("provider");
        let model = catalog.find("lilac", "m").expect("model");
        let spec = ModelSpec::from_entry(&config, provider, model, None).expect("spec");
        assert_eq!(spec.wire, Wire::Chat);
        assert_eq!(
            spec.endpoint.as_deref(),
            Some("https://api.getlilac.com/v1")
        );
        assert_eq!(spec.api_key.expose(), "sk-test");
        assert_eq!(spec.capabilities.max_context, Some(128_000));
        assert_eq!(spec.capabilities.max_output, Some(8192));
        assert!(spec.capabilities.tool_kinds.contains(&ToolKind::Function));
    }
}
