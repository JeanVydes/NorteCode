//! Gate every upload on the host and active model capabilities.

use super::error::AttachmentError;
use super::kind::AttachmentKind;
use super::mime::MimeType;
use crate::ui_model::UiModel;

/// Verify that the active host and model can take this media type.
///
/// An unknown model is refused. A client must not guess what a model accepts.
pub(crate) fn validate(
    model: &UiModel,
    mime: &MimeType,
) -> Result<AttachmentKind, AttachmentError> {
    let kind = AttachmentKind::from_mime(mime)?;
    if !model
        .server_info
        .as_ref()
        .is_some_and(|info| info.capabilities.iter().any(|name| name == "uploads"))
    {
        return Err(AttachmentError::UploadsUnavailable);
    }
    let Some(info) = model.active_model_info() else {
        return Err(AttachmentError::ModelUnknown);
    };
    if !info.input.contains(&kind.modality()) {
        return Err(AttachmentError::ModalityRejected {
            kind: kind.word(),
            model: info.model.clone(),
        });
    }
    Ok(kind)
}
