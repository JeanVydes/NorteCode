use super::*;
use norte_code_wire::catalog::{Limit, Modalities, Reasoning as Declared};

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
