//! Where copied content goes.

use super::error::ClipboardError;

/// The one clipboard every copy path in the application writes to.
pub(crate) trait ClipboardSink: Send + Sync {
    /// Place text on the system clipboard.
    ///
    /// # Errors
    /// Returns an error when the platform clipboard is unavailable.
    fn write(&self, text: &str) -> Result<(), ClipboardError>;
}
