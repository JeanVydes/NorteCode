//! A closable overlay for reading a plan with numbered Markdown lines.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius, draw_box};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// The plan currently shown, if any.
#[derive(Serialize, Default)]
pub(crate) struct PlanOverlay {
    #[serde(skip)]
    plan: Option<norte_code_wire::PlanSummary>,
    #[serde(skip)]
    scroll: u16,
}

impl PlanOverlay {
    /// Show a plan from the beginning.
    pub(crate) fn open(&mut self, plan: norte_code_wire::PlanSummary) {
        self.plan = Some(plan);
        self.scroll = 0;
    }

    /// Whether a plan is showing.
    pub(crate) fn is_open(&self) -> bool {
        self.plan.is_some()
    }

    /// Close the overlay.
    pub(crate) fn close(&mut self) {
        self.plan = None;
        self.scroll = 0;
    }

    fn lines(&self) -> Vec<String> {
        self.plan
            .as_ref()
            .map(|plan| plan.content.lines().map(str::to_owned).collect())
            .unwrap_or_default()
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

        let lines = self.lines();
        let gutter = 5u16;
        let first = self
            .scroll
            .min(u16::try_from(lines.len()).unwrap_or(u16::MAX));
        for (row, (index, line)) in
            (inner.y + 2..).zip(lines.iter().enumerate().skip(first as usize))
        {
            if row >= inner.y + inner.height {
                break;
            }
            let number = format!("{:>4}", index + 1);
            buf.set_stringn(
                inner.x + 1,
                row,
                &number,
                gutter as usize,
                Style::default().fg(theme.muted).bg(theme.background),
            );
            let body = if line.trim_start().starts_with('#') {
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground).bg(theme.background)
            };
            buf.set_stringn(
                inner.x + 1 + gutter,
                row,
                line,
                usize::from(inner.width.saturating_sub(gutter + 1)),
                body,
            );
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
                    self.scroll = (self.scroll + 1)
                        .min(u16::try_from(self.lines().len()).unwrap_or(u16::MAX));
                    Outcome::Consumed
                }
                _ => Outcome::Ignored,
            },
            _ => Outcome::Ignored,
        }
    }
}
