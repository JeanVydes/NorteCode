//! Runtime-independent RAEP host adapter.

use std::collections::HashMap;
use std::sync::Arc;

use norte_code_agent::Service;
use norte_code_wire::{ClientRequestKind, ServerEventKind, SessionKey};
use raep_types::{AttachMode, Envelope, FrameType, StandardFrameType};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore, broadcast};

use crate::payloads::payload;
use norte_code_wire::{JobStatus, ServiceDataKind};

mod agents;
mod artifacts;
pub(crate) mod capabilities;
mod config_accessors;
mod emit;
mod exports;
mod frames;
mod history;
mod jobs;
pub(crate) mod payloads;
pub(crate) mod replay;
mod sessions;
pub(crate) mod stores;
mod turns;
pub(crate) mod uploads;

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use capabilities::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use payloads::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use replay::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use stores::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use uploads::*;

/// A canonical `service_data` payload, decoded to find the telemetry rows.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CanonicalServiceData {
    kind: ServiceDataKind,
    data: Value,
}

/// A replay descriptor in an attach payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ReplayMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    from_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_seq: Option<u64>,
    truncated: bool,
}

/// The session-attached payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SessionAttachedPayload {
    created: bool,
    mode: AttachMode,
    last_seq: u64,
    replay: ReplayMeta,
    negotiated_capabilities: Vec<Capability>,
    /// The non-secret summary of the effective agent (RFC 7). This host
    /// only runs one configured agent, so it is that agent.
    agent: norte_code_wire::AgentSummary,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct UserInputOrigin {
    kind: UserInputOriginKind,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct QueuedTurnStatePayload {
    state: raep_types::TurnState,
}

#[derive(Debug, serde::Serialize)]
pub(crate) enum UserInputOriginKind {
    #[serde(rename = "user")]
    User,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct AcceptedContentPart {
    #[serde(rename = "type")]
    kind: AcceptedPartKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<AcceptedSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<ImageDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pages: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AcceptedPartKind {
    #[default]
    Text,
    Image,
    Document,
    Audio,
    Video,
}

impl AcceptedPartKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Document => "document",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ImageDetail {
    Auto,
    Low,
    High,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(crate) enum AcceptedSource {
    #[serde(rename = "upload")]
    Upload { upload_id: String },
    #[serde(rename = "output")]
    Output { output_id: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AssetSourceKind {
    Upload,
    Output,
}

/// A `USER_INPUT` payload as this host accepts it.
///
/// The spec (RFC 12.4) allows two more regions that this host does not
/// implement yet: `replaces_turn_id` (rewind-and-re-run in one step) and any
/// `response_format` other than plain `text`. Both are still decoded here,
/// so a client that sends them gets a clear rejection from
/// [`validate_user_input`] instead of silently ignoring them.
///
/// **`replaces_turn_id`** (RFC 18.2): `SESSION_REWIND` to that turn with
/// `inclusive: true` and `cancel_pending: false`, then run the new input as a
/// new turn, in one step. `queue_policy` does not apply. The new envelope
/// `turn_id` MUST be new, and the same preconditions and errors apply.
/// Implemented in [`NorteCodeRaepHost::user_input`].
///
/// **`response_format`** (RFC 12.6): `{type: "text"}` (default) or
/// `{type: "json_schema", schema, strict}`. `schema` is a JSON Schema
/// (2020-12, at most 16 KiB, local `$ref` only). The capability is
/// `structured_output` and the effective agent must list
/// `structured_output` in `capabilities.features`; otherwise the input
/// fails with `INVALID_FRAME` and `details.reason:
/// response_format_unsupported`. Implemented in
/// [`NorteCodeRaepHost::user_input`] and [`NorteCodeRaepHost::validate_user_input`].
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub(crate) struct AcceptedUserInput {
    /// Plain text input. Exactly one of `prompt` and `content` is required.
    #[serde(default)]
    prompt: Option<String>,
    /// Multimodal input. Exactly one of `prompt` and `content` is required.
    #[serde(default)]
    content: Option<Vec<AcceptedContentPart>>,
    /// Behavior when another turn is active.
    #[serde(default)]
    queue_policy: Option<raep_types::QueuePolicy>,
    /// Per-turn configuration overrides. See [`decode_override`]: the known
    /// fields are applied to this turn; unknown fields fail instead of being
    /// silently dropped.
    #[serde(default)]
    overrides: Option<Value>,
    /// Output modalities the client wants back. This host only produces text:
    /// anything other than `["text"]` is rejected.
    #[serde(default)]
    output: Option<Vec<raep_types::Modality>>,
    /// The shape the final answer must take. Hardcoded to `text` for this
    /// host: `json_schema` is rejected until structured output exists.
    #[serde(default)]
    response_format: Option<AcceptedResponseFormat>,
    /// Rewind-and-re-run in one step (RFC 18.2). Not supported yet.
    #[serde(default)]
    replaces_turn_id: Option<String>,
    /// Arbitrary caller metadata: an opaque object of at most 4 KiB, stored
    /// with the turn and never shown to the model (RFC 12.4). Validated here
    /// for size; the service's turn record has no slot for it yet, so it is
    /// not persisted.
    #[serde(default)]
    #[allow(dead_code)]
    metadata: Option<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResponseFormatKind {
    Text,
    JsonSchema,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(crate) struct AcceptedResponseFormat {
    #[serde(rename = "type")]
    kind: ResponseFormatKind,
    /// JSON Schema 2020-12, at most 16 KiB, local `$ref` only. Only present for
    /// `json_schema`.
    #[serde(default)]
    schema: Option<Value>,
    /// Whether the turn fails when the answer does not validate, instead of
    /// earning one correction message.
    #[serde(default)]
    strict: bool,
}

/// Validates the shape of a `response_format` value without touching the
/// service. The capability check (`structured_output` in the effective
/// agent's `capabilities.features`) happens in `user_input`, which has the
/// service and the negotiated capabilities.
/// Whether every `$ref` in `value` is a local JSON Pointer (`#...`).
///
/// The spec allows `$ref` only within the same schema; a reference to
/// another document would need a resolver the host does not carry.
pub(crate) fn refs_are_local(value: &Value) -> Result<(), HostError> {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get("$ref")
                && !(reference.trim() == "#" || reference.trim().starts_with("#/"))
            {
                return Err(HostError::InvalidFrame(
                    "response_format $ref must be local".to_owned(),
                ));
            }
            for nested in map.values() {
                refs_are_local(nested)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for item in items {
                refs_are_local(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(crate) fn validate_response_format_shape(
    format: &AcceptedResponseFormat,
) -> Result<(), HostError> {
    match format.kind {
        ResponseFormatKind::Text => Ok(()),
        ResponseFormatKind::JsonSchema => {
            let Some(schema) = &format.schema else {
                return Err(HostError::InvalidFrame(
                    "response_format json_schema requires schema".to_owned(),
                ));
            };
            let bytes = serde_json::to_vec(schema)?.len();
            if bytes > 16 * 1024 {
                return Err(HostError::InvalidFrame(
                    "response_format schema exceeds 16 KiB".to_owned(),
                ));
            }
            // Local `$ref` only: any `$ref` that is not a JSON Pointer into
            // this same schema (`#...`) is rejected, matching the spec.
            refs_are_local(schema)?;
            Ok(())
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryGetRequest {
    #[serde(default)]
    before_turn_id: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    include_rewound: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkRequest {
    /// The fork source, repeated from the envelope by clients that fill the
    /// payload the way the spec's example does. The host reads the envelope's
    /// `session_id`; the field is accepted so a client naming the session
    /// twice is not rejected for it.
    #[serde(default)]
    #[allow(dead_code)]
    session_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    from_turn_id: Option<String>,
    #[serde(default)]
    inclusive: Option<bool>,
    #[serde(default)]
    carry_usage: Option<bool>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    include: Option<norte_code_wire::ForkInclude>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RewindRequest {
    to_turn_id: String,
    #[serde(default)]
    inclusive: bool,
    #[serde(default)]
    cancel_pending: bool,
    #[serde(default)]
    workspace: Option<WorkspaceRewindPolicy>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WorkspaceRewindPolicy {
    Keep,
    Restore,
}

pub(crate) fn default_true() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct ExportInclude {
    #[serde(default = "default_true")]
    children: bool,
    #[serde(default = "default_true")]
    uploads: bool,
    #[serde(default = "default_true")]
    job_output: bool,
    #[serde(default = "default_true")]
    outputs: bool,
    #[serde(default)]
    workspace: bool,
}

impl Default for ExportInclude {
    fn default() -> Self {
        Self {
            children: true,
            uploads: true,
            job_output: true,
            outputs: true,
            workspace: false,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExportRequest {
    #[serde(default)]
    include: ExportInclude,
    #[serde(default)]
    quiesce: Option<QuiescePolicy>,
    #[serde(default)]
    wait_timeout_s: Option<u64>,
    #[serde(default)]
    encrypt: Option<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QuiescePolicy {
    Reject,
    Wait,
    KillJobs,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportCommitRequest {
    #[serde(default)]
    agent_map: HashMap<String, String>,
    #[serde(default)]
    carry_usage: Option<bool>,
    #[serde(default)]
    decryption_jwk: Option<Value>,
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct PortableHistoryItem {
    role: PortableHistoryRole,
    content: Value,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    origin: Option<Value>,
    #[serde(default)]
    provenance: Option<Value>,
    #[serde(default)]
    rewound: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PortableHistoryRole {
    User,
    Assistant,
}

impl PortableHistoryRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// The `SESSION_CONFIGURED` payload.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SessionConfiguredPayload {
    applied: bool,
}

/// Rejects the `UserInput` field values this host does not support.
///
/// `replaces_turn_id` is now implemented as an atomic rewind-and-re-run (RFC
/// 18.2): `SESSION_REWIND` to that turn with `inclusive: true`,
/// `cancel_pending: false`, then the new input as a new turn. `queue_policy`
/// is ignored while `replaces_turn_id` is present. Non-text output is
/// accepted only when both the model and the attached client support it;
/// `metadata` over 4 KiB violates the spec's bound for opaque caller
/// metadata.
pub(crate) fn validate_user_input(input: &AcceptedUserInput) -> Result<(), HostError> {
    match (input.prompt.as_deref(), input.content.as_deref()) {
        (Some(prompt), None) if !prompt.trim().is_empty() => {
            if prompt.len() > 256 * 1024 {
                return Err(HostError::InvalidFrame(
                    "prompt exceeds max_prompt_bytes".to_owned(),
                ));
            }
        }
        (None, Some(parts)) if !parts.is_empty() => {
            validate_content_parts(parts)?;
        }
        (Some(_), Some(_)) => {
            return Err(HostError::InvalidFrame(
                "exactly one of prompt and content is required".to_owned(),
            ));
        }
        _ => {
            return Err(HostError::InvalidFrame(
                "prompt or non-empty content is required".to_owned(),
            ));
        }
    }
    if let Some(id) = &input.replaces_turn_id {
        // `replaces_turn_id` is implemented. It is a `SESSION_REWIND` +
        // `USER_INPUT` in one step; the actual rewind happens in
        // `NorteCodeRaepHost::user_input` before any queueing.
        // No validation error here — see the comment above.
        let _ = id;
    }
    if let Some(format) = &input.response_format
        && format.kind != ResponseFormatKind::Text
    {
        // `response_format` other than text is supported only when the
        // effective agent advertises `structured_output`. The host-side
        // capability check happens in `user_input` (which has the service,
        // the session spec, and the negotiated capabilities); here we only
        // validate the shape.
        validate_response_format_shape(format)?;
    }
    if let Some(output) = &input.output
        && (output.is_empty()
            || output
                .iter()
                .enumerate()
                .any(|(index, modality)| output[..index].contains(modality)))
    {
        return Err(HostError::InvalidFrame(
            "output must contain unique modalities".to_owned(),
        ));
    }
    if let Some(metadata) = &input.metadata {
        let bytes = serde_json::to_vec(metadata)?.len();
        if bytes > 4 * 1024 {
            return Err(HostError::InvalidFrame("metadata exceeds 4 KiB".to_owned()));
        }
    }
    Ok(())
}

pub(crate) fn validate_content_parts(parts: &[AcceptedContentPart]) -> Result<(), HostError> {
    if parts.len() > 16 {
        return Err(HostError::InvalidFrame(
            "content exceeds max_parts".to_owned(),
        ));
    }
    let mut text_bytes = 0_usize;
    for (index, part) in parts.iter().enumerate() {
        text_bytes = text_bytes.saturating_add(validate_content_part(part, index)?);
    }
    if text_bytes > 256 * 1024 {
        return Err(HostError::InvalidFrame(
            "content text exceeds max_prompt_bytes".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_content_part(
    part: &AcceptedContentPart,
    index: usize,
) -> Result<usize, HostError> {
    match part.kind {
        AcceptedPartKind::Text => {
            if part.text.as_deref().is_none_or(str::is_empty)
                || part.source.is_some()
                || part.mime_type.is_some()
            {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: text parts require text only"
                )));
            }
            Ok(part.text.as_deref().map_or(0, str::len))
        }
        AcceptedPartKind::Image => {
            if part.text.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: image parts cannot contain text"
                )));
            }
            if part
                .detail
                .is_some_and(|detail| detail != ImageDetail::Auto)
            {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: image detail overrides are not supported"
                )));
            }
            validate_upload_source(part, index)?;
            Ok(0)
        }
        AcceptedPartKind::Document => {
            if part.text.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: document parts cannot contain text"
                )));
            }
            if part.pages.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: document page ranges are not supported"
                )));
            }
            validate_upload_source(part, index)?;
            Ok(0)
        }
        AcceptedPartKind::Audio | AcceptedPartKind::Video => {
            if part.text.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: media parts cannot contain text"
                )));
            }
            if part
                .duration_ms
                .is_some_and(|duration| duration > MAX_MEDIA_DURATION_MS)
            {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: duration exceeds max_media_duration_ms"
                )));
            }
            if part
                .transcript
                .as_ref()
                .is_some_and(|text| text.len() > 16 * 1024)
            {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: transcript exceeds 16 KiB"
                )));
            }
            if part.kind == AcceptedPartKind::Video && part.transcript.is_some() {
                return Err(HostError::InvalidFrame(format!(
                    "content part {index}: transcript is only valid for audio"
                )));
            }
            validate_upload_source(part, index)?;
            Ok(0)
        }
    }
}

