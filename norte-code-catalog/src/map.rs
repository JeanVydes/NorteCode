//! Mapping a catalogue model to Norte's capability declaration.
//!
//! This is the one place where a provider fact becomes [`norte::Capabilities`].
//! Every mapping here is total and from an enum to an enum: a name the
//! catalogue does not carry is a name that was never in the file, because the
//! file's enums refused it at parse time. Nothing is guessed and nothing is
//! inferred from a string, and there are no overrides above it — the catalogue
//! is the authority on what a model can do, and the configuration no longer
//! holds a second opinion.

use std::collections::BTreeSet;

use norte::{
    CacheMode, Capabilities, Feature, Modality, Reasoning, ReasoningEffort, StreamingGranularity,
    StructuredOutputMode, ToolKind,
};

use crate::schema::{
    Modality as DeclaredModality, Model, SupportedEffort, SupportedParams, WireSchema,
};

/// The Norte wire family and cache mode a catalogue schema implies.
///
/// They are inherent methods on the file's own enum rather than a second type
/// that renames it. `WireKind` was that second type: `Chat` was `OpenAI`,
/// `Messages` was `Anthropic` and `GenerateContent` was `Google`, so every value
/// had two names and a conversion between them that could only be a bug.
impl WireSchema {
    /// The Norte wire-family string.
    ///
    /// The crate that implements each wire owns the string, so it is read from
    /// there rather than written out here where it could drift from it.
    #[must_use]
    pub fn family(self) -> &'static str {
        match self {
            Self::OpenAI => norte::openai::WIRE_CHAT,
            Self::Anthropic => norte::anthropic::WIRE_MESSAGES,
            Self::Google => norte::gemini::WIRE_GENERATE,
        }
    }

    /// How the wire's provider caches prompt prefixes.
    ///
    /// Anthropic's cache is explicit — the request marks the blocks it wants
    /// cached — and the other two do it themselves from the prefix.
    #[must_use]
    pub fn cache_mode(self) -> CacheMode {
        match self {
            Self::Anthropic => CacheMode::Explicit,
            Self::OpenAI | Self::Google => CacheMode::Automatic,
        }
    }
}

/// The capabilities a catalogue model declares.
///
/// A missing limit stays absent, which makes compaction refuse for that model
/// with a visible warning instead of guessing at a window it does not know.
#[must_use]
pub fn capabilities(model: &Model, schema: WireSchema) -> Capabilities {
    let mut builder = Capabilities::builder()
        .streaming(StreamingGranularity::Delta)
        .cache(schema.cache_mode())
        .reasoning(reasoning(model));

    // An empty input list means the file said nothing, and an entry that says
    // nothing is not an entry that refuses everything: a model that accepted no
    // part could not hold a conversation either, so text is assumed.
    let input: Vec<Modality> = if model.modalities.input.is_empty() {
        vec![Modality::Text]
    } else {
        model
            .modalities
            .input
            .iter()
            .copied()
            .map(modality_of)
            .collect()
    };
    for modality in input {
        builder = builder.input(modality);
    }
    let output: Vec<Modality> = if model.modalities.output.is_empty() {
        vec![Modality::Text]
    } else {
        model
            .modalities
            .output
            .iter()
            .copied()
            .map(modality_of)
            .collect()
    };
    for modality in output {
        builder = builder.output(modality);
    }

    for feature in features(model) {
        builder = builder.feature(feature);
    }
    if model.supported_params.contains(&SupportedParams::Tools) {
        builder = builder.tool_kind(ToolKind::Function);
    }
    // Which mode is not declared: the catalogue says whether a schema is
    // enforced, and the conservative reading of "structured output" is a JSON
    // object shaped by the prompt rather than enforced by the provider.
    if model
        .supported_params
        .contains(&SupportedParams::StructuredOutput)
    {
        builder = builder.structured_output(StructuredOutputMode::JsonObject);
    }
    if let Some(context) = model.limit.context {
        builder = builder.max_context(context);
    }
    if let Some(output) = model.limit.output {
        builder = builder.max_output(output);
    }

    builder.build()
}

/// The independent on/off capabilities a model's parameter list implies.
///
/// Norte's [`Feature`] set is the closed list for this, so a parameter that is
/// a feature is mapped here and one that is a request field is not:
/// `temperature` and `seed` are things a request may carry, which is not a
/// question Norte's capability type asks.
fn features(model: &Model) -> Vec<Feature> {
    let mut features = Vec::new();
    let has = |param: SupportedParams| model.supported_params.contains(&param);
    if has(SupportedParams::ToolChoice) {
        features.push(Feature::ForcedToolChoice);
    }
    if has(SupportedParams::Logprobs) || has(SupportedParams::TopLogprobs) {
        features.push(Feature::Logprobs);
    }
    features
}

