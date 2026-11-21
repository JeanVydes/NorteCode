//! Short reader-facing notices for failed attachments.

use std::path::PathBuf;

use super::error::AttachmentError;
use super::mime::MimeType;

/// How strongly a notice should be shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NoticeTone {
    /// The message was not attached, but nothing else broke.
    Warn,
    /// An operation failed.
    Error,
}

/// What happened to an attachment the reader tried to add.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AttachmentNotice {
    /// The platform reported a malformed media type.
    InvalidMime {
        /// The value the platform reported.
        value: String,
    },
    /// The file extension does not name a media type we know.
    UnknownMime {
        /// The file whose type is unknown.
        path: PathBuf,
    },
    /// The media type exists but the host will not accept it.
    UnsupportedMime {
        /// The rejected media type.
        mime: MimeType,
    },
    /// The host does not advertise uploads.
    UploadsUnavailable,
    /// The active model could not be resolved.
    ModelUnknown,
    /// The active model does not accept this input.
    ModalityRejected {
        /// The modality the model rejected.
        kind: &'static str,
        /// The model that rejected it.
        model: String,
    },
    /// The upload did not complete.
    UploadFailed {
        /// The reason the host reported.
        reason: String,
    },
    /// The system clipboard is not available.
    ClipboardUnavailable {
        /// The reason the platform reported.
        reason: String,
    },
}

impl AttachmentNotice {
    /// Build the reader-facing text.
    pub(crate) fn text(&self) -> String {
        match self {
            Self::InvalidMime { value } => format!("invalid media type `{value}`, no chip created"),
            Self::UnknownMime { path } => {
                format!("unknown media type for {}, no chip created", path.display())
            }
            Self::UnsupportedMime { mime } => {
                format!(
                    "unsupported media type `{}`, no chip created",
                    mime.as_str()
                )
            }
            Self::UploadsUnavailable => {
                "the host does not accept uploads, no chip created".to_owned()
            }
            Self::ModelUnknown => "active model is unknown, no chip created".to_owned(),
            Self::ModalityRejected { kind, model } => {
                format!("model {model} does not accept {kind} input, no chip created")
            }
            Self::UploadFailed { reason } => format!("upload failed: {reason}, no chip created"),
            Self::ClipboardUnavailable { reason } => {
                format!("clipboard unavailable: {reason}, no chip created")
            }
        }
    }

    /// How strongly the app should show this notice.
    pub(crate) const fn tone(&self) -> NoticeTone {
        match self {
            Self::UploadFailed { .. } | Self::ClipboardUnavailable { .. } => NoticeTone::Error,
            _ => NoticeTone::Warn,
        }
    }
}

impl From<AttachmentError> for AttachmentNotice {
    fn from(error: AttachmentError) -> Self {
        match error {
            AttachmentError::InvalidMime { value } => Self::InvalidMime { value },
            AttachmentError::UnknownMime { path } => Self::UnknownMime { path },
            AttachmentError::UnsupportedMime { mime, .. } => Self::UnsupportedMime { mime },
            AttachmentError::UploadsUnavailable => Self::UploadsUnavailable,
            AttachmentError::ModelUnknown => Self::ModelUnknown,
            AttachmentError::ModalityRejected { kind, model } => {
                Self::ModalityRejected { kind, model }
            }
        }
    }
}
