//! What is drawn over the screen.
//!
//! One overlay at a time. It dims everything under it with a scrim and draws a
//! single modal, so the reader always knows what owns the keys.

mod scrim;

pub(crate) mod list;
pub(crate) mod modal;

pub(crate) use list::ModalList;
pub(crate) use modal::Modal;
pub(crate) use scrim::paint_scrim;

/// One row of a modal list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MenuEntry {
    /// What the row says.
    pub(crate) label: String,
    /// The key or chord that runs it, when it has one.
    pub(crate) hint: Option<String>,
    /// A group the row belongs to; the list prints it as a quiet caption.
    pub(crate) section: Option<String>,
    /// Whether the row is the current value of a choice.
    pub(crate) current: bool,
}

impl MenuEntry {
    /// A row with a label and an optional hint.
    pub(crate) fn new(label: impl Into<String>, hint: Option<String>) -> Self {
        Self {
            label: label.into(),
            hint,
            section: None,
            current: false,
        }
    }

    /// The same row under a section caption.
    pub(crate) fn in_section(mut self, section: &str) -> Self {
        self.section = Some(section.to_owned());
        self
    }

    /// The same row marked as the current value.
    pub(crate) const fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }
}
