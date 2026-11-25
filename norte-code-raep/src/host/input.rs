//! Accepted RAEP user-input and content shapes.

use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct AcceptedContentPart {
    #[serde(rename = "type")]
    pub(super) kind: AcceptedPartKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<AcceptedSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) alt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) detail: Option<ImageDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) pages: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) transcript: Option<String>,
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
    pub(super) fn as_str(self) -> &'static str {
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
    pub(super) prompt: Option<String>,
    /// Multimodal input. Exactly one of `prompt` and `content` is required.
    #[serde(default)]
    pub(super) content: Option<Vec<AcceptedContentPart>>,
    /// Behavior when another turn is active.
    #[serde(default)]
    pub(super) queue_policy: Option<raep_types::QueuePolicy>,
    /// Per-turn configuration overrides. See [`decode_override`]: the known
    /// fields are applied to this turn; unknown fields fail instead of being
    /// silently dropped.
    #[serde(default)]
    pub(super) overrides: Option<Value>,
    /// Output modalities the client wants back. This host only produces text:
    /// anything other than `["text"]` is rejected.
    #[serde(default)]
    pub(super) output: Option<Vec<raep_types::Modality>>,
    /// The shape the final answer must take. Hardcoded to `text` for this
    /// host: `json_schema` is rejected until structured output exists.
    #[serde(default)]
    pub(super) response_format: Option<AcceptedResponseFormat>,
    /// Rewind-and-re-run in one step (RFC 18.2). Not supported yet.
    #[serde(default)]
    pub(super) replaces_turn_id: Option<String>,
    /// Arbitrary caller metadata: an opaque object of at most 4 KiB, stored
    /// with the turn and never shown to the model (RFC 12.4). Validated here
    /// for size; the service's turn record has no slot for it yet, so it is
    /// not persisted.
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) metadata: Option<Value>,
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
    pub(super) kind: ResponseFormatKind,
    /// JSON Schema 2020-12, at most 16 KiB, local `$ref` only. Only present for
    /// `json_schema`.
    #[serde(default)]
    pub(super) schema: Option<Value>,
    /// Whether the turn fails when the answer does not validate, instead of
    /// earning one correction message.
    #[serde(default)]
    pub(super) strict: bool,
}
