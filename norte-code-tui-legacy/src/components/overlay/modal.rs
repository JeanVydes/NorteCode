//! A box drawn over the screen.
//!
//! The size adapts to the terminal instead of being fixed: a modal that asks
//! for 72 columns in a 40-column terminal is the bug this replaces.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::components::{palette, spacing};
use crate::ui_model::ThemeMode;

const MAX_WIDTH_PERCENT: u32 = 90;
const MAX_HEIGHT_PERCENT: u32 = 80;
const MAX_WIDTH: u16 = 76;
pub(crate) const MAX_HEIGHT: u16 = 40;
const MIN_WIDTH: u16 = 30;
const MIN_HEIGHT: u8 = 8;

/// The rows a modal spends on its border, its padding and its controls.
const MODAL_CHROME: u16 = 6;

/// A modal box: a title, a body and a controls line.
pub(crate) struct Modal<'a> {
    pub(crate) title: &'a str,
    pub(crate) controls: &'a str,
    pub(crate) body: Vec<Line<'static>>,
    pub(crate) theme: ThemeMode,
    /// The row of the body the reader is on, painted as selected.
    ///
    /// The modal scrolls itself so that row stays inside the window; only the
    /// modal knows how tall the window really is, after borders, padding and
    /// the controls line.
    pub(crate) selected: Option<usize>,
}

impl Modal<'_> {
    /// The area the modal covers, centered and never larger than the screen.
    fn area(body_rows: u16, frame: Rect) -> Rect {
        let width_limit = u16::try_from(u32::from(frame.width) * MAX_WIDTH_PERCENT / 100)
            .unwrap_or(frame.width)
            .min(MAX_WIDTH);
        let width = width_limit.max(MIN_WIDTH).min(frame.width);
        let height_limit = u16::try_from(u32::from(frame.height) * MAX_HEIGHT_PERCENT / 100)
            .unwrap_or(frame.height)
            .min(MAX_HEIGHT);
        // Borders, the padding of two rows and the controls line.
        let height = body_rows
            .saturating_add(MODAL_CHROME)
            .max(u16::from(MIN_HEIGHT))
            .min(height_limit)
            .min(frame.height);
        Rect {
            x: frame.x + frame.width.saturating_sub(width) / 2,
            y: frame.y + frame.height.saturating_sub(height) / 2,
            width,
            height,
        }
    }

    /// The first row the body shows, so the selection stays in the window.
    fn scroll_for(&self, window: u16) -> u16 {
        let Some(selected) = self.selected else {
            return 0;
        };
        let window = usize::from(window);
        if window == 0 {
            return 0;
        }
        let total = self.body.len();
        let lowest = selected.saturating_sub(window - 1);
        let highest = total.saturating_sub(window);
        u16::try_from(lowest.min(highest)).unwrap_or(0)
    }

    /// Draws the modal. The scrim must already be painted.
    pub(crate) fn render(&self, frame: &mut Frame<'_>) {
        let rows = u16::try_from(self.body.len()).unwrap_or(u16::MAX);
        let area = Self::area(rows, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(palette::border_style(self.theme))
            .padding(spacing::modal())
            .style(palette::modal_style(self.theme))
            .title(Line::from(Span::styled(
                format!(" {} ", self.title),
                Style::default()
                    .fg(palette::accent(self.theme))
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(u16::from(!self.controls.is_empty())),
            ])
            .split(inner);

        let mut body = self.body.clone();
        if let Some(line) = self.selected.and_then(|index| body.get_mut(index)) {
            line.style = palette::selected_style(self.theme).add_modifier(Modifier::BOLD);
        }
        let scroll = self.scroll_for(sections[0].height);
        frame.render_widget(
            Paragraph::new(Text::from(body))
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0))
                .style(palette::modal_style(self.theme)),
            sections[0],
        );
        if !self.controls.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    self.controls.to_owned(),
                    palette::muted_style(self.theme),
                ))),
                sections[1],
            );
        }
    }
}

#[cfg(test)]
#[path = "modal_tests.rs"]
mod tests;
