//! Shared measurements for the chrome.

use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

impl super::TextBuilder {
    /// The display width of one character.
    #[must_use]
    pub(crate) fn char_width(character: char) -> usize {
        UnicodeWidthChar::width(character).unwrap_or(0)
    }

    /// Cuts a line at the width, keeping whole characters.
    #[must_use]
    pub(crate) fn fit_line(line: Line<'static>, width: u16) -> Line<'static> {
        let budget = usize::from(width);
        let mut spans = Vec::new();
        let mut used = 0;
        for span in line.spans {
            if used >= budget {
                break;
            }
            let mut text = String::new();
            for character in span.content.chars() {
                let width = Self::char_width(character);
                if used + width > budget {
                    break;
                }
                used += width;
                text.push(character);
            }
            if !text.is_empty() {
                spans.push(ratatui::text::Span::styled(text, span.style));
            }
        }
        Line::from(spans).style(line.style)
    }
}
