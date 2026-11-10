//! Turning an attachment into the media part the model sees.

use norte::{MediaKind, MediaPart, MediaSource, MimeType};

use norte_code_wire::{Attachment, ErrorCode};

use super::Service;
impl Service {
    /// One attachment as the media part the model sees.
    ///
    /// The bytes never travel on the wire: the client that pasted them put them
    /// in the blob store both ends share and named the blob. This reads the id
    /// back and hands the engine a part whose source is that blob; the media
    /// adapter is what turns it into bytes on the way to the provider.
    ///
    /// A blob the store does not hold is refused rather than sent: a part nobody
    /// can resolve would reach the provider as an id it never heard of.
    pub(super) async fn media_part(
        &self,
        attachment: &Attachment,
    ) -> std::result::Result<Option<MediaPart>, String> {
        let mime = MimeType::new(attachment.mime.clone())
            .map_err(|_| format!("attachment: `{}` is not a media type", attachment.mime))?;
        // The wire carries the digest as text; the store speaks its parsed id.
        let id = if let Some(hex) = &attachment.blob {
            let id = hex
                .parse::<norte::BlobId>()
                .map_err(|_| format!("attachment: `{hex}` is not a blob id"))?;
            if !self
                .blobs
                .contains(id)
                .await
                .map_err(|error| error.to_string())?
            {
                return Err(format!("attachment: blob `{id}` was not found"));
            }
            id
        } else {
            let bytes = tokio::fs::read(&attachment.path).await.map_err(|error| {
                format!("attachment: could not read `{}`: {error}", attachment.path)
            })?;
            self.blobs
                .put(&bytes)
                .await
                .map_err(|error| error.to_string())?
        };
        let mut part = MediaPart::new(kind_of_mime(&attachment.mime), MediaSource::Blob(id), mime);
        part.metadata.duration_ms = attachment.duration_ms;
        part.metadata.transcript.clone_from(&attachment.transcript);
        Ok(Some(part))
    }
}

/// What a media part's kind is, from the MIME type the client sent.
///
/// The client names a file's media type, not a [`MediaKind`], and the kind is
/// what the capability check reads. Deriving it in one place is what keeps the
/// check and the part that is actually sent in step: a JPEG declared `image/jpeg`
/// is an image to both, and a `pdf` is a document to both.
///
/// Both callers use this: [`unsupported_attachment`] reads the kind to refuse a
/// part the model cannot take, and [`Service::media_part`] stamps it on the part
/// that is sent. A single derivation is what makes those two the same answer -
/// it was `Image` unconditionally on the way out while the check derived the
/// kind, so a PDF was checked as a document and sent as an image.
pub(super) fn kind_of_mime(mime: &str) -> MediaKind {
    let top = mime.split_once('/').map_or(mime, |(top, _)| top);
    match top {
        "image" => MediaKind::Image,
        "audio" => MediaKind::Audio,
        "video" => MediaKind::Video,
        _ => MediaKind::Document,
    }
}

/// The first attachment this model cannot take, as an error code and a message
/// naming it.
///
/// Checked against the model's own capabilities before the turn commits
/// anything. The message names the modality and the model, because the two
/// things a person needs to act are what was refused and what refused it; the
/// way out: declare it under the provider's `models` block or use an
/// image-capable model: is in the message for the same reason.
pub(super) fn unsupported_attachment(
    capabilities: &norte::Capabilities,
    attachments: &[Attachment],
) -> Option<(ErrorCode, String)> {
    for attachment in attachments {
        let modality = norte::Modality::from(kind_of_mime(&attachment.mime));
        if capabilities.modalities.supports_input(modality) {
            continue;
        }
        return Some((
            ErrorCode::Capability,
            format!(
                "the model does not accept {modality:?} input, so `{}` was not sent; \
                 use a model that takes {modality:?}, or declare `modalities.input` for \
                 it under the provider's `models` block",
                attachment.name
            ),
        ));
    }
    None
}

#[cfg(test)]
#[path = "turn_media_tests.rs"]
mod tests;