pub(crate) fn validate_upload_source(
    part: &AcceptedContentPart,
    index: usize,
) -> Result<(), HostError> {
    let mime = part.mime_type.as_deref().ok_or_else(|| {
        HostError::InvalidFrame(format!("content part {index}: mime_type is required"))
    })?;
    if part.source.is_none() {
        return Err(HostError::InvalidFrame(format!(
            "content part {index}: source is required"
        )));
    }
    validate_media_mime(part.kind, mime)
        .map_err(|message| HostError::InvalidFrame(format!("content part {index}: {message}")))
}

pub(crate) fn validate_media_mime(kind: AcceptedPartKind, mime: &str) -> Result<(), String> {
    norte::MimeType::new(mime.to_owned())
        .map_err(|_| format!("`{mime}` is not a valid MIME type"))?;
    let supported = match kind {
        AcceptedPartKind::Image => mime.starts_with("image/"),
        AcceptedPartKind::Document => {
            matches!(mime, "application/pdf" | "text/plain" | "text/markdown")
        }
        AcceptedPartKind::Audio => mime.starts_with("audio/"),
        AcceptedPartKind::Video => mime.starts_with("video/"),
        AcceptedPartKind::Text => false,
    };
    if supported {
        Ok(())
    } else {
        Err(format!(
            "MIME type `{mime}` is not supported for {} input",
            kind.as_str()
        ))
    }
}

