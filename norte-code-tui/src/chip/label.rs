//! The one-line labels chips are drawn with.

use serde::Serialize;

use super::content::OriginalText;
use crate::attachment::AttachmentKind;

/// How many characters of a long paste the label previews.
const PREVIEW_CHARS: usize = 6;

/// The text drawn in place of the chip's content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ChipLabel(String);

impl ChipLabel {
    /// Build the label for a long paste.
    pub(crate) fn copied(text: &OriginalText) -> Self {
        let collapsed = text
            .as_str()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut seen = 0;
        let mut preview = String::new();
        for ch in collapsed.chars() {
            if !ch.is_whitespace() {
                if seen == PREVIEW_CHARS {
                    break;
                }
                seen += 1;
            }
            preview.push(ch);
        }
        Self(format!("[Copied: {}...]", preview.trim_end()))
    }

    /// Build the label for an uploaded kind.
    pub(crate) fn uploaded(kind: AttachmentKind, index: usize) -> Self {
        Self(format!("[{} #{}]", chip_word(kind), index))
    }

    /// The label as text.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The reader-facing name of an attachment kind.
pub(crate) const fn chip_word(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "Image",
        AttachmentKind::File => "File",
        AttachmentKind::Audio => "Audio",
        AttachmentKind::Video => "Video",
    }
}
