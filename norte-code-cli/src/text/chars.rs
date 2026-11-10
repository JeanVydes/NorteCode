//! Which characters can control the terminal or spoof reading order.
//!
//! The table is long, categorical and load-bearing for safety, so it lives on
//! its own with a test per class: a missing range would let a model write text
//! the terminal obeys.

/// Whether a character can control the terminal or spoof reading order.
pub(crate) fn needs_escape(character: char) -> bool {
    (character.is_control() && character != '\n' && character != '\t') || is_bidi(character)
}

/// The invisible formatting characters: bidi overrides, zero-width joiners and
/// line/paragraph separators.
fn is_bidi(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
    )
}

#[cfg(test)]
#[path = "chars_tests.rs"]
mod tests;