pub(crate) fn supported_input_modalities(
    service: &Service,
    session: &SessionKey,
) -> Vec<norte::Modality> {
    let Some(spec) = service.session_spec(session) else {
        return Vec::new();
    };
    [
        norte::Modality::Text,
        norte::Modality::Image,
        norte::Modality::Document,
        norte::Modality::Audio,
        norte::Modality::Video,
    ]
    .into_iter()
    .filter(|modality| spec.capabilities.modalities.supports_input(*modality))
    .collect()
}

pub(crate) fn to_norte_modality(modality: &raep_types::Modality) -> norte::Modality {
    match modality {
        raep_types::Modality::Text => norte::Modality::Text,
        raep_types::Modality::Image => norte::Modality::Image,
        raep_types::Modality::Document => norte::Modality::Document,
        raep_types::Modality::Audio => norte::Modality::Audio,
        raep_types::Modality::Video => norte::Modality::Video,
    }
}

pub(crate) fn raep_modality_for_media(kind: norte_code_wire::MediaKind) -> raep_types::Modality {
    match kind {
        norte_code_wire::MediaKind::Image => raep_types::Modality::Image,
        norte_code_wire::MediaKind::Document => raep_types::Modality::Document,
        norte_code_wire::MediaKind::Audio => raep_types::Modality::Audio,
        norte_code_wire::MediaKind::Video => raep_types::Modality::Video,
    }
}

/// Extracts the reasoning, model and budget overrides from one override
/// object.
///
/// Only the fields in the closed set below are accepted; any other key
/// fails with `AGENT_OVERRIDE_DENIED`, so a client that sends a field this
/// host cannot honour gets an error naming it rather than a silent no-op.
pub(crate) type OverrideParts = (
    Option<norte_code_wire::Reasoning>,
    Option<(String, String)>,
    Option<norte_code_wire::Budget>,
);

pub(crate) fn decode_override(value: &Value) -> Result<OverrideParts, HostError> {
    let Value::Object(fields) = value else {
        return Err(HostError::InvalidFrame(
            "overrides must be an object".to_owned(),
        ));
    };
    let mut reasoning = None;
    let mut model = None;
    let mut budget = None;
    for (key, field) in fields {
        match key.as_str() {
            "reasoning" => {
                reasoning = Some(decode_reasoning_override(field)?);
            }
            "model" => {
                let id = field.get("id").and_then(Value::as_str).ok_or_else(|| {
                    HostError::InvalidFrame("model override requires `id`".to_owned())
                })?;
                let provider = field
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                model = Some((provider, id.to_owned()));
            }
            "provider" => {
                let id = field.as_str().ok_or_else(|| {
                    HostError::InvalidFrame("provider override must be a string".to_owned())
                })?;
                model = Some((id.to_owned(), model.map(|(_, m)| m).unwrap_or_default()));
            }
            "budgets" | "budget" => {
                budget = Some(decode_budget_override(field)?);
            }
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "override field `{other}` is not supported by this host"
                )));
            }
        }
    }
    Ok((reasoning, model, budget))
}

/// Rejects override values this host cannot honour.
///
/// This is the `AGENT_OVERRIDE_DENIED` check the spec requires (RFC 7):
/// narrowing is allowed, widening scopes, tool sets or budgets is not. This
/// host has no scopes or tool sets to widen, and budgets can only be set —
/// there is no raise — so the check is that the value decodes at all. A
/// malformed value fails here, naming the field, instead of deep inside the
/// service.
pub(crate) fn validate_override(
    reasoning: Option<&norte_code_wire::Reasoning>,
    model: Option<&(String, String)>,
    budget: Option<&norte_code_wire::Budget>,
) -> Result<(), HostError> {
    if let Some(norte_code_wire::Reasoning::Effort { effort }) = reasoning
        && effort.is_empty()
    {
        return Err(HostError::InvalidFrame(
            "reasoning.effort must not be empty".to_owned(),
        ));
    }
    if let Some((_, model_id)) = model
        && model_id.is_empty()
    {
        return Err(HostError::InvalidFrame(
            "model.id must not be empty".to_owned(),
        ));
    }
    if let Some(budget) = budget
        && budget.limit <= 0.0
    {
        return Err(HostError::InvalidFrame(
            "budget limit must be positive".to_owned(),
        ));
    }
    Ok(())
}

