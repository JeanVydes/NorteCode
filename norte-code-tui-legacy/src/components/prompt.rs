//! The text prompts a view can open on request.
//!
//! They are opened from the command menu instead of a shortcut, so the view
//! takes them as an event and draws them as a modal.

/// A prompt a view opens when the menu asks for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Prompt {
    /// Asks for the path of a bundle to import.
    Import,
    /// Asks for the session budget in USD.
    Budget,
    /// Asks for a new title for the active session.
    Rename,
    /// Asks to confirm deleting the active session.
    Delete,
    /// Asks for the turn to rewind to.
    Rewind,
}
