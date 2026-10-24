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
mod tests {
    use super::needs_escape;

    #[test]
    fn every_control_is_escaped_except_the_two_that_layout_needs() {
        for code in 0..=0x1f {
            let character = char::from_u32(code).expect("ascii control");
            let expected = character != '\n' && character != '\t';
            assert_eq!(
                needs_escape(character),
                expected,
                "U+{code:04X} should {} be escaped",
                if expected { "" } else { "not" }
            );
        }
        assert!(needs_escape('\u{7f}'), "delete is a control");
    }

    #[test]
    fn each_bidi_range_is_covered() {
        for character in ['\u{061c}', '\u{200b}', '\u{200f}', '\u{2028}', '\u{202e}'] {
            assert!(needs_escape(character), "U+{:04X}", u32::from(character));
        }
        for character in ['\u{2060}', '\u{2064}', '\u{2066}', '\u{206f}', '\u{feff}'] {
            assert!(needs_escape(character), "U+{:04X}", u32::from(character));
        }
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for character in ['a', 'Z', '0', ' ', 'ú', '⏳', '·', '─', '✓', '\n', '\t'] {
            assert!(
                !needs_escape(character),
                "U+{:04X} is plain text",
                u32::from(character)
            );
        }
    }
}
