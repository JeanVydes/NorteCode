//! Typed failures for the attachment pipeline.

use std::path::PathBuf;

use thiserror::Error;

use super::mime::MimeType;

/// Why an attachment could not be prepared or sent.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub(crate) enum AttachmentError {
    /// The platform reported a value that is not a media type.
    #[error("invalid media type `{value}`")]
    InvalidMime {
        /// The value the platform reported.
        value: String,
    },
    /// The file extension does not name a media type we know.
    #[error("unknown media type for {}", path.display())]
    UnknownMime {
        /// The file whose type is unknown.
        path: PathBuf,
    },
    /// The media type exists but the host will not accept it.
    #[error("unsupported media type `{mime}`")]
    UnsupportedMime {
        /// The rejected media type.
        mime: MimeType,
        /// The file that carried it, when one was involved.
        path: Option<PathBuf>,
    },
    /// The host does not advertise uploads.
    #[error("the host does not accept uploads")]
    UploadsUnavailable,
    /// The active model could not be resolved.
    #[error("the active model is unknown")]
    ModelUnknown,
    /// The active model does not accept this modality.
    #[error("the active model does not accept `{kind}` input")]
    ModalityRejected {
        /// The modality the model rejected.
        kind: &'static str,
        /// The model that rejected it.
        model: String,
    },
}
