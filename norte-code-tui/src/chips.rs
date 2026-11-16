//! What the composer holds in place of the full content.
//!
//! A chip is one piece of content the composer shows in a single line: a
//! text too long to read inline, a file, an image. A chip is atomic: the
//! reader never edits it, and deleting it removes the whole thing. The chips
//! that travel together are one unit and are deleted together.

use norte_code_wire::UserInputPart;

/// A chip: content the composer shows in one line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Chip {
    /// What the chip carries.
    pub(crate) content: ChipContent,
    /// What the chip shows.
    pub(crate) label: String,
    /// The detail the chip shows next to the label.
    pub(crate) detail: String,
    /// What the chip is, when the content says so.
    pub(crate) kind: Option<norte_code_wire::InputPartKind>,
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
}

impl Chip {
    /// A chip that carries text.
    pub(crate) fn text(content: String) -> Self {
        let detail = format!("{} chars", content.chars().count());
        Self {
            content: ChipContent::Text(content),
            label: "text".to_owned(),
            detail,
            kind: None,
        }
    }

    /// What the chip is, for the reader.
    pub(crate) fn kind(&self) -> ChipKind {
        match self.kind {
            Some(norte_code_wire::InputPartKind::Image) => ChipKind::Image,
            Some(norte_code_wire::InputPartKind::Text) | None => ChipKind::Text,
            Some(_) => ChipKind::File,
        }
    }
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

/// The icon a chip is drawn with.
pub(crate) const fn chip_icon(kind: ChipKind) -> &'static str {
    match kind {
        ChipKind::Text => "≡",
        ChipKind::File => "▣",
        ChipKind::Image => "▤",
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
