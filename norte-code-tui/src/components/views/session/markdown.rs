use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::components::palette;

#[path = "markdown/table.rs"]
mod table;

use table::{Table, render_table};

pub(super) fn render(markdown: &str, width: u16) -> Vec<Line<'static>> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut renderer = MarkdownRenderer::new(width);
    for event in Parser::new_ext(markdown, options) {
        renderer.event(event);
    }
    renderer.finish()
}

struct MarkdownRenderer {
    lines: Vec<Line<'static>>,
    line: Vec<Span<'static>>,
    style: Style,
    styles: Vec<Style>,
    table: Option<Table>,
    width: usize,
}

impl MarkdownRenderer {
    fn new(width: u16) -> Self {
        Self {
            lines: Vec::new(),
            line: Vec::new(),
            style: Style::default(),
            styles: Vec::new(),
            table: None,
            width: usize::from(width).max(1),
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.append_text(&text),
            Event::Code(code) => {
                self.start_style(
                    Style::default()
                        .fg(palette::warning())
                        .add_modifier(Modifier::BOLD),
                );
                self.append_text(&code);
                self.end_style();
            }
            Event::Html(html) | Event::InlineHtml(html) => self.html(&html),
            Event::SoftBreak => self.append_text(" "),
            Event::HardBreak => self.newline(),
            Event::Rule => {
                self.finish_line();
                self.lines.push(Line::from(vec![Span::styled(
                    "─".repeat(self.width),
                    Style::default().fg(palette::muted()),
                )]));
            }
            Event::TaskListMarker(checked) => {
                self.append_text(if checked { "[x] " } else { "[ ] " });
            }
            Event::FootnoteReference(label) => {
                self.append_text(&format!("[^{label}]"));
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::List(_) => self.start_block(),
            Tag::Heading { .. } => {
                self.start_block();
                self.start_style(
                    Style::default()
                        .fg(palette::brand())
                        .add_modifier(Modifier::BOLD),
                );
            }
            Tag::Strong => self.start_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Emphasis => self.start_style(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strikethrough => {
                self.start_style(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { .. } => {
                self.start_style(
                    Style::default()
                        .fg(palette::info())
                        .add_modifier(Modifier::UNDERLINED),
                );
            }
            Tag::CodeBlock(_) => {
                self.start_block();
                self.start_style(Style::default().fg(palette::warning()));
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
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => {
                self.end_style();
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
                    self.lines.extend(render_table(&table, self.width));
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
        if !self.line.is_empty() {
            self.lines.push(Line::from(std::mem::take(&mut self.line)));
        }
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

    fn finish(mut self) -> Vec<Line<'static>> {
        self.finish_line();
        while self.lines.last().is_some_and(|line| line.spans.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
