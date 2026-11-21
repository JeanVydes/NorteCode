//! Typed clipboard failures.

use thiserror::Error;

/// Why the platform clipboard could not be used.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub(crate) enum ClipboardError {
    /// The clipboard could not be opened or had no usable payload.
    #[error("clipboard unavailable: {reason}")]
    Unavailable {
        /// The reason the platform reported.
        reason: String,
    },
    /// A clipboard image could not be encoded for upload.
    #[error("could not encode clipboard image: {reason}")]
    Encoding {
        /// The reason the encoder reported.
        reason: String,
    },
}
