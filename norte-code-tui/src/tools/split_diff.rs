use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::edit_diff::{DiffRow, DiffRows};
use crate::{components::palette, text_builder::TextBuilder};

pub(crate) struct SplitDiffComponent;

impl SplitDiffComponent {
    pub(crate) fn render(rows: &DiffRows, width: u16) -> Vec<Line<'static>> {
        let column_width = usize::from(width.saturating_sub(3) / 2);
        let mut lines = vec![TextBuilder::split_diff_label()];
        for row in &rows.0 {
            let (left, right, left_style, right_style) = match row {
                DiffRow::Context(text) => (
                    text.as_str(),
                    text.as_str(),
                    Style::default(),
                    Style::default(),
                ),
                DiffRow::Change(before, after) => (
                    before.as_str(),
                    after.as_str(),
                    Style::default().fg(palette::danger()),
                    Style::default().fg(palette::success()),
                ),
                DiffRow::Removed(text) => (
                    text.as_str(),
                    "",
                    Style::default().fg(palette::danger()),
                    Style::default(),
                ),
                DiffRow::Added(text) => (
                    "",
                    text.as_str(),
                    Style::default(),
                    Style::default().fg(palette::success()),
                ),
            };
            lines.push(Line::from(vec![
                Span::styled(Self::fit(left, column_width), left_style),
                Span::raw(" | "),
                Span::styled(Self::fit(right, column_width), right_style),
            ]));
        }
        lines
    }

    fn fit(text: &str, width: usize) -> String {
        text.chars().take(width).collect()
    }
}
