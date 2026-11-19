//! How the plan review is drawn: a document viewer, not a wall of text.
//!
//! The content is shown the way an editor shows a file: a header with the
//! name, a numbered gutter, a scrollbar and a row of buttons the reader can
//! click as well as key.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
};
use unicode_width::UnicodeWidthStr;

use super::{InputMode, PlanReviewView, PromptKind};
use crate::components::{palette, spacing};
use crate::ui_model::{ThemeMode, UiModel};

/// The columns the numbered gutter takes, markdown included.
const GUTTER_WIDTH: u16 = 6;

/// The buttons the footer offers, in the order they are drawn.
pub(super) const BUTTONS: [&str; 3] = ["approve", "reject", "feedback"];

impl PlanReviewView {
    /// Draws the review over a scrim.
    pub(crate) fn draw(&mut self, frame: &mut Frame<'_>, model: &UiModel) {
        if !self.is_open() {
            return;
        }
        let theme = model.preferences.theme;
        crate::components::paint_scrim(frame, theme);
        let area = dialog(frame.area());
        frame.render_widget(Clear, area);

        let is_plan = self.prompt_kind == PromptKind::Plan;
        let title = if is_plan {
            " plan review "
        } else {
            " question "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(palette::border_style(theme))
            .style(palette::modal_style(theme))
            .title(Line::from(Span::styled(
                title.to_owned(),
                Style::default()
                    .fg(palette::accent(theme))
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(4),
                Constraint::Length(3),
            ])
            .split(inner);

        self.render_header(frame, sections[0], theme, is_plan);
        if is_plan {
            self.render_document(frame, sections[1], theme);
        } else {
            self.render_question(frame, sections[1], model);
        }
        self.render_footer(frame, sections[2], theme, is_plan, model);
    }

    /// The line above the document: what it is and how big.
    fn render_header(&self, frame: &mut Frame<'_>, area: Rect, theme: ThemeMode, is_plan: bool) {
        let lines = self.content.lines().count();
        let label = if is_plan {
            format!("{lines} lines")
        } else {
            self.question.clone().unwrap_or_default()
        };
        let hint = if is_plan {
            "  j/k scroll, g/G top and bottom"
        } else {
            ""
        };
        let line = Line::from(vec![
            Span::styled(label, palette::text_style(theme)),
            Span::styled(hint.to_owned(), palette::muted_style(theme)),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(palette::modal_style(theme)),
            area,
        );
    }

    /// The document, rendered as the markdown it is.
    ///
    /// A plan is written in markdown, so the viewer draws headings, lists,
    /// code and tables the way the transcript does, with a numbered gutter
    /// beside it. Showing the raw source would leave "##" and "**" in the
    /// reader's way.
    fn render_document(&self, frame: &mut Frame<'_>, area: Rect, theme: ThemeMode) {
        let text_width = area.width.saturating_sub(GUTTER_WIDTH);
        let rendered = crate::components::markdown::render(&self.content, text_width);
        let height = usize::from(area.height);
        let total = rendered.len();
        let scroll = usize::from(
            self.scroll
                .min(u16::try_from(total.saturating_sub(height)).unwrap_or(u16::MAX)),
        );
        let gutter = total.to_string().len().max(3);
        let mut body = Vec::with_capacity(height);
        for (index, line) in rendered.iter().enumerate().skip(scroll).take(height) {
            let mut spans = vec![Span::styled(
                format!("{:>gutter$}  ", index + 1),
                Style::default().fg(palette::border(theme)),
            )];
            spans.extend(line.spans.iter().cloned());
            body.push(Line::from(spans).style(line.style));
        }
        if body.is_empty() {
            body.push(Line::from(Span::styled(
                "The plan is empty.",
                palette::muted_style(theme),
            )));
        }
        frame.render_widget(
            Paragraph::new(ratatui::text::Text::from(body))
                .style(palette::modal_style(theme))
                .block(Block::default().padding(spacing::panel())),
            area,
        );
        if total > height {
            let mut state = ScrollbarState::new(total.saturating_sub(height)).position(scroll);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .style(palette::border_style(theme)),
                area,
                &mut state,
            );
        }
    }

    /// The question: the options, each with its marker.
    fn render_question(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let theme = model.preferences.theme;
        let mut body = Vec::new();
        for (index, option) in self.options.iter().enumerate() {
            let selected = index == self.selected_option;
            let checked = self.multiple && self.selected_options.contains(&index);
            let marker = if checked {
                "[x]"
            } else if selected {
                "[>]"
            } else {
                "[ ]"
            };
            let style = if selected {
                palette::selected_style(theme).add_modifier(Modifier::BOLD)
            } else {
                palette::text_style(theme)
            };
            body.push(Line::from(Span::styled(
                format!("{marker} {option}"),
                style,
            )));
        }
        if body.is_empty() {
            body.push(Line::from(Span::styled(
                "Type an answer with f, then Enter.",
                palette::muted_style(theme),
            )));
        }
        frame.render_widget(
            Paragraph::new(ratatui::text::Text::from(body))
                .style(palette::modal_style(theme))
                .block(Block::default().padding(spacing::panel())),
            area,
        );
    }

    /// The buttons and what they do, which the reader can also click.
    fn render_footer(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: ThemeMode,
        is_plan: bool,
        model: &UiModel,
    ) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(2)])
            .split(area);
        let labels: Vec<&str> = if is_plan {
            BUTTONS.to_vec()
        } else {
            vec!["answer", "submit"]
        };
        let mut spans = Vec::new();
        for (index, label) in labels.iter().enumerate() {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(palette::background(theme))
                    .bg(if index == 0 {
                        palette::accent(theme)
                    } else {
                        palette::surface_alt(theme)
                    })
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw("  "));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).alignment(Alignment::Left),
            rows[0],
        );
        let response = match self.input_mode {
            InputMode::Respond => String::new(),
            InputMode::Feedback | InputMode::Answer => format!("{}_", self.response),
        };
        let controls = if response.is_empty() {
            if is_plan {
                "Enter or a approve, d reject, f feedback, j/k scroll".to_owned()
            } else if self.multiple {
                "j/k move, Space select, Enter submit, f answer".to_owned()
            } else {
                "j/k move, Enter select, f answer".to_owned()
            }
        } else {
            format!("{response}   (Enter sends, Esc cancels)")
        };
        let _ = model;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                controls,
                palette::muted_style(theme),
            )))
            .alignment(Alignment::Left),
            rows[1],
        );
    }

    /// The buttons as they were drawn: the row and the columns each spans.
    ///
    /// The hit test reads this, so a click lands on what the reader saw even
    /// after the layout changes.
    pub(crate) fn buttons(&self, area: Rect) -> Vec<(Rect, usize)> {
        let area = dialog(area);
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        let footer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(4),
                Constraint::Length(3),
            ])
            .split(inner)[2];
        let labels: &[&str] = if self.prompt_kind == PromptKind::Plan {
            &BUTTONS
        } else {
            &["answer", "submit"]
        };
        let mut column = inner.x;
        let mut buttons = Vec::with_capacity(labels.len());
        for (index, label) in labels.iter().enumerate() {
            let width = u16::try_from(label.chars().count() + 2).unwrap_or(0);
            buttons.push((
                Rect {
                    x: column,
                    y: footer.y,
                    width,
                    height: 1,
                },
                index,
            ));
            column = column.saturating_add(width).saturating_add(2);
        }
        buttons
    }

    /// The button at a screen row and column, when the reader clicked one.
    pub(crate) fn button_at(&self, column: u16, row: u16, area: Rect) -> Option<usize> {
        self.buttons(area)
            .into_iter()
            .find(|(rect, _)| {
                column >= rect.x && column < rect.x.saturating_add(rect.width) && row == rect.y
            })
            .map(|(_, index)| index)
    }
}

/// The area the dialog covers.
fn dialog(area: Rect) -> Rect {
    let width = area.width.saturating_mul(88) / 100;
    let height = area.height.saturating_mul(84) / 100;
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// The width of a string once drawn.
#[allow(dead_code)]
fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}
