//! Uploads: what a file is, and how it travels with a message.
//!
//! Both clients send files the same way, so the mapping from a media type to
//! the part that carries it lives here and not twice.

use std::path::Path;

use norte_code_wire::{InputPartKind, OutputModality, UserInputPart};

/// The kind of part a media type travels as.
///
/// Returns [`None`] for a media type no client sends.
#[must_use]
pub fn input_part_kind(mime: &str) -> Option<InputPartKind> {
    if mime.starts_with("image/") {
        Some(InputPartKind::Image)
    } else if mime.starts_with("audio/") {
        Some(InputPartKind::Audio)
    } else if mime.starts_with("video/") {
        Some(InputPartKind::Video)
    } else if matches!(
        mime,
        "application/pdf" | "text/plain" | "text/markdown" | "application/json"
    ) {
        Some(InputPartKind::Document)
    } else {
        None
    }
}

/// The modality a part belongs to.
#[must_use]
pub fn modality_of(kind: InputPartKind) -> OutputModality {
    match kind {
        InputPartKind::Text => OutputModality::Text,
        InputPartKind::Image => OutputModality::Image,
        InputPartKind::Document => OutputModality::Document,
        InputPartKind::Audio => OutputModality::Audio,
        InputPartKind::Video => OutputModality::Video,
    }
}

/// The word a person reads for a modality.
#[must_use]
pub fn modality_word(modality: OutputModality) -> &'static str {
    match modality {
        OutputModality::Text => "text",
        OutputModality::Image => "image",
        OutputModality::Document => "document",
        OutputModality::Audio => "audio",
        OutputModality::Video => "video",
    }
}

/// The media type a file name most likely carries.
///
/// A client that cannot ask the platform guesses from the extension, and
/// refuses the files it cannot name.
#[must_use]
pub fn mime_of_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        _ => return None,
    };
    Some(mime)
}

/// The part that carries an upload.
#[must_use]
pub fn upload_part(kind: InputPartKind, upload_id: &str, mime_type: &str) -> UserInputPart {
    UserInputPart::upload(kind, upload_id.to_owned(), mime_type.to_owned())
}

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