/// Decodes a `reasoning` override object into a [`norte_code_wire::Reasoning`].
///
/// The wire shape follows the `AgentSpec` fragment (RFC 7): `mode`
/// (`off`/`auto`/`on`), `effort` (a name) and `budget_tokens`. `mode: on`
/// without an effort maps to `Effort` with the service's default effort;
/// `mode: auto` maps to `Automatic` and `mode: off` to `Disabled`.
pub(crate) fn decode_reasoning_override(
    value: &Value,
) -> Result<norte_code_wire::Reasoning, HostError> {
    let Some(fields) = value.as_object() else {
        return Err(HostError::InvalidFrame(
            "reasoning override must be an object".to_owned(),
        ));
    };
    for key in fields.keys() {
        match key.as_str() {
            "mode" | "effort" | "budget_tokens" => {}
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "reasoning override field `{other}` is not supported"
                )));
            }
        }
    }
    let mode = fields.get("mode").and_then(Value::as_str).unwrap_or("auto");
    let effort = fields.get("effort").and_then(Value::as_str);
    let budget_tokens = fields.get("budget_tokens").and_then(Value::as_u64);
    Ok(match mode {
        "off" | "disabled" => norte_code_wire::Reasoning::Disabled,
        "auto" | "automatic" => norte_code_wire::Reasoning::Automatic,
        "on" => {
            if let Some(tokens) = budget_tokens {
                norte_code_wire::Reasoning::Budget {
                    tokens: u32::try_from(tokens).map_err(|_| {
                        HostError::InvalidFrame(
                            "reasoning.budget_tokens is out of range".to_owned(),
                        )
                    })?,
                }
            } else if let Some(effort) = effort {
                norte_code_wire::Reasoning::Effort {
                    effort: effort.to_owned(),
                }
            } else {
                norte_code_wire::Reasoning::Effort {
                    effort: "medium".to_owned(),
                }
            }
        }
        other => {
            return Err(HostError::InvalidFrame(format!(
                "reasoning.mode `{other}` is not supported"
            )));
        }
    })
}

/// Decodes a `budgets` override object into a [`norte_code_wire::Budget`].
///
/// The wire shape follows the `AgentSpec` fragment: `max_turn_cost` /
/// `max_session_cost` (with `currency`, default `USD`) and `max_turn_tokens`
/// map to `BudgetUnit::Usd` / `BudgetUnit::Tokens`. The service's current
/// budget semantics are a single per-session limit, so the first field
/// present wins; a field this host does not map fails instead of being
/// silently dropped.
pub(crate) fn decode_budget_override(value: &Value) -> Result<norte_code_wire::Budget, HostError> {
    let Some(fields) = value.as_object() else {
        return Err(HostError::InvalidFrame(
            "budget override must be an object".to_owned(),
        ));
    };
    for key in fields.keys() {
        match key.as_str() {
            "max_turn_cost" | "max_session_cost" | "currency" | "max_turn_tokens"
            | "max_session_tokens" | "on_exceed" => {}
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "budget override field `{other}` is not supported"
                )));
            }
        }
    }
    let action = match fields
        .get("on_exceed")
        .and_then(Value::as_str)
        .unwrap_or("stop")
    {
        "ask" => norte_code_wire::BudgetAction::Ask,
        "degrade" | "warn" => norte_code_wire::BudgetAction::Warn,
        _ => norte_code_wire::BudgetAction::Stop,
    };
    if let Some(cost) = fields
        .get("max_turn_cost")
        .or_else(|| fields.get("max_session_cost"))
        .and_then(Value::as_f64)
        .or_else(|| {
            fields
                .get("max_turn_cost")
                .or_else(|| fields.get("max_session_cost"))
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok().map(f64::from))
        })
    {
        return Ok(norte_code_wire::Budget {
            unit: norte_code_wire::BudgetUnit::Usd,
            limit: cost,
            action,
        });
    }
    if let Some(tokens) = fields
        .get("max_turn_tokens")
        .or_else(|| fields.get("max_session_tokens"))
        .and_then(Value::as_u64)
    {
        return Ok(norte_code_wire::Budget {
            unit: norte_code_wire::BudgetUnit::Tokens,
            limit: f64::from(u32::try_from(tokens).unwrap_or(u32::MAX)),
            action,
        });
    }
    Err(HostError::InvalidFrame(
        "budget override requires a cost or token limit".to_owned(),
    ))
}

