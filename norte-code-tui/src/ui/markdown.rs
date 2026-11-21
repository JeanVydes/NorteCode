//! The single Markdown renderer for plans and assistant messages.
//!
//! Parses `CommonMark` plus tables, strikethrough, task lists and footnotes into
//! styled ratatui lines. Tables shrink to fit the width and fall back to a
//! stacked `label: value` layout when columns would be too narrow. Links keep
//! their URL so a click can open them.

use std::process::Command;

use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use terminalui::Theme;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// A rendered document: the lines plus the links on each row.
#[derive(Clone, Debug, Default)]
pub(crate) struct Markdown {
    /// The styled lines of the document.
    pub(crate) lines: Vec<Line<'static>>,
    /// `(row, start_column, end_column, url)` for every link run.
    pub(crate) links: Vec<(usize, usize, usize, String)>,
}

/// Open a link in the host's browser, ignoring non-HTTP schemes.
pub(crate) fn open_link(url: &str) {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return;
    }
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = Command::new("xdg-open").arg(url).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = url;
}

impl Markdown {
    /// Render a Markdown document to at most `width` columns.
    pub(crate) fn render(text: &str, width: u16, theme: &Theme) -> Self {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES;
        let mut renderer = Renderer::new(width, theme);
        for event in Parser::new_ext(text, options) {
            renderer.event(event);
        }
        renderer.finish()
    }

    /// The URL under a row/column, if any.
    pub(crate) fn link_at(&self, row: usize, column: usize) -> Option<&str> {
        self.links.iter().find_map(|(line, start, end, url)| {
            (*line == row && column >= *start && column < *end).then_some(url.as_str())
        })
    }
}

struct Renderer<'a> {
    lines: Vec<Line<'static>>,
    line: Vec<Span<'static>>,
    style: Style,
    styles: Vec<Style>,
    table: Option<Table>,
    link: Option<(usize, usize, String)>,
    pending_links: Vec<(usize, usize, usize, String)>,
    width: usize,
    theme: &'a Theme,
}

impl<'a> Renderer<'a> {
    fn new(width: u16, theme: &'a Theme) -> Self {
        Self {
            lines: Vec::new(),
            line: Vec::new(),
            style: Style::default(),
            styles: Vec::new(),
            table: None,
            link: None,
            pending_links: Vec::new(),
            width: usize::from(width).max(1),
            theme,
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.append_text(&text),
            Event::Code(code) => self.styled(
                &code,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Event::Html(html) | Event::InlineHtml(html) => self.html(&html),
            Event::SoftBreak => self.append_text(" "),
            Event::HardBreak => self.newline(),
            Event::Rule => {
                self.finish_line();
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(self.width),
                    Style::default().fg(self.theme.muted),
                )));
            }
            Event::TaskListMarker(checked) => {
                self.append_text(if checked { "[x] " } else { "[ ] " });
            }
            Event::FootnoteReference(label) => self.append_text(&format!("[^{label}]")),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::List(_) => self.start_block(),
            Tag::Heading { level, .. } => {
                self.start_block();
                let size = level as u8;
                let style = Style::default()
                    .fg(if size <= 2 {
                        self.theme.accent
                    } else {
                        self.theme.foreground
                    })
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(if size <= 3 {
                        Modifier::UNDERLINED
                    } else {
                        Modifier::empty()
                    });
                self.start_style(style);
            }
            Tag::Strong => self.start_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Emphasis => self.start_style(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strikethrough => {
                self.start_style(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                self.start_style(
                    Style::default()
                        .fg(self.theme.accent)
                        .add_modifier(Modifier::UNDERLINED),
                );
                self.link = Some((self.lines.len(), self.line_width(), dest_url.into_string()));
            }
            Tag::CodeBlock(_) => {
                self.start_block();
                self.start_style(Style::default().fg(self.theme.accent));
            }
            Tag::BlockQuote(_) => {
                self.start_block();
                self.append_text("│ ");
            }
            Tag::Item => self.append_text("• "),
            Tag::Table(alignments) => {
                self.start_block();
                self.table = Some(Table::new(alignments));
            }
            Tag::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.start_row();
                }
                self.start_style(Style::default().add_modifier(Modifier::BOLD));
            }
            Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.start_row();
                }
            }
            Tag::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.start_cell();
                }
            }
            Tag::Image { .. } => self.append_text("[image: "),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Item | TagEnd::BlockQuote(_) | TagEnd::List(_) => {
                self.finish_line();
            }
            TagEnd::Heading(_) | TagEnd::CodeBlock => {
                self.finish_line();
                self.end_style();
            }
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => self.end_style(),
            TagEnd::Link => {
                self.end_style();
                if let Some((line, start, url)) = self.link.take() {
                    let end = if line == self.lines.len() {
                        self.line_width()
                    } else {
                        self.lines
                            .get(line)
                            .map_or(start, ratatui::text::Line::width)
                    };
                    if end > start {
                        // Stored on the renderer, moved into the document at finish.
                        self.pending_links.push((line, start, end, url));
                    }
                }
            }
            TagEnd::TableHead => {
                self.end_style();
                if let Some(table) = self.table.as_mut() {
                    table.finish_row();
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.finish_row();
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.finish_cell();
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.finish_line();
                    let rendered = table.render(self.width, self.theme);
                    self.lines.extend(rendered.lines);
                    self.pending_links.extend(rendered.links);
                }
            }
            TagEnd::Image => self.append_text("]"),
            _ => {}
        }
    }

    fn html(&mut self, html: &str) {
        match html.trim().to_ascii_lowercase().as_str() {
            "<u>" => self.start_style(Style::default().add_modifier(Modifier::UNDERLINED)),
            "</u>" => self.end_style(),
            "<br>" | "<br/>" | "<br />" => self.newline(),
            _ => self.append_text(html),
        }
    }

    fn styled(&mut self, text: &str, style: Style) {
        self.start_style(style);
        self.append_text(text);
        self.end_style();
    }

    fn append_text(&mut self, text: &str) {
        if let Some(table) = self.table.as_mut()
            && table.in_cell
        {
            table.append(Span::styled(text.to_owned(), self.style));
            return;
        }
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                self.newline();
            }
            if !part.is_empty() {
                self.line.push(Span::styled(part.to_owned(), self.style));
            }
        }
    }

    fn start_block(&mut self) {
        self.finish_line();
    }

    fn finish_line(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let wrapped = wrap_spans(&self.line, self.width);
        self.line.clear();
        self.lines.extend(wrapped.into_iter().map(Line::from));
    }

    fn newline(&mut self) {
        self.lines.push(Line::from(std::mem::take(&mut self.line)));
    }

    fn start_style(&mut self, style: Style) {
        self.styles.push(self.style);
        self.style = self.style.patch(style);
    }

    fn end_style(&mut self) {
        self.style = self.styles.pop().unwrap_or_default();
    }

    fn line_width(&self) -> usize {
        self.line.iter().map(ratatui::text::Span::width).sum()
    }

    fn finish(mut self) -> Markdown {
        self.finish_line();
        while self.lines.last().is_some_and(|line| line.spans.is_empty()) {
            self.lines.pop();
        }
        Markdown {
            lines: self.lines,
            links: self.pending_links,
        }
    }
}