/// Norte's modality for a catalogue one.
#[must_use]
pub fn modality_of(modality: DeclaredModality) -> Modality {
    match modality {
        DeclaredModality::Text => Modality::Text,
        DeclaredModality::Image => Modality::Image,
        DeclaredModality::Audio => Modality::Audio,
        DeclaredModality::Video => Modality::Video,
        DeclaredModality::File => Modality::Document,
    }
}

/// The reasoning declaration a catalogue model carries.
///
/// `mandatory` is what makes `can_disable` false: a model that always reasons
/// cannot be asked not to, which is a fact about the model and not a default a
/// person may change.
#[must_use]
pub fn reasoning(model: &Model) -> Reasoning {
    let declared = &model.reasoning;
    let mut efforts: BTreeSet<ReasoningEffort> = declared
        .supported_efforts
        .iter()
        .filter_map(|effort| effort_of(*effort))
        .collect();
    if let Some(default) = declared.default_effort.and_then(effort_of) {
        efforts.insert(default);
    }
    // A model that reasons but declares no effort at all still reasons: the
    // declaration says so, and refusing every setting would leave a control the
    // provider offers unusable. The effort set stays empty in that case, which
    // is what leaves the neutral request as the only choice.
    let supported = declared.mandatory
        || declared.default_enabled
        || declared.budget_supported
        || !declared.supported_efforts.is_empty();
    Reasoning {
        supported,
        // A model that does not reason has nothing to turn off, and one that
        // always reasons cannot be asked to stop. The file says the rest: an
        // effort list that names `none` is a provider that accepts off, and a
        // provider that accepts it without listing it is a provider whose
        // dialect this host does not guess at.
        can_disable: supported
            && !declared.mandatory
            && declared.supported_efforts.contains(&SupportedEffort::None),
        efforts,
        supports_budget: declared.budget_supported,
        // The catalogue carries no ceiling, only whether a budget is taken: a
        // number invented here would be a limit the provider never agreed to.
        max_budget: None,
    }
}