/// A `SESSION_CONFIGURE` payload as this host accepts it (RFC 8.1).
///
/// The payload is the standard one — `agent_id`, `agent_override`,
/// `options`, `apply` — not a serialised [`AgentCommand`]. The host
/// translates its known fields onto service requests; anything it cannot
/// express fails with `AGENT_OVERRIDE_DENIED` instead of being ignored.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub(crate) struct SessionConfigurePayload {
    /// Switch to another agent. Only honoured when it names the host's
    /// configured agent; anything else is `agent_mismatch`.
    #[serde(default)]
    agent_id: Option<String>,
    /// Per-session configuration fragment. See [`decode_override`].
    #[serde(default)]
    agent_override: Option<Value>,
    /// Session options supported by this host.
    #[serde(default)]
    options: Option<Value>,
    /// When the change applies. This host supports immediate application only.
    #[serde(default)]
    apply: Option<ConfigureApplyPolicy>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigureApplyPolicy {
    Now,
    NextTurn,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionOptions {
    #[serde(default)]
    job_turn_policy: Option<norte_code_wire::JobTurnPolicy>,
}

pub(crate) fn parse_job_turn_policy(
    options: Option<&Value>,
) -> Result<Option<norte_code_wire::JobTurnPolicy>, HostError> {
    let Some(options) = options else {
        return Ok(None);
    };
    let options: SessionOptions = serde_json::from_value(options.clone()).map_err(|error| {
        HostError::InvalidFrame(format!("SESSION_OPTIONS_UNSUPPORTED: {error}"))
    })?;
    Ok(options.job_turn_policy)
}

/// Host configuration independent of the HTTP listener.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Agent identifier accepted during attachment.
    pub agent_id: String,
    /// Advertised server version.
    pub server_version: String,
    /// Broadcast channel capacity for live frames.
    pub replay_capacity: usize,
    /// Directory holding one replay file per session.
    pub replay_dir: std::path::PathBuf,
    /// Maximum bytes retained per session's replay file.
    pub replay_bytes: u64,
    /// Maximum number of concurrently running jobs per session.
    pub max_concurrent_jobs: usize,
    /// Duration for which terminal job output remains queryable.
    pub job_output_retention_s: u64,
    /// Maximum number of command results retained for idempotent retries.
    pub idempotency_capacity: usize,
    /// Maximum user inputs queued behind an active turn.
    pub max_queued_inputs: usize,
    /// Maximum frame size advertised to clients.
    pub max_frame_bytes: u64,
    /// Optional bearer token required by operational HTTP routes.
    pub auth_token: Option<String>,
    /// Optional bearer-token to principal mappings for multi-user ownership.
    pub auth_principals: HashMap<String, String>,
    /// Browser origins allowed to call the HTTP API.
    ///
    /// A request that carries `Origin` is rejected unless the value is listed
    /// here. A non-browser client sends no `Origin` and is unaffected.
    pub allowed_origins: Vec<String>,
    /// Maximum size accepted for one uploaded artifact.
    pub max_artifact_bytes: usize,
    /// Maximum size of one portable session bundle.
    pub max_export_bytes: usize,
    /// Lifetime of generated portable bundles.
    pub export_ttl_s: u64,
    /// Maximum concurrent WebSocket and SSE connections.
    pub max_connections: usize,
    /// Maximum time spent handling one command, in milliseconds.
    pub command_timeout_ms: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            agent_id: "norte-code".to_owned(),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            replay_capacity: 10_000,
            replay_dir: std::path::PathBuf::from("norte-code-replay"),
            replay_bytes: 8 * 1024 * 1024,
            max_concurrent_jobs: 8,
            job_output_retention_s: 86_400,
            idempotency_capacity: 4_096,
            max_queued_inputs: 8,
            max_frame_bytes: 1024 * 1024,
            auth_token: None,
            auth_principals: HashMap::new(),
            allowed_origins: vec![
                "http://localhost:8080".to_owned(),
                "http://127.0.0.1:8080".to_owned(),
            ],
            max_artifact_bytes: 16 * 1024 * 1024,
            max_export_bytes: 64 * 1024 * 1024,
            export_ttl_s: 3600,
            max_connections: 256,
            command_timeout_ms: 120_000,
        }
    }
}

/// Errors raised while translating or executing a RAEP frame.
#[derive(Debug, Error)]
pub enum HostError {
    /// The frame is invalid or cannot be used in the current connection state.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    /// The client sent a frame type this host does not implement.
    #[error("unknown frame type: {0}")]
    UnknownType(String),
    /// A frame id or turn id was reused for a different request.
    #[error("idempotency conflict: {0}")]
    IdempotencyConflict(String),
    /// A turn was requested while the session was occupied under reject policy.
    #[error("session is busy")]
    SessionBusy,
    /// A cancellation targeted a turn that is not active.
    #[error("turn is not active")]
    TurnNotActive,
    /// The Norte Code service rejected a request before streaming it.
    #[error("service request failed: {0}")]
    Service(String),
    /// Replay is unavailable for the requested sequence.
    #[error("replay unavailable: {0}")]
    Replay(String),
    /// A frame could not be serialized.
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The durable replay journal could not be read or written.
    #[error("replay journal failed: {0}")]
    ReplayJournal(String),
    /// An upload could not be stored or resolved.
    #[error("artifact failed: {0}")]
    Artifact(String),
}

/// A Norte Code service exposed through the RAEP wire.
pub struct NorteCodeRaepHost {
    service: Arc<Service>,
    services: HashMap<String, Arc<Service>>,
    config: HostConfig,
    state: Mutex<HostState>,
    events: broadcast::Sender<Envelope<Value>>,
    replay: ReplayLog,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    upload_index: Option<Arc<dyn UploadIndex>>,
    pin_store: Option<Arc<dyn PinStore>>,
    connections: Arc<Semaphore>,
}

pub(crate) fn summarize_plugin_updates(
    results: Vec<(String, Result<String, String>)>,
) -> Result<String, String> {
    let updated = results
        .iter()
        .filter_map(|(_, result)| result.as_ref().ok())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let failures = results
        .into_iter()
        .filter_map(|(plugin, result)| result.err().map(|error| format!("{plugin}: {error}")))
        .collect::<Vec<_>>();
    if failures.is_empty() {
        return Ok(updated);
    }
    let successful = if updated.is_empty() {
        String::new()
    } else {
        format!(" Updated successfully: {updated}.")
    };
    Err(format!(
        "Plugin update failed: {}.{successful}",
        failures.join("; ")
    ))
}

impl NorteCodeRaepHost {
    /// Creates a host around the server-owned Norte Code service.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured agent identifier is empty or the
    /// replay capacity is zero.
    pub fn new(service: Arc<Service>, config: HostConfig) -> Result<Self, HostError> {
        if config.agent_id.is_empty() {
            return Err(HostError::InvalidFrame(
                "agent_id must not be empty".to_owned(),
            ));
        }
        if config.replay_capacity == 0 {
            return Err(HostError::InvalidFrame(
                "replay_capacity must be positive".to_owned(),
            ));
        }
        if config.max_queued_inputs == 0 {
            return Err(HostError::InvalidFrame(
                "max_queued_inputs must be positive".to_owned(),
            ));
        }
        if config.max_connections == 0 {
            return Err(HostError::InvalidFrame(
                "max_connections must be positive".to_owned(),
            ));
        }
        if config.max_export_bytes == 0 || config.export_ttl_s == 0 {
            return Err(HostError::InvalidFrame(
                "export size and TTL limits must be positive".to_owned(),
            ));
        }
        service.configure_job_limits(config.max_concurrent_jobs, config.job_output_retention_s);
        let max_connections = config.max_connections;
        let (events, _) = broadcast::channel(config.replay_capacity.max(16));
        let replay = ReplayLog::new(config.replay_dir.clone(), config.replay_bytes);
        Ok(Self {
            services: HashMap::from([(config.agent_id.clone(), Arc::clone(&service))]),
            service,
            config,
            state: Mutex::new(HostState::default()),
            events,
            replay,
            artifact_store: None,
            upload_index: None,
            pin_store: None,
            connections: Arc::new(Semaphore::new(max_connections)),
        })
    }

    /// Creates a host exposing several named agent services.
    ///
    /// The entry named by `config.agent_id` is the default used by
    /// connection-scoped requests and by clients that do not name an agent.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured default agent is not present.
    pub fn with_agents(
        services: HashMap<String, Arc<Service>>,
        config: HostConfig,
    ) -> Result<Self, HostError> {
        let service = services.get(&config.agent_id).cloned().ok_or_else(|| {
            HostError::InvalidFrame(format!(
                "no service configured for agent `{}`",
                config.agent_id
            ))
        })?;
        let mut host = Self::new(service, config)?;
        for service in services.values() {
            service.configure_job_limits(
                host.config.max_concurrent_jobs,
                host.config.job_output_retention_s,
            );
        }
        host.services = services;
        Ok(host)
    }

