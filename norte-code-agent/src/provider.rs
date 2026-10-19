//! Provider construction: turning a catalogue entry into a Norte model.
//!
//! The models.dev catalogue is the source of provider data: its `npm` package
//! selects the wire, its `api` URL is the endpoint, and its `env` list names the
//! environment variables that hold the key. Nothing here hardcodes a vendor
//! URL, and every `OpenAI`-compatible provider takes the same path as
//! `OpenRouter`.
//!
//! The pre-catalogue fallback, [`Wire::for_provider`], exists only until the
//! catalogue (milestone 2) supplies the wire directly.

use std::sync::Arc;

use norte::anthropic::AnthropicMessages;
use norte::gemini::GeminiGenerate;
use norte::http::{AuthScheme, VendorProfile};
use norte::openai::{OpenAiChat, OpenAiResponses};
use norte::{
    Capabilities, Modality, Model, ModelId, ModelRef, ProviderId, StreamingGranularity, ToolKind,
    WireFamily,
};

use norte_code_config::{ApiKey, Config, ModelOverride};

use crate::ServiceError;

/// The wire protocol a provider speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    /// `OpenAI` Responses (`@ai-sdk/openai`).
    Responses,
    /// `OpenAI` chat completions, the `OpenAI`-compatible wire.
    Chat,
    /// Anthropic Messages (`@ai-sdk/anthropic`).
    Messages,
    /// Google `generateContent` (`@ai-sdk/google`).
    GenerateContent,
}

impl Wire {
    /// Maps a models.dev `npm` package to a wire.
    ///
    /// The catalogue spells the package, not the wire: `openai-compatible`
    /// means the chat wire, `anthropic` the Messages wire, `google` the
    /// `generateContent` wire and `@ai-sdk/openai` the Responses wire.
    /// Everything else is assumed OpenAI-compatible, which is what most of the
    /// catalogue's 193 `openai-compatible` entries are.
    #[must_use]
    pub fn from_npm(npm: &str) -> Self {
        if npm.contains("openai-compatible") {
            Self::Chat
        } else if npm.contains("anthropic") {
            Self::Messages
        } else if npm.contains("google") {
            Self::GenerateContent
        } else if npm == "@ai-sdk/openai" {
            Self::Responses
        } else {
            Self::Chat
        }
    }

    /// The pre-catalogue fallback, for the four providers the spec names.
    ///
    /// The catalogue replaces this as soon as it lands.
    #[must_use]
    pub fn for_provider(provider: &str) -> Self {
        match provider {
            "openai" => Self::Responses,
            "anthropic" => Self::Messages,
            "google" | "gemini" => Self::GenerateContent,
            _ => Self::Chat,
        }
    }

    /// The Norte wire family string.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when Norte rejects the value.
    pub fn family(self) -> Result<WireFamily, ServiceError> {
        let name = match self {
            Self::Responses => norte::openai::WIRE_RESPONSES,
            Self::Chat => norte::openai::WIRE_CHAT,
            Self::Messages => norte::anthropic::WIRE_MESSAGES,
            Self::GenerateContent => norte::gemini::WIRE_GENERATE,
        };
        Ok(WireFamily::new(name)?)
    }
}

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
}

impl ModelSpec {
    /// Builds a spec from a models.dev provider entry: `npm` selects the wire,
    /// `api` is the endpoint and `env` names the key variables. This is the
    /// path the catalogue uses; no vendor is special-cased.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the reference is not `provider/model`,
    /// when no key can be resolved, or when the endpoint is not a valid URL.
    pub fn from_catalog(
        config: &Config,
        reference: &str,
        npm: &str,
        api: Option<&str>,
        env: &[String],
        endpoint_override: Option<String>,
    ) -> Result<Self, ServiceError> {
        let (provider, model) = reference
            .split_once('/')
            .ok_or_else(|| ServiceError::ModelReference(reference.to_owned()))?;
        let api_key = config.resolve_api_key(provider, env)?;
        let configured = config
            .providers
            .get(provider)
            .and_then(|entry| entry.endpoint.clone());
        Ok(Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
            wire: Wire::from_npm(npm),
            endpoint: endpoint_override
                .or(configured)
                .or_else(|| api.map(str::to_owned)),
            api_key,
        })
    }

    /// Splits a `provider/model` reference, resolves its key and endpoint, and
    /// picks a wire.
    ///
    /// `endpoint` is a command-line override and wins over the provider's
    /// configured endpoint. `default_env` is the catalogue's `env` list; the
    /// config's own `api_key_env` and `api_key_file` win over it.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the reference is not `provider/model`,
    /// when no key can be resolved, or when the endpoint is not a valid URL.
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
        Ok(Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
            wire: Wire::for_provider(provider),
            endpoint: endpoint.or(configured),
            api_key,
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
    let profile = VendorProfile::new(provider, wire.family()?, endpoint)?;
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

/// The conservative capability declaration Norte Code starts from: text in,
/// text out, streamed deltas and function tools. The catalogue (milestone 2)
/// and `model_overrides` refine it.
#[must_use]
pub fn capabilities(overrides: Option<&ModelOverride>) -> Capabilities {
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
        }
    }

    #[test]
    fn npm_selects_the_wire() {
        assert_eq!(Wire::from_npm("@ai-sdk/openai-compatible"), Wire::Chat);
        assert_eq!(Wire::from_npm("@ai-sdk/anthropic"), Wire::Messages);
        assert_eq!(Wire::from_npm("@ai-sdk/google"), Wire::GenerateContent);
        assert_eq!(Wire::from_npm("@ai-sdk/openai"), Wire::Responses);
        assert_eq!(Wire::from_npm("@ai-sdk/groq"), Wire::Chat);
    }

    #[test]
    fn every_wire_builds_from_an_endpoint() {
        let capabilities = capabilities(None);
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
        let capabilities = capabilities(None);
        let error = VendorModels
            .build(&spec("lilac", Wire::Chat, None), &capabilities)
            .err()
            .expect("an error");
        assert!(matches!(error, ServiceError::MissingEndpoint { .. }));
    }
}
