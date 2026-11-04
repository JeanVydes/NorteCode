//! The one place the interface's colours, glyphs and spacing are chosen.
//!
//! Every painter reads this module instead of writing a literal. A colour is a
//! value, not an escape sequence: the string painters (`Style`) and the frame
//! (`ratatui::style`) both translate it for their destination, so the log and
//! the drawn screen cannot drift apart.
//!
//! Bright magenta is the brand: it marks what is live — the spinner, the
//! prompt, the selected row and tool activity. The other hues keep their
//! meaning (green is success, red is failure, yellow is a warning) and are
//! never decoration.
//!
//! The glyphs are safe Unicode: every terminal shows them on its stock font.
//! No Nerd Font is assumed, because an icon a terminal cannot draw is a hole
//! where the interface was.

/// The primary accent: bright magenta.
pub(crate) const PRIMARY: (u8, u8, u8) = (0xFF, 0x5C, 0xD6);

/// The 256-colour index closest to [`PRIMARY`].
pub(crate) const PRIMARY_256: u8 = 201;

/// The classic sixteen-colour fallback for [`PRIMARY`].
pub(crate) const PRIMARY_ANSI: &str = "95";

/// The secondary text and chrome colour.
pub(crate) const SECONDARY: (u8, u8, u8) = (0x9A, 0xA0, 0xA6);
pub(crate) const SECONDARY_256: u8 = 245;
pub(crate) const SECONDARY_ANSI: &str = "2";

/// The alternate text colour used for thinking and tool output.
pub(crate) const ALTERNATE: (u8, u8, u8) = (0xB8, 0xA8, 0xB8);
pub(crate) const ALTERNATE_256: u8 = 182;
pub(crate) const ALTERNATE_ANSI: &str = "35";

/// The quiet ink of rules and chrome.
pub(crate) const RULE: (u8, u8, u8) = (0x4A, 0x4E, 0x54);

/// The background of alternate tool output panels.
pub(crate) const OUTPUT_BACKGROUND: (u8, u8, u8) = (0x2A, 0x2A, 0x30);

pub(crate) const GREEN: (u8, u8, u8) = (0x98, 0xC3, 0x79);
pub(crate) const RED: (u8, u8, u8) = (0xE0, 0x6C, 0x75);
pub(crate) const YELLOW: (u8, u8, u8) = (0xE5, 0xC0, 0x7B);

/// The glyphs the interface draws.
#[allow(dead_code)]
pub(crate) mod icon {
    /// The prompt marker and the menu cursor.
    pub(crate) const PROMPT: &str = "›";
    /// The frames of the spinner, in the order they turn.
    pub(crate) const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];
    /// Something finished.
    pub(crate) const DONE: &str = "✓";
    /// Something not started.
    pub(crate) const PENDING: &str = "○";
    /// Something running.
    pub(crate) const RUNNING: &str = "●";
    /// A list that was cut off at the edge.
    pub(crate) const MORE: &str = "…";
    /// The horizontal rule.
    pub(crate) const RULE: &str = "─";
    /// The top edge of a blocked entry.
    pub(crate) const BOX_TOP: &str = "╭";
    /// The bottom edge of a blocked entry.
    pub(crate) const BOX_BOTTOM: &str = "╰";
    /// The margin of a message the human sent.
    pub(crate) const SAID: &str = "▌";
    /// The margin of the model's reasoning.
    pub(crate) const THINKING: &str = "│";
    /// Something that failed.
    pub(crate) const FAIL: &str = "✗";
    /// Alias for [`FAIL`].
    pub(crate) const FAILED: &str = "✗";
    /// Alias for [`FAIL`].
    pub(crate) const ERROR: &str = "✗";
    /// Alias for [`FAIL`].
    pub(crate) const CROSS: &str = "✗";
    /// Alias for [`FAIL`].
    pub(crate) const FAILURE: &str = "✗";
    /// A short failure marker for terminals without the cross.
    pub(crate) const BANG: &str = "!";
}

/// The spacing the layout is built from.
pub(crate) mod metrics {
    /// The rows the input block may grow to.
    pub(crate) const INPUT_ROWS: u16 = 10;
    /// How many completion rows the menu shows at once.
    pub(crate) const COMPLETION_ROWS: usize = 4;
    /// The columns every conversation row is drawn inside.
    pub(crate) const MARGIN_COLS: u16 = 3;
    /// The indent of a completion menu row under the draft.
    pub(crate) const MENU_INDENT: usize = 4;
    /// How long one spinner frame holds, in milliseconds.
    pub(crate) const SPINNER_MS: u64 = 90;
}

/// The theme a widget is drawn with.
///
/// It carries no state today: the palette and the glyphs live as constants on
/// this module, and the struct is the handle a [`crate::ui::Widget`] receives
/// so a painter never has to name the module it reads from.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Theme;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_is_orange() {
        assert_eq!(PRIMARY, (0xFF, 0x5C, 0xD6));
        assert_eq!(PRIMARY_256, 201);
    }

    #[test]
    fn the_icon_set_is_safe_unicode() {
        assert_eq!(icon::DONE, "✓");
        assert_eq!(icon::PENDING, "○");
        assert_eq!(icon::RUNNING, "●");
        assert_eq!(icon::PROMPT, "›");
        assert_eq!(icon::MORE, "…");
        assert_eq!(icon::RULE, "─");
        assert_eq!(icon::BOX_TOP, "╭");
        assert_eq!(icon::BOX_BOTTOM, "╰");
    }
}