    /// Moves the replay log to another directory.
    #[must_use]
    pub fn with_replay_log(mut self, log: ReplayLog) -> Self {
        self.replay = log;
        self
    }

    /// Enables durable storage for client-uploaded artifacts.
    #[must_use]
    pub fn with_artifact_store(mut self, artifact_store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(artifact_store);
        self
    }

    /// Enables session-scoped uploads with durable metadata.
    #[must_use]
    pub fn with_upload_index(mut self, upload_index: Arc<dyn UploadIndex>) -> Self {
        self.upload_index = Some(upload_index);
        self
    }

    /// Enables durable pin storage for `CONTEXT_PIN`.
    #[must_use]
    pub fn with_pin_store(mut self, pin_store: Arc<dyn PinStore>) -> Self {
        self.pin_store = Some(pin_store);
        self
    }

    /// Returns a receiver for live frames emitted by any attached connection.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope<Value>> {
        self.events.subscribe()
    }

    /// Spawns the drain from the service's delegation events into this
    /// host's frame broadcast (RFC 8.2.10).
    ///
    /// The service emits `ChildSessionStarted/Updated` and
    /// `DelegatorStateChanged` on its own broadcast when a child is spawned
    /// or its delegator is suspended; the host turns each one into the
    /// standard frame on the parent's stream, with the same `seq` ordering
    /// as every other frame of that session. Without this drain the events
    /// would sit in the service's channel with no reader.
    ///
    /// # Errors
    ///
    /// Returns an error when the delegation receiver has already been taken:
    /// the host takes it exactly once, at startup.
    pub fn spawn_delegation_drain(self: &Arc<Self>) -> Result<(), HostError> {
        let receiver = self.service.delegation_rx_take().ok_or_else(|| {
            HostError::InvalidFrame("delegation receiver already taken".to_owned())
        })?;
        let host = Arc::clone(self);
        tokio::spawn(async move {
            let mut receiver = receiver;
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        let session_id = event
                            .session
                            .as_ref()
                            .map_or_else(String::new, |key| key.as_str().to_owned());
                        if session_id.is_empty() {
                            continue;
                        }
                        if let Err(error) = host.emit_event(&session_id, event).await {
                            tracing::warn!(%error, session = %session_id, "delegation event could not be emitted");
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(lost)) => {
                        tracing::warn!(lost, "delegation event stream lagged; frames were dropped");
                    }
                }
            }
        });
        Ok(())
    }

    async fn current_seq(&self, session_id: &str) -> u64 {
        self.state
            .lock()
            .await
            .sequences
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }
}

pub(crate) fn turn_id_for_event(kind: &ServerEventKind) -> Option<u64> {
    match kind {
        ServerEventKind::TurnStarted { turn }
        | ServerEventKind::Delta { turn, .. }
        | ServerEventKind::MediaPartCompleted { turn, .. }
        | ServerEventKind::ToolStarted { turn, .. }
        | ServerEventKind::ToolOutput { turn, .. }
        | ServerEventKind::ToolFinished { turn, .. }
        | ServerEventKind::RequestReconstructed { turn, .. }
        | ServerEventKind::TurnFinished { turn, .. }
        | ServerEventKind::QuestionAsked { turn, .. }
        | ServerEventKind::RetryScheduled { turn, .. }
        | ServerEventKind::RetryStarted { turn, .. } => Some(*turn),
        _ => None,
    }
}

pub(crate) fn job_id_for_event(kind: &ServerEventKind) -> Option<String> {
    match kind {
        ServerEventKind::JobStarted { job_id, .. }
        | ServerEventKind::JobProgressDelta { job_id, .. } => Some(job_id.clone()),
        ServerEventKind::JobFinished { job, .. } => Some(job.id.clone()),
        _ => None,
    }
}

pub(crate) fn frame_type_for_event(kind: &ServerEventKind) -> FrameType {
    match kind {
        ServerEventKind::Delta { .. } => standard(StandardFrameType::TokenDelta),
        ServerEventKind::MediaPartCompleted { .. } => {
            standard(StandardFrameType::OutputPartFinished)
        }
        ServerEventKind::ToolStarted { .. } => standard(StandardFrameType::ToolCallStarted),
        ServerEventKind::ToolOutput { .. } => standard(StandardFrameType::ToolOutputDelta),
        ServerEventKind::ToolFinished { .. } => standard(StandardFrameType::ToolCallFinished),
        ServerEventKind::JobStarted { .. } => standard(StandardFrameType::JobStarted),
        ServerEventKind::JobProgressDelta { .. } => standard(StandardFrameType::JobProgressDelta),
        ServerEventKind::JobFinished { .. } => standard(StandardFrameType::JobFinished),
        ServerEventKind::TurnStarted { .. }
        | ServerEventKind::TurnFinished { .. }
        | ServerEventKind::TurnStopped { .. } => standard(StandardFrameType::TurnStateChanged),
        ServerEventKind::JobList { .. } => standard(StandardFrameType::JobListResult),
        ServerEventKind::MessageReplayed { .. } => standard(StandardFrameType::HistoryPage),
        ServerEventKind::Error { .. } => standard(StandardFrameType::Error),
        ServerEventKind::SteerApplied { .. } => standard(StandardFrameType::UserInputAccepted),
        ServerEventKind::ChildSessionStarted { .. } => {
            standard(StandardFrameType::ChildSessionStarted)
        }
        ServerEventKind::ChildSessionUpdated { .. }
        | ServerEventKind::ChildAwaitingInput { .. } => {
            standard(StandardFrameType::ChildSessionUpdated)
        }
        ServerEventKind::DelegatorStateChanged { .. } => {
            standard(StandardFrameType::DelegatorStateChanged)
        }
        other => FrameType::Extension(extension_name_for_event(other)),
    }
}

