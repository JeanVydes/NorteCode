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
use norte_code_config::{ModalityKind, ModelOverride, ReasoningOverride, WireKind};

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

impl From<WireKind> for Wire {
    fn from(kind: WireKind) -> Self {
        match kind {
            WireKind::Responses => Self::Responses,
            WireKind::Chat => Self::Chat,
            WireKind::Messages => Self::Messages,
            WireKind::GenerateContent => Self::GenerateContent,
        }
    }
}

impl Wire {
    /// Maps a models.dev `npm` package to a wire.    ///
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
    let mut declared = reasoning(
        model,
        overrides.and_then(|overrides| overrides.reasoning.as_ref()),
    );
    // models.dev exposes a `toggle` for a visible off switch, but several
    // Anthropic models accept `thinking: {type: "disabled"}` without one. Add
    // the API fact unless the host has an explicit override.
    if wire == Wire::Messages
        && overrides
            .and_then(|overrides| overrides.reasoning.as_ref())
            .and_then(|overrides| overrides.can_disable)
            .is_none()
        && anthropic_can_disable(&model.id)
    {
        declared.can_disable = true;
    }
    let mut builder = Capabilities::builder()
        .streaming(StreamingGranularity::Delta)
        .cache(wire.cache_mode())
        .reasoning(declared);

    // The configured modalities win outright: a list replaces what the
    // catalogue declared rather than adding to it, so a host can take an image
    // away from a model the catalogue overstates as well as give one to a model
    // the catalogue does not describe at all. Each key falls back on its own: a
    // model with an input list and no output one keeps the output the catalogue
    // declared.
    let modalities = overrides.and_then(|overrides| overrides.modalities.as_ref());
    let configured_input = modalities.and_then(|modalities| modalities.input.as_ref());
    let configured_output = modalities.and_then(|modalities| modalities.output.as_ref());

    let input: Vec<Modality> = match configured_input {
        Some(configured) => configured.iter().copied().map(modality_of_kind).collect(),
        None => declared_modalities(&model.modalities.input, Modality::Text).collect(),
    };
    for modality in input {
        builder = builder.input(modality);
    }

