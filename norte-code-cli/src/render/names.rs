//! Lower-case names and the value sets for the capability enums.
//!
//! They serialize as snake case; these keep the debug output readable without
//! a serialization dependency.

use norte::Reasoning as ReasoningCapability;

use super::{
    CacheMode, Feature, Modality, ReasoningEffort, StreamingGranularity, StructuredOutputMode,
    ToolKind,
};

/// Formats a set of modalities.
pub(crate) fn modality_set<'a>(values: impl Iterator<Item = &'a Modality>) -> String {
    let names: Vec<_> = values.map(|value| modality_name(*value)).collect();
    if names.is_empty() {
        "-".to_owned()
    } else {
        names.join(", ")
    }
}

/// Formats the tool kinds a model accepts.
pub(crate) fn tool_kind_set(values: &std::collections::BTreeSet<ToolKind>) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values
        .iter()
        .map(|kind| tool_kind_name(*kind))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats the independent features a model declares.
pub(crate) fn feature_set(values: &std::collections::BTreeSet<Feature>) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values
        .iter()
        .map(|feature| feature_name(*feature))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats the structured-output modes a model declares.
pub(crate) fn structured_set(values: &std::collections::BTreeSet<StructuredOutputMode>) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values
        .iter()
        .map(|mode| structured_name(*mode))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats a model's reasoning controls.
pub(crate) fn reasoning_capabilities(reasoning: &ReasoningCapability) -> String {
    if !reasoning.supported {
        return "not supported".to_owned();
    }
    let mut parts = Vec::new();
    if reasoning.can_disable {
        parts.push("off allowed".to_owned());
    }
    if reasoning.efforts.is_empty() {
        parts.push("auto only".to_owned());
    } else {
        parts.push(format!(
            "efforts {}",
            reasoning
                .efforts
                .iter()
                .map(|effort| effort_label(*effort))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if reasoning.supports_budget {
        parts.push(reasoning.max_budget.map_or_else(
            || "budget yes".to_owned(),
            |max| format!("budget up to {max}"),
        ));
    }
    parts.join(" · ")
}

/// Lower-case labels for the capability enums. They serialize as snake case;
/// these keep the debug output readable without a serialization dependency.
pub(crate) fn modality_name(modality: Modality) -> &'static str {
    match modality {
        Modality::Text => "text",
        Modality::Image => "image",
        Modality::Audio => "audio",
        Modality::Video => "video",
        Modality::Document => "document",
        _ => "other",
    }
}

pub(crate) fn tool_kind_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Function => "function",
        ToolKind::ProviderExecuted => "provider-executed",
        ToolKind::ProviderSchema => "provider-schema",
        _ => "other",
    }
}

pub(crate) fn feature_name(feature: Feature) -> &'static str {
    match feature {
        Feature::Citations => "citations",
        Feature::ParallelToolCalls => "parallel-tool-calls",
        Feature::ForcedToolChoice => "forced-tool-choice",
        Feature::Batch => "batch",
        Feature::Logprobs => "logprobs",
        _ => "other",
    }
}

pub(crate) fn structured_name(mode: StructuredOutputMode) -> &'static str {
    match mode {
        StructuredOutputMode::JsonObject => "json-object",
        StructuredOutputMode::JsonSchema => "json-schema",
        StructuredOutputMode::StrictSchema => "strict-schema",
        _ => "other",
    }
}

pub(crate) fn streaming_name(mode: StreamingGranularity) -> &'static str {
    match mode {
        StreamingGranularity::Delta => "delta",
        StreamingGranularity::Part => "part",
        StreamingGranularity::Disabled => "disabled",
        _ => "other",
    }
}

pub(crate) fn cache_name(mode: CacheMode) -> &'static str {
    match mode {
        CacheMode::None => "none",
        CacheMode::Automatic => "automatic",
        CacheMode::Explicit => "explicit",
        _ => "other",
    }
}

pub(crate) fn effort_label(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::XHigh => "xhigh",
        ReasoningEffort::Max => "max",
        _ => "other",
    }
}

/// The name a catalogue file writes for a supported parameter.
///
/// It is a function and not a `Display` impl because the names are the file's
/// vocabulary and not Rust's: `frequency_penalty` is what a person types in
/// `models.json`, and the debug output has to read like the file it describes.
pub(crate) fn supported_param_name(param: norte_code_catalog::SupportedParams) -> &'static str {
    use norte_code_catalog::SupportedParams as P;
    match param {
        P::FrequencyPenalty => "frequency_penalty",
        P::IncludeReasoning => "include_reasoning",
        P::Logprobs => "logprobs",
        P::MaxTokens => "max_tokens",
        P::MinP => "min_p",
        P::PresencePenalty => "presence_penalty",
        P::Reasoning => "reasoning",
        P::ReasoningEffort => "reasoning_effort",
        P::ResponseFormat => "response_format",
        P::Seed => "seed",
        P::Stop => "stop",
        P::StructuredOutput => "structured_output",
        P::Temperature => "temperature",
        P::ToolChoice => "tool_choice",
        P::Tools => "tools",
        P::TopP => "top_p",
        P::TopLogprobs => "top_logprobs",
        P::TopK => "top_k",
        P::Verbosity => "verbosity",
    }
}