/// Names a `NorteCode` event that has no faithful standard frame.
///
/// Extensions carry the adapter's own payload, so a standard frame is never
/// sent with a shape the standard does not define.
pub(crate) fn extension_name_for_event(kind: &ServerEventKind) -> String {
    let tag = serde_json::to_value(kind)
        .ok()
        .and_then(|value| {
            value
                .get("event")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "event".to_owned());
    format!("norte_code.{tag}")
}

pub(crate) fn session_for_request(kind: &ClientRequestKind) -> Option<&SessionKey> {
    match kind {
        ClientRequestKind::RenameSession { session, .. }
        | ClientRequestKind::DeleteSession { session }
        | ClientRequestKind::Steer { session, .. }
        | ClientRequestKind::CancelTurn { session }
        | ClientRequestKind::StopJobs { session, .. }
        | ClientRequestKind::DetachTurn { session }
        | ClientRequestKind::ListJobs { session }
        | ClientRequestKind::ListPlans { session }
        | ClientRequestKind::Resume { session, .. }
        | ClientRequestKind::AnswerQuestion { session, .. }
        | ClientRequestKind::SetModel { session, .. }
        | ClientRequestKind::SetReasoning { session, .. }
        | ClientRequestKind::SetBudget { session, .. }
        | ClientRequestKind::Compact { session }
        | ClientRequestKind::ForkSession { session, .. }
        | ClientRequestKind::ListTools {
            session: Some(session),
        }
        | ClientRequestKind::SetApprovalRule {
            session: Some(session),
            ..
        }
        | ClientRequestKind::ForgetApprovalRule {
            session: Some(session),
            ..
        }
        | ClientRequestKind::SessionCompact { session, .. }
        | ClientRequestKind::ContextPin { session, .. }
        | ClientRequestKind::SessionRewind { session, .. }
        | ClientRequestKind::HistoryGet { session, .. }
        | ClientRequestKind::SessionExport { session, .. } => Some(session),
        ClientRequestKind::SendMessage { session, .. }
        | ClientRequestKind::CostReport { session }
        | ClientRequestKind::ListTelemetry { session } => session.as_ref(),
        ClientRequestKind::CreateSession { .. }
        | ClientRequestKind::ListSessions
        | ClientRequestKind::ListModels
        | ClientRequestKind::ListCatalog
        | ClientRequestKind::ListTools { session: None }
        | ClientRequestKind::SetApprovalRule { session: None, .. }
        | ClientRequestKind::ForgetApprovalRule { session: None, .. }
        | ClientRequestKind::RefreshCatalog
        | ClientRequestKind::CatalogUpsertProvider { .. }
        | ClientRequestKind::CatalogRemoveProvider { .. }
        | ClientRequestKind::CatalogUpsertModel { .. }
        | ClientRequestKind::CatalogRemoveModel { .. }
        | ClientRequestKind::ListSkills
        | ClientRequestKind::RefreshSkills
        | ClientRequestKind::ListPlugins
        | ClientRequestKind::InstallPlugin { .. }
        | ClientRequestKind::UpdatePlugins { .. }
        | ClientRequestKind::RemovePlugin { .. }
        | ClientRequestKind::SetPluginEnabled { .. }
        | ClientRequestKind::ListMcp
        | ClientRequestKind::RefreshMcp
        | ClientRequestKind::SetMcpActive { .. }
        | ClientRequestKind::SessionImport { .. } => None,
    }
}

/// Whether a command only reads state and can run without an attachment.
pub(crate) fn is_read_only_request(kind: &ClientRequestKind) -> bool {
    matches!(
        kind,
        ClientRequestKind::ListTelemetry { .. }
            | ClientRequestKind::CostReport { .. }
            | ClientRequestKind::ListJobs { .. }
            | ClientRequestKind::ListModels
            | ClientRequestKind::ListCatalog
            | ClientRequestKind::ListSkills
            | ClientRequestKind::ListMcp
            | ClientRequestKind::ListTools { .. }
            | ClientRequestKind::ListPlans { .. }
            | ClientRequestKind::ListSessions
    )
}

pub(crate) fn is_plugin_request(kind: &ClientRequestKind) -> bool {
    matches!(
        kind,
        ClientRequestKind::ListPlugins
            | ClientRequestKind::InstallPlugin { .. }
            | ClientRequestKind::UpdatePlugins { .. }
            | ClientRequestKind::RemovePlugin { .. }
            | ClientRequestKind::SetPluginEnabled { .. }
    )
}

pub(crate) fn standard(kind: StandardFrameType) -> FrameType {
    FrameType::Standard(kind)
}

pub(crate) fn observer_read_only_frame(kind: &FrameType) -> bool {
    matches!(
        kind,
        FrameType::Standard(
            StandardFrameType::Ack
                | StandardFrameType::HistoryGet
                | StandardFrameType::JobList
                | StandardFrameType::JobGet
        )
    )
}

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

pub(super) fn job_status(job: &norte_code_wire::JobInfo) -> JobStatus {
    match job.state {
        norte_code_wire::JobState::Running => JobStatus::Running,
        norte_code_wire::JobState::Cancelled => JobStatus::Killed,
        norte_code_wire::JobState::Finished if job.timed_out => JobStatus::TimedOut,
        norte_code_wire::JobState::Finished
            if job.exit_code.is_some_and(|exit_code| exit_code != 0) =>
        {
            JobStatus::Failed
        }
        norte_code_wire::JobState::Finished => JobStatus::Completed,
    }
}

pub(crate) fn job_output_from_index(
    job: &norte_code_wire::JobInfo,
    from_index: u64,
) -> Result<&str, HostError> {
    let retained_bytes = u64::try_from(job.output.len()).unwrap_or(u64::MAX);
    let retained_from = job.output_bytes_total.saturating_sub(retained_bytes);
    if from_index < retained_from {
        return Err(HostError::InvalidFrame("JOB_OUTPUT_EXPIRED".to_owned()));
    }
    if from_index > job.output_bytes_total {
        return Err(HostError::InvalidFrame(
            "JOB_GET.from_index exceeds available output".to_owned(),
        ));
    }
    let relative = usize::try_from(from_index - retained_from)
        .map_err(|_| HostError::InvalidFrame("JOB_GET.from_index is out of range".to_owned()))?;
    job.output.get(relative..).ok_or_else(|| {
        HostError::InvalidFrame("JOB_GET.from_index is not a UTF-8 boundary".to_owned())
    })
}

pub(super) fn job_finished_payload(
    job: &norte_code_wire::JobInfo,
    triggered_new_turn: bool,
    new_turn_id: Option<&str>,
    injected_into_turn_id: Option<&str>,
) -> Value {
    let start = job
        .output
        .char_indices()
        .find_map(|(index, _)| {
            (job.output.len().saturating_sub(16 * 1024) <= index).then_some(index)
        })
        .unwrap_or(0);
    let output_snapshot = &job.output[start..];
    payload(norte_code_wire::JobFinished {
        job_id: Some(job.id.clone()),
        status: job_status(job),
        exit_code: job.exit_code,
        signal: None,
        summary: job.label.clone(),
        output_snapshot: output_snapshot.to_owned(),
        output_truncated: job.dropped || start > 0,
        output_bytes_total: job.output_bytes_total,
        duration_ms: job.elapsed_ms,
        triggered_new_turn,
        new_turn_id: new_turn_id.map(str::to_owned),
        injected_into_turn_id: injected_into_turn_id.map(str::to_owned),
    })
}

pub(crate) fn parse_portable_history(
    bytes: &[u8],
) -> Result<Vec<norte_code_agent::service::ImportedHistoryItem>, HostError> {
    let mut items = Vec::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let item: PortableHistoryItem = serde_json::from_slice(line)?;
        if item.rewound {
            return Err(HostError::InvalidFrame(
                "BUNDLE_UNSUPPORTED: rewound history cannot be restored by this host".to_owned(),
            ));
        }
        let content = match item.content {
            Value::String(content) => content,
            Value::Array(parts) => {
                let mut content = String::new();
                for part in parts {
                    if part.get("type").and_then(Value::as_str) != Some("text") {
                        return Err(HostError::InvalidFrame(
                            "BUNDLE_UNSUPPORTED: non-text history parts are not supported"
                                .to_owned(),
                        ));
                    }
                    let text = part.get("text").and_then(Value::as_str).ok_or_else(|| {
                        HostError::InvalidFrame(
                            "BUNDLE_INVALID: text content part has no text".to_owned(),
                        )
                    })?;
                    content.push_str(text);
                }
                content
            }
            _ => {
                return Err(HostError::InvalidFrame(
                    "BUNDLE_INVALID: history content must be text".to_owned(),
                ));
            }
        };
        items.push(norte_code_agent::service::ImportedHistoryItem {
            role: item.role.as_str().to_owned(),
            content,
            turn_id: item.turn_id,
            timestamp: item.timestamp,
            origin: item.origin.or(item.provenance),
        });
        if items.len() > 100_000 {
            return Err(HostError::InvalidFrame(
                "BUNDLE_INVALID: too many history items".to_owned(),
            ));
        }
    }
    Ok(items)
}

