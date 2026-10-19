//! Mapping catalogue entries to Norte.
//!
//! This is the one place where provider facts become [`norte::Capabilities`]:
//! modalities, limits, tool calling, structured output, prompt-cache mode and
//! reasoning. Config overrides always win, and a missing field stays missing
//! rather than becoming a guess.

use std::collections::BTreeSet;

use norte::{
    CacheMode, Capabilities, Modality, Reasoning, ReasoningEffort, StreamingGranularity,
    StructuredOutputMode, ToolKind,
};
use norte_code_config::{ModelOverride, ReasoningOverride};

use crate::schema::{Model, ReasoningOption};

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
    /// `openai-compatible` means the chat wire, `anthropic` the Messages wire,
    /// `google` the `generateContent` wire, and `@ai-sdk/openai` the Responses
    /// wire. Everything else is assumed OpenAI-compatible, which is what most
    /// of the catalogue's `openai-compatible` entries are.
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

    /// The fallback for the four providers the spec names, before the
    /// catalogue is available.
    #[must_use]
    pub fn for_provider(provider: &str) -> Self {
        match provider {
            "openai" => Self::Responses,
            "anthropic" => Self::Messages,
            "google" | "gemini" => Self::GenerateContent,
            _ => Self::Chat,
        }
    }

    /// The Norte wire-family string.
    #[must_use]
    pub fn family(self) -> &'static str {
        match self {
            Self::Responses => norte::openai::WIRE_RESPONSES,
            Self::Chat => norte::openai::WIRE_CHAT,
            Self::Messages => norte::anthropic::WIRE_MESSAGES,
            Self::GenerateContent => norte::gemini::WIRE_GENERATE,
        }
    }

    /// How the provider caches prompt prefixes.
    #[must_use]
    pub fn cache_mode(self) -> CacheMode {
        match self {
            Self::Messages => CacheMode::Explicit,
            Self::Responses | Self::Chat | Self::GenerateContent => CacheMode::Automatic,
        }
    }
}

/// Builds the capability declaration for one catalogue model.
///
/// Config overrides win for the context window, the output limit and the
/// reasoning declaration. A missing limit stays absent, which makes compaction
/// refuse for that model with a visible warning instead of guessing.
#[must_use]
pub fn capabilities(model: &Model, wire: Wire, overrides: Option<&ModelOverride>) -> Capabilities {
    let mut builder = Capabilities::builder()
        .streaming(StreamingGranularity::Delta)
        .cache(wire.cache_mode())
        .reasoning(reasoning(
            model,
            overrides.and_then(|overrides| overrides.reasoning.as_ref()),
        ));

    let mut has_input = false;
    for name in &model.modalities.input {
        if let Some(modality) = modality_of(name) {
            builder = builder.input(modality);
            has_input = true;
        }
    }
    if !has_input {
        builder = builder.input(Modality::Text);
    }

    let mut has_output = false;
    for name in &model.modalities.output {
        if let Some(modality) = modality_of(name) {
            builder = builder.output(modality);
            has_output = true;
        }
    }
    if !has_output {
        builder = builder.output(Modality::Text);
    }

    if model.tool_call {
        builder = builder.tool_kind(ToolKind::Function);
    }
    // The catalogue says whether structured output exists, not which mode; the
    // conservative reading is a JSON object without a schema.
    if model.structured_output == Some(true) {
        builder = builder.structured_output(StructuredOutputMode::JsonObject);
    }

    if let Some(context) = overrides
        .and_then(|overrides| overrides.max_context)
        .or(model.limit.context)
    {
        builder = builder.max_context(context);
    }
    if let Some(output) = overrides
        .and_then(|overrides| overrides.max_output)
        .or(model.limit.output)
    {
        builder = builder.max_output(output);
    }

    builder.build()
}

