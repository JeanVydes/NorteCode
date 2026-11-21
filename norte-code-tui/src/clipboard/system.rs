//! The platform clipboard, used by every copy and paste path.

use arboard::Clipboard;

use super::content::{ClipboardContent, ClipboardFile, ClipboardImage};
use super::error::ClipboardError;
use super::image::encode_png;
use super::sink::ClipboardSink;
use super::source::ClipboardSource;
use crate::attachment::MimeType;

/// One clipboard, shared by the composer and the message actions.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemClipboard;

impl ClipboardSource for SystemClipboard {
    fn read(&self) -> Result<ClipboardContent, ClipboardError> {
        let mut clipboard = Clipboard::new().map_err(|error| ClipboardError::Unavailable {
            reason: error.to_string(),
        })?;

        if let Ok(image) = clipboard.get().image() {
            let bytes = encode_png(&image)?;
            return Ok(ClipboardContent::Image(ClipboardImage {
                mime: MimeType::png(),
                bytes,
            }));
        }

        if let Ok(paths) = clipboard.get().file_list()
            && !paths.is_empty()
        {
            return Ok(ClipboardContent::Files(
                paths
                    .into_iter()
                    .map(|path| ClipboardFile { path })
                    .collect(),
            ));
        }

        match clipboard.get().text() {
            Ok(text) if !text.is_empty() => Ok(ClipboardContent::Text(text)),
            Ok(_) => Err(ClipboardError::Unavailable {
                reason: "clipboard is empty".to_owned(),
            }),
            Err(error) => Err(ClipboardError::Unavailable {
                reason: error.to_string(),
            }),
        }
    }
}

impl ClipboardSink for SystemClipboard {
    fn write(&self, text: &str) -> Result<(), ClipboardError> {
        let mut clipboard = Clipboard::new().map_err(|error| ClipboardError::Unavailable {
            reason: error.to_string(),
        })?;
        clipboard
            .set_text(text)
            .map_err(|error| ClipboardError::Unavailable {
                reason: error.to_string(),
            })
    }
}
