//! The spacing the interface is laid out with.
//!
//! Every margin, padding and gap comes from these tokens, so the screen keeps
//! one rhythm instead of a different one per view.

use ratatui::widgets::Padding;

/// The air between the screen edge and any panel.
pub const SCREEN_GUTTER: u16 = 2;

/// The air inside a panel, on both sides of its content.
pub const PANEL_PADDING: u16 = 1;

/// The air inside a modal, on both sides of its content.
pub const MODAL_PADDING: u16 = 2;

/// The horizontal padding of a panel.
#[must_use]
pub const fn panel() -> Padding {
    Padding::horizontal(PANEL_PADDING)
}

/// The horizontal padding of a modal.
#[must_use]
pub const fn modal() -> Padding {
    Padding::horizontal(MODAL_PADDING)
}