/// Maps the catalogue's reasoning declaration, then applies config overrides.
///
/// When the catalogue only says `reasoning: true`, the result is supported
/// with no declared effort and no budget, which leaves `Automatic` as the only
/// choice. That is deliberate: Norte's check is strict, so Norte Code never
/// offers a control the provider did not declare.
#[must_use]
pub fn reasoning(model: &Model, overrides: Option<&ReasoningOverride>) -> Reasoning {
    let mut result = if model.reasoning {
        let mut efforts = BTreeSet::new();
        let mut can_disable = false;
        let mut supports_budget = false;
        let mut max_budget = None;
        for option in &model.reasoning_options {
            match option {
                ReasoningOption::Effort { values } => {
                    for value in values {
                        if let Some(effort) = effort_of(value) {
                            efforts.insert(effort);
                        }
                    }
                }
                ReasoningOption::BudgetTokens { max, .. } => {
                    supports_budget = true;
                    // `-1` and `0` mean "no declared ceiling"; Norte's field is
                    // a positive u32.
                    if let Some(max) = max {
                        if *max > 0 {
                            max_budget = u32::try_from(*max).ok();
                        }
                    }
                }
                ReasoningOption::Toggle => can_disable = true,
                ReasoningOption::Unknown => {}
            }
        }
        Reasoning {
            supported: true,
            can_disable,
            efforts,
            supports_budget,
            max_budget,
        }
    } else {
        Reasoning::default()
    };

    if let Some(overrides) = overrides {
        if let Some(supported) = overrides.supported {
            result.supported = supported;
        }
        if let Some(can_disable) = overrides.can_disable {
            result.can_disable = can_disable;
        }
        if let Some(efforts) = &overrides.efforts {
            result.efforts = efforts.iter().filter_map(|name| effort_of(name)).collect();
        }
        if let Some(supports_budget) = overrides.supports_budget {
            result.supports_budget = supports_budget;
        }
        if let Some(max_budget) = overrides.max_budget {
            result.max_budget = Some(max_budget);
        }
    }
    result
}

fn modality_of(name: &str) -> Option<Modality> {
    match name {
        "text" => Some(Modality::Text),
        "image" => Some(Modality::Image),
        "audio" => Some(Modality::Audio),
        "video" => Some(Modality::Video),
        "pdf" | "document" | "file" => Some(Modality::Document),
        _ => None,
    }
}

fn effort_of(name: &str) -> Option<ReasoningEffort> {
    match name {
        "minimal" => Some(ReasoningEffort::Minimal),
        "low" => Some(ReasoningEffort::Low),
        "medium" => Some(ReasoningEffort::Medium),
        // Norte models four efforts; the catalogue's newer top ends map down.
        "high" | "xhigh" | "max" => Some(ReasoningEffort::High),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Catalog;

    #[test]
    fn npm_selects_the_wire() {
        assert_eq!(Wire::from_npm("@ai-sdk/openai-compatible"), Wire::Chat);
        assert_eq!(Wire::from_npm("@ai-sdk/anthropic"), Wire::Messages);
        assert_eq!(Wire::from_npm("@ai-sdk/google"), Wire::GenerateContent);
        assert_eq!(Wire::from_npm("@ai-sdk/openai"), Wire::Responses);
        assert_eq!(Wire::from_npm("@ai-sdk/groq"), Wire::Chat);
    }

    #[test]
    fn gemini_maps_limits_modalities_and_cache() {
        let catalog = Catalog::bundled();
        let model = catalog.find("google", "gemini-2.5-pro").expect("model");
        let capabilities = capabilities(model, Wire::GenerateContent, None);
        assert_eq!(capabilities.max_context, Some(1_048_576));
        assert!(
            capabilities
                .modalities
                .input
                .contains_key(&Modality::Document)
        );
        assert!(capabilities.modalities.output.contains(&Modality::Text));
        assert!(capabilities.tool_kinds.contains(&ToolKind::Function));
        assert_eq!(capabilities.cache, CacheMode::Automatic);
    }

    #[test]
    fn anthropic_caches_explicitly_and_keeps_its_efforts() {
        let catalog = Catalog::bundled();
        let model = catalog
            .find("anthropic", "claude-sonnet-5-5")
            .expect("model");
        let capabilities = capabilities(model, Wire::Messages, None);
        assert_eq!(capabilities.cache, CacheMode::Explicit);
        assert!(capabilities.reasoning.supported);
        assert!(
            capabilities
                .reasoning
                .efforts
                .contains(&ReasoningEffort::High)
        );
    }

    #[test]
    fn overrides_win_over_the_catalogue() {
        let catalog = Catalog::bundled();
        let model = catalog.find("google", "gemini-2.5-pro").expect("model");
        let overrides = ModelOverride {
            max_context: Some(1234),
            max_output: Some(99),
            reasoning: Some(ReasoningOverride {
                supported: Some(false),
                ..ReasoningOverride::default()
            }),
        };
        let capabilities = capabilities(model, Wire::GenerateContent, Some(&overrides));
        assert_eq!(capabilities.max_context, Some(1234));
        assert_eq!(capabilities.max_output, Some(99));
        assert!(!capabilities.reasoning.supported);
    }
}
