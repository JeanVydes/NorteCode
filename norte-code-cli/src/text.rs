//! Terminal-safe rendering of untrusted text.
//!
//! Model answers, tool output, provider errors and catalogue metadata are not
//! trusted. A raw ANSI escape can move the cursor, clear the screen, change the
//! title or write to the clipboard; a bidi override can make a command read
//! differently from what it runs. [`terminal`] escapes both while preserving
//! the newlines and tabs that normal text needs.

use std::borrow::Cow;
use std::fmt::Write as _;

mod chars;

pub(crate) use chars::needs_escape;

/// Returns `text` with terminal controls and bidi overrides escaped.
///
/// Printable text is returned without allocating. Newlines and tabs are kept;
/// every other control character and the invisible bidi/zero-width formatting
/// characters are written as `\u{...}`. This is a display transform only: the
/// escaped text must never be sent back to a model or executed.
pub(crate) fn terminal(text: &str) -> Cow<'_, str> {
    if !text.chars().any(needs_escape) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 8);
    for character in text.chars() {
        if needs_escape(character) {
            let _ = write!(escaped, "\\u{{{:x}}}", u32::from(character));
        } else {
            escaped.push(character);
        }
    }
    Cow::Owned(escaped)
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
