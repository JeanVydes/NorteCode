//! What the composer holds in place of the full content.
//!
//! A chip is one piece of content the composer shows in a single line: a
//! text too long to read inline, a file, an image. A chip is atomic: the
//! reader never edits it, and deleting it removes the whole thing. The chips
//! that travel together are one unit and are deleted together.

use std::fmt;

use norte_code_wire::{InputPartKind, UserInputPart};
use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{components::palette, ui_model::ThemeMode};

/// How much of a pasted text a chip previews.
const PREVIEW_LIMIT: usize = 48;

/// A chip: content the composer shows in one line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Chip {
    /// What the chip carries.
    pub(crate) content: ChipContent,
    /// What the chip shows: a path, or what a pasted text starts with.
    pub(crate) label: String,
    /// What the chip is, when the content says so.
    pub(crate) kind: Option<InputPartKind>,
}

/// What a chip carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ChipContent {
    /// Text the reader pasted.
    Text(String),
    /// A file the host stores.
    Upload(UserInputPart),
}

/// What a chip is, for the reader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChipKind {
    /// Text too long to show.
    Text,
    /// A file.
    File,
    /// An image.
    Image,
    /// An audio recording.
    Audio,
    /// A video.
    Video,
}

impl ChipKind {
    /// How the chip names its kind.
    const fn label(self) -> &'static str {
        match self {
            Self::Text => "Copied",
            Self::File => "File",
            Self::Image => "Image",
            Self::Audio => "Audio",
            Self::Video => "Video",
        }
    }
}

impl Chip {
    /// A chip that carries text.
    pub(crate) fn text(content: String) -> Self {
        let label = preview(&content);
        Self {
            content: ChipContent::Text(content),
            label,
            kind: None,
        }
    }

    /// A chip that carries an upload the host stored.
    pub(crate) fn upload(part: UserInputPart, label: String, kind: InputPartKind) -> Self {
        Self {
            content: ChipContent::Upload(part),
            label,
            kind: Some(kind),
        }
    }

    /// What the chip is, for the reader.
    pub(crate) fn kind(&self) -> ChipKind {
        match self.kind {
            Some(InputPartKind::Image) => ChipKind::Image,
            Some(InputPartKind::Audio) => ChipKind::Audio,
            Some(InputPartKind::Video) => ChipKind::Video,
            Some(InputPartKind::Document) => ChipKind::File,
            Some(InputPartKind::Text) | None => ChipKind::Text,
        }
    }
}

impl fmt::Display for Chip {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            ChipKind::Text => write!(formatter, "[Copied: {}]", self.label),
            kind => write!(formatter, "[{} {}]", kind.label(), self.label),
        }
    }
}

/// The one-line preview of a pasted text.
fn preview(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = collapsed.chars().take(PREVIEW_LIMIT).collect::<String>();
    if collapsed.chars().count() > PREVIEW_LIMIT {
        preview.push_str("...");
    }
    preview
}

/// The line a chip is drawn with.
pub(crate) fn chip_line(chip: &Chip, theme: ThemeMode) -> Line<'static> {
    Line::from(Span::styled(
        chip.to_string(),
        Style::default().fg(palette::text(theme)),
    ))
}

/// The chips that travel together.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ChipGroup {
    /// The chips of the group.
    pub(crate) chips: Vec<Chip>,
}

impl ChipGroup {
    /// A group that carries one chip.
    pub(crate) fn one(chip: Chip) -> Self {
        Self { chips: vec![chip] }
    }
}

/// The parts the chips travel as.
///
/// A chip that carries text travels as a text part, and a chip that carries
/// an upload travels as the upload the host stored.
pub(crate) fn parts(chips: &[Chip]) -> Vec<UserInputPart> {
    chips
        .iter()
        .map(|chip| match &chip.content {
            ChipContent::Text(text) => UserInputPart::text(text.clone()),
            ChipContent::Upload(part) => part.clone(),
        })
        .collect()
}

#[cfg(test)]
#[path = "chips_tests.rs"]
mod tests;
