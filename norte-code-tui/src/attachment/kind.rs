//! The closed set of attachment kinds the client can carry.

use norte_code_wire::{InputPartKind, Modality};

use super::error::AttachmentError;
use super::mime::MimeType;

/// What an attachment carries, from the reader's point of view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttachmentKind {
    /// A picture.
    Image,
    /// A file the reader names as a file.
    File,
    /// A recording.
    Audio,
    /// A moving picture.
    Video,
}

impl AttachmentKind {
    /// The word shown in a notice.
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    /// The wire part that carries this kind.
    pub(crate) const fn part_kind(self) -> InputPartKind {
        match self {
            Self::Image => InputPartKind::Image,
            Self::File => InputPartKind::Document,
            Self::Audio => InputPartKind::Audio,
            Self::Video => InputPartKind::Video,
        }
    }

    /// The model input modality this kind needs.
    pub(crate) const fn modality(self) -> Modality {
        match self {
            Self::Image => Modality::Image,
            Self::File => Modality::File,
            Self::Audio => Modality::Audio,
            Self::Video => Modality::Video,
        }
    }

    /// The fixed slot this kind occupies in attachment counts.
    pub(crate) const fn slot(self) -> usize {
        match self {
            Self::Image => 0,
            Self::File => 1,
            Self::Audio => 2,
            Self::Video => 3,
        }
    }

    /// Classify a media type using the host's rules.
    ///
    /// The client helper also accepts `application/json`, but the host
    /// rejects it, so the stricter host rule is the one used here.
    pub(crate) fn from_mime(mime: &MimeType) -> Result<Self, AttachmentError> {
        let value = mime.as_str();
        if value.starts_with("image/") {
            Ok(Self::Image)
        } else if value.starts_with("audio/") {
            Ok(Self::Audio)
        } else if value.starts_with("video/") {
            Ok(Self::Video)
        } else if matches!(value, "application/pdf" | "text/plain" | "text/markdown") {
            Ok(Self::File)
        } else {
            Err(AttachmentError::UnsupportedMime {
                mime: mime.clone(),
                path: None,
            })
        }
    }
}
