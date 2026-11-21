//! Typed clipboard payloads.

use std::path::PathBuf;

use crate::attachment::MimeType;

/// An image decoded from the platform clipboard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClipboardImage {
    /// The media type the encoded bytes use.
    pub(crate) mime: MimeType,
    /// The encoded image bytes.
    pub(crate) bytes: Vec<u8>,
}

/// A file the platform clipboard points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClipboardFile {
    /// Where the bytes can be read.
    pub(crate) path: PathBuf,
}

/// What the platform clipboard currently holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardContent {
    /// A picture, chosen before any other payload.
    Image(ClipboardImage),
    /// One or more files, chosen before plain text.
    Files(Vec<ClipboardFile>),
    /// Plain text from the system clipboard.
    Text(String),
}
