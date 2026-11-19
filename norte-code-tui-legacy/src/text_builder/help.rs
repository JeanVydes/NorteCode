//! The keyboard help.
//!
//! The list is short on purpose: every command that is not a tab, a menu or a
//! movement lives in the command menu, and the help says so.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::components::palette;
use crate::keys;
use crate::ui_model::ThemeMode;

impl super::TextBuilder {
    /// The title of the help modal.
    #[must_use]
    pub const fn help_title() -> &'static str {
        "keys"
    }

    /// The lines the help modal shows.
    #[must_use]
    pub fn help_lines(theme: ThemeMode) -> Vec<Line<'static>> {
        let key =
            |text: &str| Span::styled(text.to_owned(), Style::default().fg(palette::accent(theme)));
        let what = |text: &str| {
            Span::styled(
                format!("  {text}"),
                Style::default().fg(palette::text_muted(theme)),
            )
        };
        let line = |k: &str, w: &str| Line::from(vec![key(k), what(w)]);

        vec![
            line("Esc", "open the command menu"),
            line("q", "quit"),
            line("?", "this help"),
            Line::default(),
            line("Tab / Shift+Tab", "next or previous tab"),
            line("1 to 9", "jump to a tab"),
            line("j / k", "move the selection"),
            line("Enter", "open or run"),
            line("g / G", "top or bottom of a list"),
            line("/", "filter the current list"),
            Line::default(),
            line("i", "focus the composer"),
            line("Enter", "send in the composer"),
            line("Shift+Enter", "new line in the composer"),
            line(keys::CANCEL_TURN_LABEL, "stop the running turn"),
            line("Ctrl+A", "attach a file"),
            Line::default(),
            Line::from(what(
                "Everything else, including compact, export, reasoning and queue mode, is in the menu.",
            )),
        ]
    }
}
