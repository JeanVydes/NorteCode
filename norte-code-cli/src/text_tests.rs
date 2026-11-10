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
