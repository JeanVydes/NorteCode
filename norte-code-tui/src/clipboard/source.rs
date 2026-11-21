//! Where pasted content comes from.

use super::content::ClipboardContent;
use super::error::ClipboardError;

/// A source the composer can read when the reader presses Ctrl+V.
pub(crate) trait ClipboardSource: Send + Sync {
    /// Read the current payload, preferring images over files over text.
    ///
    /// # Errors
    /// Returns an error when the platform clipboard is unavailable or empty.
    fn read(&self) -> Result<ClipboardContent, ClipboardError>;
}