pub(crate) fn validate_bundle_usage(usage: &Value) -> Result<(), HostError> {
    let Some(object) = usage.as_object() else {
        return Err(HostError::InvalidFrame(
            "BUNDLE_INVALID: usage_total must be an object".to_owned(),
        ));
    };
    for key in [
        "input_tokens",
        "output_tokens",
        "cached_tokens",
        "reasoning_tokens",
        "tool_calls",
        "duration_ms",
    ] {
        if object
            .get(key)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(HostError::InvalidFrame(format!(
                "BUNDLE_INVALID: usage_total.{key} must be a non-negative integer"
            )));
        }
    }
    if let Some(cost) = object.get("cost") {
        let valid = cost
            .get("amount")
            .and_then(Value::as_f64)
            .is_some_and(|amount| amount >= 0.0)
            && cost
                .get("currency")
                .and_then(Value::as_str)
                .is_some_and(|currency| currency == "USD");
        if !valid {
            return Err(HostError::InvalidFrame(
                "BUNDLE_UNSUPPORTED: usage_total.cost must use USD".to_owned(),
            ));
        }
    }
    let extra = object
        .get("extra")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let Some(extra) = extra.as_object() else {
        return Err(HostError::InvalidFrame(
            "BUNDLE_INVALID: usage_total.extra must be an object".to_owned(),
        ));
    };
    if extra
        .keys()
        .any(|key| key != "requests" && key != "unpriced_requests")
        || ["reasoning_tokens", "tool_calls", "duration_ms"]
            .into_iter()
            .any(|key| object.get(key).and_then(Value::as_u64).unwrap_or(0) > 0)
    {
        return Err(HostError::InvalidFrame(
            "BUNDLE_UNSUPPORTED: usage_total has counters this host cannot preserve".to_owned(),
        ));
    }
    if ["requests", "unpriced_requests"]
        .into_iter()
        .any(|key| extra.get(key).is_some_and(|value| value.as_u64().is_none()))
    {
        return Err(HostError::InvalidFrame(
            "BUNDLE_INVALID: usage_total.extra counters must be non-negative integers".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_export_request(request: &ExportRequest) -> Result<(), HostError> {
    if request
        .quiesce
        .is_some_and(|policy| policy != QuiescePolicy::Reject)
    {
        return Err(HostError::InvalidFrame(
            "SESSION_EXPORT currently supports only quiesce=reject".to_owned(),
        ));
    }
    if request.wait_timeout_s.is_some() {
        return Err(HostError::InvalidFrame(
            "wait_timeout_s is only valid with quiesce=wait".to_owned(),
        ));
    }
    if request.encrypt.is_some() {
        return Err(HostError::InvalidFrame(
            "encrypted exports are not supported".to_owned(),
        ));
    }
    if request.include.children
        || request.include.uploads
        || request.include.job_output
        || request.include.outputs
        || request.include.workspace
    {
        return Err(HostError::InvalidFrame(
            "SESSION_EXPORT requires children, uploads, job_output, outputs, and workspace all false"
                .to_owned(),
        ));
    }
    Ok(())
}

pub(crate) type PortableHistoryEntry = (
    Option<u64>,
    norte_code_wire::MessageRole,
    norte_code_wire::ReplayedKind,
    String,
    i64,
    Option<Value>,
);

pub(crate) fn serialize_plain_text_history(
    history: Vec<PortableHistoryEntry>,
    max_bytes: usize,
) -> Result<Vec<u8>, HostError> {
    let mut bytes = Vec::new();
    for (turn, role, kind, content, timestamp, origin) in history {
        // The bundle carries text, and a conversation is more than text: the
        // model reasons, calls tools, and reads their results. Exporting only
        // refused those, so any real session could not be exported at all.
        // Each non-text part is written as the assistant text it already
        // renders as (`name {json}` for a call), and tool output is dropped
        // because it is reproducible from the call.
        let role = match (role, kind) {
            (norte_code_wire::MessageRole::User, norte_code_wire::ReplayedKind::Text) => {
                ExportHistoryRole::User
            }
            (norte_code_wire::MessageRole::Assistant, _) => ExportHistoryRole::Assistant,
            // A user row that is not text has no counterpart in the bundle's
            // two roles; tool output is reproducible from the call; and the
            // system prompt is not the conversation. All of them are dropped
            // rather than failing the whole export.
            _ => continue,
        };
        let item = HistoryExportItem {
            role,
            content: &content,
            turn_id: turn.map(|turn| format!("turn_{turn}")),
            timestamp,
            origin,
        };
        serde_json::to_writer(&mut bytes, &item)?;
        bytes.push(b'\n');
        if bytes.len() > max_bytes {
            return Err(HostError::InvalidFrame(
                "PAYLOAD_TOO_LARGE: export exceeds max_export_bytes".to_owned(),
            ));
        }
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