/// A Markdown table, rendered responsively to the available width.
struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
    row: Vec<Vec<Span<'static>>>,
    cell: Vec<Span<'static>>,
    in_cell: bool,
}

impl Table {
    fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            rows: Vec::new(),
            row: Vec::new(),
            cell: Vec::new(),
            in_cell: false,
        }
    }

    fn start_row(&mut self) {
        self.row.clear();
    }

    fn start_cell(&mut self) {
        self.cell.clear();
        self.in_cell = true;
    }

    fn append(&mut self, span: Span<'static>) {
        if self.in_cell {
            self.cell.push(span);
        }
    }

    fn finish_cell(&mut self) {
        self.row.push(std::mem::take(&mut self.cell));
        self.in_cell = false;
    }

    fn finish_row(&mut self) {
        if !self.row.is_empty() {
            self.rows.push(std::mem::take(&mut self.row));
        }
    }

    fn render(self, width: usize, theme: &Theme) -> Markdown {
        let Some(headers) = self.rows.first() else {
            return Markdown::default();
        };
        let columns = headers.len();
        if columns == 0 {
            return Markdown::default();
        }
        let border_width = columns.saturating_mul(3).saturating_add(1);
        let inner = width.saturating_sub(border_width);
        let shared = inner / columns;
        if shared < 4 {
            return self.stacked(width, theme);
        }
        let mut widths = (0..columns)
            .map(|column| {
                self.rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| cell_width(cell).min(shared))
                    .max()
                    .unwrap_or(3)
                    .max(3)
            })
            .collect::<Vec<_>>();
        while widths.iter().sum::<usize>() > inner {
            let Some((index, _)) = widths
                .iter()
                .enumerate()
                .filter(|(_, size)| **size > 3)
                .max_by_key(|(_, size)| **size)
            else {
                return self.stacked(width, theme);
            };
            widths[index] -= 1;
        }
        let border_style = Style::default().fg(theme.muted);
        let mut lines = vec![border(&widths, '┌', '┬', '┐', border_style)];
        for (index, row) in self.rows.iter().enumerate() {
            lines.extend(grid_row(row, &widths, &self.alignments, border_style));
            if index == 0 {
                lines.push(border(&widths, '├', '┼', '┤', border_style));
            }
        }
        lines.push(border(&widths, '└', '┴', '┘', border_style));
        Markdown {
            lines,
            links: Vec::new(),
        }
    }

    fn stacked(self, width: usize, theme: &Theme) -> Markdown {
        let Some(headers) = self.rows.first() else {
            return Markdown::default();
        };
        let mut lines = Vec::new();
        for (index, row) in self.rows.iter().enumerate().skip(1) {
            if index > 1 {
                lines.push(Line::default());
            }
            for (column, cell) in row.iter().enumerate() {
                let header = headers
                    .get(column)
                    .map_or_else(String::new, |value| spans_text(value));
                let mut labeled = vec![Span::styled(
                    format!("{header}: "),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )];
                labeled.extend(cell.iter().cloned());
                lines.extend(
                    wrap_spans(&labeled, width.max(1))
                        .into_iter()
                        .map(Line::from),
                );
            }
        }
        Markdown {
            lines,
            links: Vec::new(),
        }
    }
}

