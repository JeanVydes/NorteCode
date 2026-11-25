//! Validation and modality mapping for user-input payloads.

use serde_json::Value;

use super::capabilities::MAX_MEDIA_DURATION_MS;
use super::{
    AcceptedContentPart, AcceptedPartKind, AcceptedResponseFormat, AcceptedUserInput, HostError,
    ImageDetail, ResponseFormatKind,
};

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
