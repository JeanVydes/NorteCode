use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::code_box::CodeRow;
use crate::{components::palette, ui_model::ThemeMode};

pub(crate) struct SplitDiffComponent;

impl SplitDiffComponent {
    /// Draws the change side by side: what was there, what is there now.
    pub(crate) fn render(rows: &[CodeRow], width: u16, theme: ThemeMode) -> Vec<Line<'static>> {
        let column_width = usize::from(width.saturating_sub(6) / 2);
        let mut lines = Vec::with_capacity(rows.len());
        for row in rows {
            let (before, after, left, right) = match row {
                CodeRow::Context { text, .. } => (text.as_str(), text.as_str(), None, None),
                CodeRow::Removed { text, .. } => (text.as_str(), "", Some(palette::danger()), None),
                CodeRow::Added { text, .. } => ("", text.as_str(), None, Some(palette::success())),
                CodeRow::Gap => {
                    lines.push(Line::from(Span::styled(
                        " ⋯",
                        Style::default().fg(palette::muted()),
                    )));
                    continue;
                }
            };
            lines.push(Line::from(vec![
                Span::styled(
                    Self::fit(before, column_width),
                    Style::default().fg(left.unwrap_or_else(|| palette::foreground(theme))),
                ),
                Span::styled(" │ ", Style::default().fg(palette::muted())),
                Span::styled(
                    Self::fit(after, column_width),
                    Style::default().fg(right.unwrap_or_else(|| palette::foreground(theme))),
                ),
            ]));
        }
        lines
    }

    fn fit(text: &str, width: usize) -> String {
        text.chars().take(width).collect()
    }
}
