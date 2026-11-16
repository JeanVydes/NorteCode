//! The engine's media part as the wire carries it.
//!
//! The engine type lives in `norte` and the wire type in `norte-code-wire`, and
//! the wire crate deliberately does not depend on the engine: the browser
//! client compiles the wire, not the engine. This is the one place the two
//! meet on the way out, so a field added to either is a compile error here
//! instead of a quietly dropped value.

use norte::{MediaKind, MediaPart, MediaSource};

/// The wire's media part, or `None` for a kind the wire cannot name.
///
/// `norte`'s enum is non-exhaustive so a newer engine may produce a kind this
/// build has never heard of; that part is not emitted rather than sent as a
/// lie.
#[must_use]
pub fn media_part(media: &MediaPart) -> Option<norte_code_wire::MediaPart> {
    let kind = match media.kind {
        MediaKind::Image => norte_code_wire::MediaKind::Image,
        MediaKind::Audio => norte_code_wire::MediaKind::Audio,
        MediaKind::Video => norte_code_wire::MediaKind::Video,
        MediaKind::Document => norte_code_wire::MediaKind::Document,
        _ => return None,
    };
    let source = match &media.source {
        MediaSource::Blob(id) => norte_code_wire::MediaSource::Blob(id.to_string()),
        MediaSource::Url(url) => norte_code_wire::MediaSource::Url(url.clone()),
        MediaSource::ProviderFile(reference) => {
            norte_code_wire::MediaSource::ProviderFile(norte_code_wire::ProviderFileRef {
                provider: reference.provider.to_string(),
                file_id: reference.file_id.clone(),
                expires_at: reference.expires_at.map(|at| at.to_rfc3339()),
                blob: reference.blob.map(|blob| blob.to_string()),
            })
        }
        MediaSource::Inline(bytes) => norte_code_wire::MediaSource::Inline(bytes.clone()),
        _ => return None,
    };
    Some(norte_code_wire::MediaPart {
        kind,
        source,
        mime: media.mime.as_str().to_owned(),
        metadata: norte_code_wire::MediaMetadata {
            duration_ms: media.metadata.duration_ms,
            width: media.metadata.width,
            height: media.metadata.height,
            transcript: media.metadata.transcript.clone(),
            extension: media.metadata.extension.clone(),
        },
    })
}