fn border(widths: &[usize], left: char, join: char, right: char, style: Style) -> Line<'static> {
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
    Line::from(Span::styled(text, style))
}

fn grid_row(
    row: &[Vec<Span<'static>>],
    widths: &[usize],
    alignments: &[Alignment],
    border_style: Style,
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
            let mut spans = vec![Span::styled("│ ", border_style)];
            for (column, lines) in cells.iter().enumerate() {
                let cell = lines.get(line_index).map(Vec::as_slice).unwrap_or_default();
                let text_width = cell_width(cell).min(widths[column]);
                let remaining = widths[column].saturating_sub(text_width);
                let alignment = alignments.get(column).copied().unwrap_or(Alignment::None);
                let left = match alignment {
                    Alignment::Right => remaining,
                    Alignment::Center => remaining / 2,
                    Alignment::None | Alignment::Left => 0,
                };
                if left > 0 {
                    spans.push(Span::raw(" ".repeat(left)));
                }
                spans.extend(cell.iter().cloned());
                if remaining > left {
                    spans.push(Span::raw(" ".repeat(remaining - left)));
                }
                spans.push(Span::styled(
                    if column + 1 == cells.len() {
                        " │"
                    } else {
                        " │ "
                    },
                    border_style,
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let characters: Vec<(char, Style)> = spans
        .iter()
        .flat_map(|span| {
            span.content
                .chars()
                .map(|character| (character, span.style))
        })
        .collect();
    let mut lines = Vec::new();
    let mut current: Vec<(char, Style)> = Vec::new();
    let mut current_width = 0usize;
    let mut pending_space = false;
    let mut index = 0;

    while index < characters.len() {
        let (character, _) = characters[index];
        if character == '\n' {
            lines.push(styled_line(&current));
            current.clear();
            current_width = 0;
            pending_space = false;
            index += 1;
            continue;
        }
        if character.is_whitespace() {
            pending_space |= !current.is_empty();
            index += 1;
            continue;
        }

        let word_start = index;
        while index < characters.len() && !characters[index].0.is_whitespace() {
            index += 1;
        }
        let word = &characters[word_start..index];
        let word_width: usize = word
            .iter()
            .map(|(character, _)| UnicodeWidthChar::width(*character).unwrap_or_default())
            .sum();
        let separator = usize::from(pending_space && !current.is_empty());
        if !current.is_empty() && current_width + separator + word_width > width {
            lines.push(styled_line(&current));
            current.clear();
            current_width = 0;
            pending_space = false;
        }
        if word_width > width {
            for (character, style) in word {
                let character_width = UnicodeWidthChar::width(*character).unwrap_or_default();
                if current_width > 0 && current_width + character_width > width {
                    lines.push(styled_line(&current));
                    current.clear();
                    current_width = 0;
                }
                current.push((*character, *style));
                current_width += character_width;
            }
        } else {
            if pending_space && !current.is_empty() {
                current.push((' ', Style::default()));
                current_width += 1;
            }
            current.extend_from_slice(word);
            current_width += word_width;
        }
        pending_space = false;
    }
    lines.push(styled_line(&current));
    lines
}

fn styled_line(characters: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut style = Style::default();
    for (character, character_style) in characters {
        if text.is_empty() {
            style = *character_style;
        }
        if *character_style == style {
            text.push(*character);
        } else {
            spans.push(Span::styled(std::mem::take(&mut text), style));
            style = *character_style;
            text.push(*character);
        }
    }
    if !text.is_empty() {
        spans.push(Span::styled(text, style));
    }
    spans
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

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
