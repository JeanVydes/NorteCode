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
mod tests {
    use std::borrow::Cow;

    use super::terminal;

    #[test]
    fn plain_text_is_not_copied() {
        assert!(matches!(terminal("hello\n\tworld"), Cow::Borrowed(_)));
    }

    #[test]
    fn ansi_sequences_are_escaped() {
        assert_eq!(terminal("\x1b[2J"), "\\u{1b}[2J");
        // OSC title: ESC ] ... BEL.
        assert_eq!(terminal("\x1b]0;owned\x07"), "\\u{1b}]0;owned\\u{7}");
    }

    #[test]
    fn carriage_returns_cannot_rewrite_a_line() {
        assert_eq!(terminal("safe\rrm -rf /"), "safe\\u{d}rm -rf /");
    }

    #[test]
    fn bidi_and_zero_width_controls_are_escaped() {
        assert_eq!(terminal("a\u{202e}b"), "a\\u{202e}b");
        assert_eq!(terminal("a\u{200b}b"), "a\\u{200b}b");
        assert_eq!(terminal("a\u{feff}b"), "a\\u{feff}b");
    }

    #[test]
    fn newlines_and_tabs_survive() {
        assert_eq!(terminal("a\nb\tc"), "a\nb\tc");
    }
}
