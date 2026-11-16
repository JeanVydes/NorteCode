use crate::components::palette;
use pulldown_cmark::Alignment;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
    row: Vec<Vec<Span<'static>>>,
    cell: Vec<Span<'static>>,
    pub(super) in_cell: bool,
}

impl Table {
    pub(super) fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            rows: Vec::new(),
            row: Vec::new(),
            cell: Vec::new(),
            in_cell: false,
        }
    }

    pub(super) fn start_row(&mut self) {
        self.row.clear();
    }

    pub(super) fn start_cell(&mut self) {
        self.cell.clear();
        self.in_cell = true;
    }

    pub(super) fn append(&mut self, span: Span<'static>) {
        if self.in_cell {
            self.cell.push(span);
        }
    }

    pub(super) fn finish_cell(&mut self) {
        self.row.push(std::mem::take(&mut self.cell));
        self.in_cell = false;
    }

    pub(super) fn finish_row(&mut self) {
        if !self.row.is_empty() {
            self.rows.push(std::mem::take(&mut self.row));
        }
    }
}

pub(super) fn render_table(table: &Table, width: usize) -> Vec<Line<'static>> {
    let Some(headers) = table.rows.first() else {
        return Vec::new();
    };
    let columns = headers.len();
    if columns == 0 {
        return Vec::new();
    }

    let border_width = columns.saturating_mul(3).saturating_add(1);
    let inner_width = width.saturating_sub(border_width);
    let shared_width = inner_width / columns;
    if shared_width < 4 {
        return render_stacked_table(&table.rows, width);
    }

    let mut widths = (0..columns)
        .map(|column| {
            table
                .rows
                .iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell_width(cell).min(shared_width))
                .max()
                .unwrap_or(3)
                .max(3)
        })
        .collect::<Vec<_>>();
    while widths.iter().sum::<usize>() > inner_width {
        let Some((index, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, column_width)| **column_width > 3)
            .max_by_key(|(_, column_width)| **column_width)
        else {
            return render_stacked_table(&table.rows, width);
        };
        widths[index] -= 1;
    }

    let mut lines = vec![table_border(&widths, '┌', '┬', '┐')];
    for (row_index, row) in table.rows.iter().enumerate() {
        lines.extend(render_grid_row(row, &widths, &table.alignments));
        if row_index == 0 {
            lines.push(table_border(&widths, '├', '┼', '┤'));
        }
    }
    lines.push(table_border(&widths, '└', '┴', '┘'));
    lines
}

fn render_grid_row(
    row: &[Vec<Span<'static>>],
    widths: &[usize],
    alignments: &[Alignment],
) -> Vec<Line<'static>> {
    let cells = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            row.get(index)
                .map_or_else(|| vec![Vec::new()], |cell| wrap_spans(cell, *width))
        })
        .collect::<Vec<_>>();
    let height = cells.iter().map(Vec::len).max().unwrap_or(1);
    (0..height)
        .map(|line_index| {
            let mut spans = vec![Span::styled("│ ", table_border_style())];
            for (column, lines) in cells.iter().enumerate() {
                let cell = lines.get(line_index).map(Vec::as_slice).unwrap_or_default();
                append_aligned_cell(
                    &mut spans,
                    cell,
                    widths[column],
                    alignments.get(column).copied().unwrap_or(Alignment::None),
                );
                spans.push(Span::styled(
                    if column + 1 == cells.len() {
                        " │"
                    } else {
                        " │ "
                    },
                    table_border_style(),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn append_aligned_cell(
    spans: &mut Vec<Span<'static>>,
    cell: &[Span<'static>],
    width: usize,
    alignment: Alignment,
) {
    let text_width = cell_width(cell).min(width);
    let remaining = width.saturating_sub(text_width);
    let left = match alignment {
        Alignment::Right => remaining,
        Alignment::Center => remaining / 2,
        Alignment::None | Alignment::Left => 0,
    };
    let right = remaining.saturating_sub(left);
    if left > 0 {
        spans.push(Span::raw(" ".repeat(left)));
    }
    spans.extend(cell.iter().cloned());
    if right > 0 {
        spans.push(Span::raw(" ".repeat(right)));
    }
}

fn table_border(widths: &[usize], left: char, join: char, right: char) -> Line<'static> {
    let mut text = String::new();
    text.push(left);
    for (index, width) in widths.iter().enumerate() {
        text.push_str(&"─".repeat(width + 2));
        text.push(if index + 1 == widths.len() {
            right
        } else {
            join
        });
    }
    Line::from(Span::styled(text, table_border_style()))
}

fn table_border_style() -> Style {
    Style::default().fg(palette::muted())
}

fn render_stacked_table(rows: &[Vec<Vec<Span<'static>>>], width: usize) -> Vec<Line<'static>> {
    let Some(headers) = rows.first() else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for (row_index, row) in rows.iter().enumerate().skip(1) {
        if row_index > 1 {
            lines.push(Line::default());
        }
        for (column, cell) in row.iter().enumerate() {
            let header = headers
                .get(column)
                .map_or_else(String::new, |value| spans_text(value));
            let mut labeled = vec![Span::styled(
                format!("{header}: "),
                Style::default().add_modifier(Modifier::BOLD),
            )];
            labeled.extend(cell.iter().cloned());
            lines.extend(
                wrap_spans(&labeled, width.max(1))
                    .into_iter()
                    .map(Line::from),
            );
        }
    }
    lines
}

fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut lines = vec![Vec::new()];
    let mut line_width = 0_usize;
    for span in spans {
        for character in span.content.chars() {
            if character == '\n' {
                lines.push(Vec::new());
                line_width = 0;
                continue;
            }
            let character_width = UnicodeWidthChar::width(character).unwrap_or_default();
            if line_width > 0 && line_width.saturating_add(character_width) > width {
                lines.push(Vec::new());
                line_width = 0;
            }
            lines
                .last_mut()
                .expect("a wrapping line exists")
                .push(Span::styled(character.to_string(), span.style));
            line_width += character_width;
        }
    }
    lines
}

fn cell_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn spans_text(spans: &[Span<'static>]) -> String {
    spans.iter().map(|span| span.content.as_ref()).collect()
}
