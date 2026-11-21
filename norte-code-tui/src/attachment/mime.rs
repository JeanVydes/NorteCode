//! Validated media types the client can send.

use std::fmt;
use std::path::Path;

use serde::Serialize;

use super::error::AttachmentError;

/// A media type the TUI is willing to hand to the host.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct MimeType(String);

impl MimeType {
    /// The media type a raw clipboard image is encoded as.
    pub(crate) fn png() -> Self {
        Self("image/png".to_owned())
    }

    /// Parse and validate a media type coming from the platform.
    pub(crate) fn parse(value: &str) -> Result<Self, AttachmentError> {
        let value = value.trim();
        let mut halves = value.split('/');
        let top = halves.next().unwrap_or_default();
        let sub = halves.next().unwrap_or_default();
        let valid = halves.next().is_none()
            && !top.is_empty()
            && !sub.is_empty()
            && value
                .chars()
                .all(|ch| !ch.is_whitespace() && !ch.is_control());
        if !valid {
            return Err(AttachmentError::InvalidMime {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// Guess the media type from a file extension.
    pub(crate) fn from_path(path: &Path) -> Result<Self, AttachmentError> {
        let Some(value) = mime_of_path(path) else {
            return Err(AttachmentError::UnknownMime {
                path: path.to_path_buf(),
            });
        };
        let mime = Self::parse(value)?;
        if mime.0 == "application/json" {
            return Err(AttachmentError::UnsupportedMime {
                mime,
                path: Some(path.to_path_buf()),
            });
        }
        Ok(mime)
    }

    /// The media type as text.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MimeType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn mime_of_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "ogg" | "oga" => Some("audio/ogg"),
        "m4a" => Some("audio/mp4"),
        "flac" => Some("audio/flac"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "webm" => Some("video/webm"),
        "mkv" => Some("video/x-matroska"),
        "pdf" => Some("application/pdf"),
        "txt" | "log" => Some("text/plain"),
        "md" | "markdown" => Some("text/markdown"),
        "json" => Some("application/json"),
        _ => None,
    }
}
