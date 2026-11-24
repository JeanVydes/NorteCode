//! Small text helpers shared by the interface.

/// Trim a string to `limit` characters, adding an ellipsis when trimmed.
pub(crate) fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}