/// Norte's effort for a catalogue one, when the two name the same thing.
///
/// `None` is not an effort — it is reasoning turned off — so it maps to nothing
/// here and is read where `can_disable` is decided.
#[must_use]
pub fn effort_of(effort: SupportedEffort) -> Option<ReasoningEffort> {
    match effort {
        SupportedEffort::None => None,
        SupportedEffort::Minimal => Some(ReasoningEffort::Minimal),
        SupportedEffort::Low => Some(ReasoningEffort::Low),
        SupportedEffort::Medium => Some(ReasoningEffort::Medium),
        SupportedEffort::High => Some(ReasoningEffort::High),
        SupportedEffort::Xhigh => Some(ReasoningEffort::XHigh),
        SupportedEffort::Max => Some(ReasoningEffort::Max),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Limit, Modalities, Reasoning as Declared};

    fn model() -> Model {
        Model {
            id: "m".to_owned(),
            provider_id: "p".to_owned(),
            canonical_slug: "p/m".to_owned(),
            name: "m".to_owned(),
            limit: Limit {
                context: Some(128_000),
                output: Some(8_192),
            },
            modalities: Modalities {
                input: vec![DeclaredModality::Text, DeclaredModality::Image],
                output: vec![DeclaredModality::Text],
            },
            supported_params: vec![SupportedParams::Tools, SupportedParams::Temperature],
            ..Model::default()
        }
    }

    #[test]
    fn the_three_schemas_name_the_three_wire_families() {
        // The schema is what a person writes in `providers.json`, and the family
        // is Norte's name for the wire it selects. There is one type and not two
        // names for each value: `WireKind::Chat` and `WireSchema::OpenAI` were
        // the same fact written twice.
        assert_eq!(WireSchema::OpenAI.family(), norte::openai::WIRE_CHAT);
        assert_eq!(
            WireSchema::Anthropic.family(),
            norte::anthropic::WIRE_MESSAGES
        );
        assert_eq!(WireSchema::Google.family(), norte::gemini::WIRE_GENERATE);
    }

    #[test]
    fn only_anthropic_caches_explicitly() {
        assert_eq!(WireSchema::Anthropic.cache_mode(), CacheMode::Explicit);
        assert_eq!(WireSchema::OpenAI.cache_mode(), CacheMode::Automatic);
        assert_eq!(WireSchema::Google.cache_mode(), CacheMode::Automatic);
    }

    #[test]
    fn a_model_that_takes_tools_declares_the_function_tool_kind() {
        let declared = capabilities(&model(), WireSchema::OpenAI);
        assert!(declared.tool_kinds.contains(&ToolKind::Function));
        assert_eq!(declared.max_context, Some(128_000));
        assert_eq!(declared.max_output, Some(8_192));
        assert!(declared.modalities.supports_input(Modality::Image));
        assert!(declared.modalities.supports_output(Modality::Text));
    }

    #[test]
    fn a_model_with_no_parameter_list_takes_no_tools() {
        // The list is what says so: an entry that does not name `tools` is an
        // entry whose model cannot call one, and the conservative reading is
        // what the provider declared.
        let mut bare = model();
        bare.supported_params.clear();
        let declared = capabilities(&bare, WireSchema::OpenAI);
        assert!(!declared.tool_kinds.contains(&ToolKind::Function));
    }

    #[test]
    fn structured_output_follows_the_parameter_list() {
        let mut sample = model();
        assert!(
            capabilities(&sample, WireSchema::OpenAI)
                .structured_output
                .is_empty()
        );
        sample
            .supported_params
            .push(SupportedParams::StructuredOutput);
        assert!(
            capabilities(&sample, WireSchema::OpenAI)
                .structured_output
                .contains(&StructuredOutputMode::JsonObject)
        );
    }

    #[test]
    fn an_empty_modality_list_is_text_and_not_nothing() {
        let mut bare = model();
        bare.modalities.input.clear();
        bare.modalities.output.clear();
        let declared = capabilities(&bare, WireSchema::OpenAI);
        assert!(declared.modalities.supports_input(Modality::Text));
        assert!(declared.modalities.supports_output(Modality::Text));
    }

    #[test]
    fn a_mandatory_model_cannot_be_turned_off() {
        // The case the field exists for: some models always reason, and asking
        // otherwise is asking for something the provider will not do.
        let mut sample = model();
        sample.reasoning = Declared {
            mandatory: true,
            default_enabled: true,
            supported_efforts: vec![SupportedEffort::Low, SupportedEffort::High],
            ..Declared::default()
        };
        let declared = reasoning(&sample);
        assert!(declared.supported);
        assert!(!declared.can_disable, "mandatory means it cannot be off");
        assert!(declared.efforts.contains(&ReasoningEffort::High));
    }

    #[test]
    fn efforts_and_the_default_are_both_offered() {
        let mut sample = model();
        sample.reasoning = Declared {
            default_enabled: true,
            default_effort: Some(SupportedEffort::Medium),
            supported_efforts: vec![
                SupportedEffort::None,
                SupportedEffort::Low,
                SupportedEffort::Xhigh,
            ],
            budget_supported: true,
            ..Declared::default()
        };
        let declared = reasoning(&sample);
        assert!(declared.supported);
        assert!(declared.can_disable);
        assert!(declared.efforts.contains(&ReasoningEffort::Low));
        assert!(declared.efforts.contains(&ReasoningEffort::XHigh));
        assert!(
            declared.efforts.contains(&ReasoningEffort::Medium),
            "the default is offered whether or not the list repeats it"
        );
        assert!(declared.supports_budget);
    }

    #[test]
    fn a_model_that_does_not_reason_declares_nothing() {
        let declared = reasoning(&model());
        assert!(!declared.supported);
        assert!(declared.efforts.is_empty());
        assert!(!declared.can_disable);
    }

    #[test]
    fn a_parameter_that_is_a_feature_becomes_one() {
        let mut sample = model();
        sample.supported_params.push(SupportedParams::ToolChoice);
        sample.supported_params.push(SupportedParams::Logprobs);
        let declared = capabilities(&sample, WireSchema::OpenAI);
        assert!(declared.features.contains(&Feature::ForcedToolChoice));
        assert!(declared.features.contains(&Feature::Logprobs));
    }

    #[test]
    fn every_documented_parameter_is_accepted() {
        // Not every name maps to something Norte asks about — `temperature` and
        // `seed` are request fields, not capabilities — so this asserts the ones
        // that do and that the rest are read without a panic.
        let mut sample = model();
        sample.supported_params = vec![
            SupportedParams::FrequencyPenalty,
            SupportedParams::IncludeReasoning,
            SupportedParams::Logprobs,
            SupportedParams::MaxTokens,
            SupportedParams::MinP,
            SupportedParams::PresencePenalty,
            SupportedParams::Reasoning,
            SupportedParams::ReasoningEffort,
            SupportedParams::ResponseFormat,
            SupportedParams::Seed,
            SupportedParams::Stop,
            SupportedParams::StructuredOutput,
            SupportedParams::Temperature,
            SupportedParams::ToolChoice,
            SupportedParams::Tools,
            SupportedParams::TopP,
            SupportedParams::TopLogprobs,
            SupportedParams::TopK,
            SupportedParams::Verbosity,
        ];
        let declared = capabilities(&sample, WireSchema::OpenAI);
        assert!(declared.tool_kinds.contains(&ToolKind::Function));
        assert!(
            declared
                .structured_output
                .contains(&StructuredOutputMode::JsonObject)
        );
        assert!(declared.features.contains(&Feature::ForcedToolChoice));
        assert!(declared.features.contains(&Feature::Logprobs));
    }
}
