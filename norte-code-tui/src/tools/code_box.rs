//! A code box: numbered lines on the footer's gray, the way an editor draws
//! them. Used by the `edit` tool so a change reads as a change to a file
//! rather than as a block of prose.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

use crate::{components::palette, ui_model::ThemeMode};

/// One line of a code box.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CodeRow {
    /// A line that did not change. The number is its line in the file the
    /// change was computed against.
    Context { number: Option<usize>, text: String },
    /// A line the change introduced.
    Added { number: Option<usize>, text: String },
    /// A line the change removed.
    Removed { number: Option<usize>, text: String },
    /// A jump over lines the reader does not need.
    Gap,
}

impl CodeRow {
    /// A line that did not change.
    pub(crate) fn context(number: Option<usize>, text: impl Into<String>) -> Self {
        Self::Context {
            number,
            text: text.into(),
        }
    }

    /// A line the change introduced.
    pub(crate) fn added(number: Option<usize>, text: impl Into<String>) -> Self {
        Self::Added {
            number,
            text: text.into(),
        }
    }

    /// A line the change removed.
    pub(crate) fn removed(number: Option<usize>, text: impl Into<String>) -> Self {
        Self::Removed {
            number,
            text: text.into(),
        }
    }

    /// The text the row carries, whatever its kind.
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Context { text, .. } | Self::Added { text, .. } | Self::Removed { text, .. } => {
                text
            }
            Self::Gap => "",
        }
    }

    /// The line number the row shows, when the file told us one.
    pub(crate) fn number(&self) -> Option<usize> {
        match self {
            Self::Context { number, .. }
            | Self::Added { number, .. }
            | Self::Removed { number, .. } => *number,
            Self::Gap => None,
        }
    }
}

/// A title line and its rows.
pub(crate) struct CodeBox<'a> {
    pub(crate) title: &'a str,
    pub(crate) rows: &'a [CodeRow],
}

/// Draws code boxes.
pub(crate) struct CodeBoxRenderer;

impl CodeBoxRenderer {
    /// Draws the box: the title on the gray, then one row per line.
    pub(crate) fn render(code: &CodeBox<'_>, width: u16, theme: ThemeMode) -> Vec<Line<'static>> {
        let width = usize::from(width);
        let background = palette::panel_background(theme);
        let gutter = code
            .rows
            .iter()
            .filter_map(CodeRow::number)
            .max()
            .unwrap_or(0)
            .to_string()
            .len()
            .max(2);
        let mut lines = Vec::with_capacity(code.rows.len() + 1);
        lines.push(Self::banner(code.title, width, background, theme));
        for row in code.rows {
            lines.push(Self::row(row, gutter, width, background, theme));
        }
        lines
    }

    /// The title row: bold text on the block's gray, filled to the edge.
    fn banner(title: &str, width: usize, background: Color, theme: ThemeMode) -> Line<'static> {
        let mut spans = vec![Span::styled(
            title.to_owned(),
            Style::default()
                .fg(palette::accent(theme))
                .add_modifier(Modifier::BOLD)
                .bg(background),
        )];
        spans.push(fill(display_width(title), width, background));
        Line::from(spans).style(Style::default().bg(background))
    }

    /// One code row: the number, a separator, the sign and the text.
    fn row(
        row: &CodeRow,
        gutter: usize,
        width: usize,
        background: Color,
        theme: ThemeMode,
    ) -> Line<'static> {
        if matches!(row, CodeRow::Gap) {
            const GAP: &str = "⋯";
            return Line::from(vec![
                Span::styled(" ".repeat(gutter + 1), Style::default().bg(background)),
                Span::styled(
                    GAP.to_owned(),
                    Style::default().fg(palette::muted()).bg(background),
                ),
                fill(gutter + 1 + display_width(GAP), width, background),
            ])
            .style(Style::default().bg(background));
        }

        let (sign, color) = match row {
            CodeRow::Added { .. } => ("+", palette::success()),
            CodeRow::Removed { .. } => ("-", palette::danger()),
            CodeRow::Context { .. } | CodeRow::Gap => (" ", palette::foreground(theme)),
        };
        let number = row
            .number()
            .map_or_else(|| " ".repeat(gutter), |number| format!("{number:>gutter$}"));
        let mut spans = vec![
            Span::styled(number, Style::default().fg(palette::muted()).bg(background)),
            Span::styled(" │ ", Style::default().fg(palette::muted()).bg(background)),
            Span::styled(
                format!("{sign} "),
                Style::default().fg(color).bg(background),
            ),
            Span::styled(
                row.text().to_owned(),
                Style::default().fg(color).bg(background),
            ),
        ];
        let used = spans
            .iter()
            .map(|span| display_width(span.content.as_ref()))
            .sum::<usize>();
        spans.push(fill(used, width, background));
        Line::from(spans).style(Style::default().bg(background))
    }
}

/// The spaces that take a row to the last column.
fn fill(used: usize, width: usize, background: Color) -> Span<'static> {
    Span::styled(
        " ".repeat(width.saturating_sub(used)),
        Style::default().bg(background),
    )
}

/// How wide a string is once the terminal has drawn it.
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|character| UnicodeWidthChar::width(character).unwrap_or(0))
        .sum()
}
