//! A closable overlay for reading a plan with numbered Markdown lines.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius, draw_box};
use terminalui::{Context, InputEvent, Outcome, Theme, View};
use unicode_width::UnicodeWidthStr;

use super::markdown::{Markdown, open_link};

/// The plan currently shown, if any.
#[derive(Serialize, Default)]
pub(crate) struct PlanOverlay {
    #[serde(skip)]
    plan: Option<norte_code_wire::PlanSummary>,
    #[serde(skip)]
    doc: Markdown,
    #[serde(skip)]
    width: u16,
    #[serde(skip)]
    scroll: u16,
    #[serde(skip)]
    area: Rect,
}

impl PlanOverlay {
    /// Show a plan from the beginning.
    pub(crate) fn open(&mut self, plan: norte_code_wire::PlanSummary) {
        self.plan = Some(plan);
        self.doc = Markdown::default();
        self.width = 0;
        self.scroll = 0;
        self.area = Rect::new(0, 0, 0, 0);
    }

    /// Whether a plan is showing.
    pub(crate) fn is_open(&self) -> bool {
        self.plan.is_some()
    }

    /// Close the overlay.
    pub(crate) fn close(&mut self) {
        self.plan = None;
        self.doc = Markdown::default();
        self.scroll = 0;
    }

    fn body_width(inner: Rect) -> u16 {
        inner.width.saturating_sub(7)
    }
}

impl View for PlanOverlay {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "plan_overlay"
    }
    fn focusable(&self) -> bool {
        true
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        let Some(plan) = &self.plan else {
            return;
        };
        self.area = area;
        buf.set_style(area, Style::default().bg(theme.background));
        let box_rect = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let chrome = Chrome {
            border: Some(Border {
                set: BorderSet::Rounded,
                color: Some(theme.border),
                radius: Radius::Small,
            }),
            background: None,
            radius: Radius::Small,
            padding: Padding {
                horizontal: 0,
                vertical: 0,
            },
            border_gradient: None,
            fill_gradient: None,
        };
        let inner = draw_box(buf, box_rect, &chrome, theme);
        let title = plan.title.clone().unwrap_or_else(|| plan.id.clone());
        buf.set_stringn(
            inner.x + 1,
            inner.y,
            format!("{title} · Esc closes"),
            usize::from(inner.width.saturating_sub(2)),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );

        let width = Self::body_width(inner);
        if self.width != width {
            self.width = width;
            self.doc = Markdown::render(&plan.content, width, theme);
        }
        let max_scroll = self
            .doc
            .lines
            .len()
            .saturating_sub(usize::from(inner.height.saturating_sub(2)));
        self.scroll = self
            .scroll
            .min(u16::try_from(max_scroll).unwrap_or(u16::MAX));

        for row in 0..inner.height.saturating_sub(2) {
            let index = usize::from(row.saturating_add(self.scroll));
            let Some(line) = self.doc.lines.get(index) else {
                break;
            };
            let y = inner.y + 2 + row;
            let number = format!("{:>4}", index + 1);
            buf.set_stringn(
                inner.x + 1,
                y,
                number,
                5,
                Style::default().fg(theme.muted).bg(theme.background),
            );
            let mut column = 6u16;
            for span in &line.spans {
                buf.set_stringn(
                    inner.x + 1 + column,
                    y,
                    span.content.as_ref(),
                    usize::from(inner.width.saturating_sub(column + 1)),
                    span.style,
                );
                column =
                    column.saturating_add(u16::try_from(span.content.width()).unwrap_or(u16::MAX));
            }
        }
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        let focused = cx.is_focused();
        match event {
            InputEvent::Key(key) if focused => match key.code {
                crossterm::event::KeyCode::Esc => {
                    self.close();
                    Outcome::Consumed
                }
                crossterm::event::KeyCode::Up => {
                    self.scroll = self.scroll.saturating_sub(1);
                    Outcome::Consumed
                }
                crossterm::event::KeyCode::Down => {
                    self.scroll = self.scroll.saturating_add(1);
                    Outcome::Consumed
                }
                _ => Outcome::Ignored,
            },
            InputEvent::Mouse(mouse)
                if mouse.kind
                    == crossterm::event::MouseEventKind::Down(
                        crossterm::event::MouseButton::Left,
                    ) =>
            {
                let row = usize::from(mouse.row.saturating_sub(self.area.y).saturating_sub(4));
                let column =
                    usize::from(mouse.column.saturating_sub(self.area.x).saturating_sub(10));
                let doc_row = row.saturating_add(usize::from(self.scroll));
                if let Some(url) = self.doc.link_at(doc_row, column) {
                    open_link(url);
                    Outcome::Consumed
                } else {
                    Outcome::Ignored
                }
            }
            _ => Outcome::Ignored,
        }
    }
}

#[cfg(test)]
#[path = "plan_overlay_tests.rs"]
mod tests;