    let output: Vec<Modality> = match configured_output {
        Some(configured) => configured.iter().copied().map(modality_of_kind).collect(),
        None => declared_modalities(&model.modalities.output, Modality::Text).collect(),
    };
    for modality in output {
        builder = builder.output(modality);
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
                        if matches!(value.as_str(), "none" | "off" | "disabled") {
                            can_disable = true;
                        } else if let Some(effort) = effort_of(value) {
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

/// The modalities a catalogue list names, or `fallback` when it names none this
/// build knows.
///
/// A model that declares no input modality at all is taken to take text: an
/// entry that says nothing is not an entry that says "nothing is accepted", and
/// a model that refused every part could not hold a conversation either.
fn declared_modalities(
    names: &[String],
    fallback: Modality,
) -> impl Iterator<Item = Modality> + '_ {
    let known: Vec<Modality> = names.iter().filter_map(|name| modality_of(name)).collect();
    if known.is_empty() {
        return vec![fallback].into_iter();
    }
    known.into_iter()
}

/// The Norte modality a configured one names.
///
/// A free function and not a `From` impl: neither [`Modality`] nor
/// [`ModalityKind`] is local here, so the orphan rule forbids the impl. It lives
/// in this crate because this is where a provider fact becomes a Norte
/// capability, and the host uses it too so both paths agree on the spelling.
#[must_use]
pub fn modality_of_kind(kind: ModalityKind) -> Modality {
    match kind {
        ModalityKind::Text => Modality::Text,
        ModalityKind::Image => Modality::Image,
        ModalityKind::Audio => Modality::Audio,
        ModalityKind::Video => Modality::Video,
        ModalityKind::Pdf => Modality::Document,
    }
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
        "high" => Some(ReasoningEffort::High),
        "xhigh" => Some(ReasoningEffort::XHigh),
        "max" => Some(ReasoningEffort::Max),
        _ => None,
    }
}

/// Whether the Anthropic generation accepts `thinking: {type: "disabled"}`.
///
/// The Anthropic thinking table is the source: 4.5 through 4.8 and Opus 5
/// accept it; Opus 5.5 and Sonnet 5.5 reject it (Sonnet 5.5's lowest setting is
/// `between_tools`, which the neutral request does not express), and Fable,
/// Mythos and the 5.5 Opus/Sonnet models cannot turn thinking off. `toggle`
/// already covers Sonnet 5 and Haiku 5.5.
fn anthropic_can_disable(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    if !id.contains("claude") || id.contains("fable") || id.contains("mythos") {
        return false;
    }
    if id.contains("opus-5-5") || id.contains("sonnet-5-5") {
        return false;
    }
    id.contains("opus-5")
        || id.contains("4-5")
        || id.contains("4-6")
        || id.contains("4-7")
        || id.contains("4-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Catalog;
    use norte_code_config::ModalitiesOverride;

    #[test]
    fn npm_selects_the_wire() {
        assert_eq!(Wire::from_npm("@ai-sdk/openai-compatible"), Wire::Chat);
        assert_eq!(Wire::from_npm("@ai-sdk/anthropic"), Wire::Messages);
        assert_eq!(Wire::from_npm("@ai-sdk/google"), Wire::GenerateContent);
        assert_eq!(Wire::from_npm("@ai-sdk/openai"), Wire::Responses);
        assert_eq!(Wire::from_npm("@ai-sdk/groq"), Wire::Chat);
    }

    #[test]
    fn a_configured_wire_maps_to_its_kind() {
        assert_eq!(Wire::from(WireKind::Messages), Wire::Messages);
        assert_eq!(Wire::from(WireKind::Chat), Wire::Chat);
        assert_eq!(Wire::from(WireKind::Responses), Wire::Responses);
        assert_eq!(Wire::from(WireKind::GenerateContent), Wire::GenerateContent);
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
        assert!(
            capabilities
                .reasoning
                .efforts
                .contains(&ReasoningEffort::XHigh)
        );
        assert!(
            capabilities
                .reasoning
                .efforts
                .contains(&ReasoningEffort::Max)
        );
    }

    #[test]
    fn anthropic_models_that_accept_disabled_say_so() {
        let catalog = Catalog::bundled();
        for id in [
            "claude-opus-4-5",
            "claude-opus-4-8",
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-haiku-5-5",
        ] {
            let model = catalog.find("anthropic", id).expect(id);
            let capabilities = capabilities(model, Wire::Messages, None);
            assert!(capabilities.reasoning.can_disable, "{id}");
        }
        for id in ["claude-opus-5-5", "claude-sonnet-5-5"] {
            let model = catalog.find("anthropic", id).expect(id);
            let capabilities = capabilities(model, Wire::Messages, None);
            assert!(!capabilities.reasoning.can_disable, "{id}");
        }
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
            ..ModelOverride::default()
        };
        let capabilities = capabilities(model, Wire::GenerateContent, Some(&overrides));
        assert_eq!(capabilities.max_context, Some(1234));
        assert_eq!(capabilities.max_output, Some(99));
        assert!(!capabilities.reasoning.supported);
    }

    #[test]
    fn a_configured_modality_gives_a_text_model_an_image() {
        // The case the whole override exists for: a model the catalogue does not
        // describe, or describes without an image, takes one because the host
        // said so. Without this the part is refused as an input modality the
        // model was believed not to have.
        let catalog = Catalog::bundled();
        let model = catalog.find("google", "gemini-2.5-pro").expect("model");
        let overrides = ModelOverride {
            modalities: Some(ModalitiesOverride {
                input: Some(vec![ModalityKind::Text, ModalityKind::Image]),
                output: None,
            }),
            ..ModelOverride::default()
        };
        let declared = capabilities(model, Wire::GenerateContent, Some(&overrides));
        assert!(declared.modalities.supports_input(Modality::Image));
        assert!(declared.modalities.supports_input(Modality::Text));
        // The output half was left out, so it keeps what the catalogue said.
        assert!(declared.modalities.supports_output(Modality::Text));
    }

    #[test]
    fn a_configured_modality_takes_one_away_too() {
        // It replaces, rather than adds: a host that reads the catalogue as
        // overstating a model has to be able to say so.
        let catalog = Catalog::bundled();
        let model = catalog.find("google", "gemini-2.5-pro").expect("model");
        let overrides = ModelOverride {
            modalities: Some(ModalitiesOverride {
                input: Some(vec![ModalityKind::Text]),
                output: None,
            }),
            ..ModelOverride::default()
        };
        let declared = capabilities(model, Wire::GenerateContent, Some(&overrides));
        assert!(declared.modalities.supports_input(Modality::Text));
        assert!(!declared.modalities.supports_input(Modality::Document));
    }

    #[test]
    fn a_pdf_and_a_document_name_the_same_modality() {
        assert_eq!(modality_of_kind(ModalityKind::Pdf), Modality::Document);
        assert_eq!(modality_of("pdf"), Some(Modality::Document));
    }
}
