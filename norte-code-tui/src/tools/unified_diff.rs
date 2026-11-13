use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::edit_diff::{DiffRow, DiffRows};
use crate::{components::palette, text_builder::TextBuilder};

pub(crate) struct UnifiedDiffComponent;

impl UnifiedDiffComponent {
    pub(crate) fn render(rows: &DiffRows) -> Vec<Line<'static>> {
        let mut lines = vec![TextBuilder::unified_diff_label()];
        for row in &rows.0 {
            match row {
                DiffRow::Context(text) => lines.push(Line::from(format!(" {text}"))),
                DiffRow::Change(before, after) => {
                    lines.push(Line::from(Span::styled(
                        format!("-{before}"),
                        Style::default().fg(palette::danger()),
                    )));
                    lines.push(Line::from(Span::styled(
                        format!("+{after}"),
                        Style::default().fg(palette::success()),
                    )));
                }
                DiffRow::Removed(text) => lines.push(Line::from(Span::styled(
                    format!("-{text}"),
                    Style::default().fg(palette::danger()),
                ))),
                DiffRow::Added(text) => lines.push(Line::from(Span::styled(
                    format!("+{text}"),
                    Style::default().fg(palette::success()),
                ))),
            }
        }
        lines
    }
}
