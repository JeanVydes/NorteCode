//! Read-only host information presented over the current screen.

use std::any::Any;
use std::time::Duration;

use crossterm::event::{KeyCode, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
};
use serde::Serialize;
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius, draw_box};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

use super::detail_panel::Field;
use crate::ui_model::UiModel;

#[derive(Default, Serialize)]
pub(crate) struct HostOverlay {
    open: bool,
    #[serde(skip)]
    fields: Vec<Field>,
    #[serde(skip)]
    area: Rect,
    scroll: usize,
    #[serde(skip)]
    scroll_limit: usize,
    #[serde(skip)]
    elapsed: Duration,
}

impl HostOverlay {
    pub(crate) fn open(&mut self, model: &UiModel) {
        self.open = true;
        self.scroll = 0;
        self.sync(model);
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn sync(&mut self, model: &UiModel) {
        if !self.open {
            return;
        }
        let mut fields = super::host_info::fields(model);
        fields.push(Field::new("sessions", model.sessions.len().to_string()));
        fields.push(Field::new("models", model.models.len().to_string()));
        fields.push(Field::new(
            "plugins",
            format!(
                "{} loaded / {} installed",
                model.plugins.iter().filter(|plugin| plugin.loaded).count(),
                model.plugins.len()
            ),
        ));
        self.fields = fields;
    }

    fn move_scroll(&mut self, amount: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(amount)
            .min(self.scroll_limit);
    }
}

impl View for HostOverlay {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "host_overlay"
    }
    fn focusable(&self) -> bool {
        self.open
    }
    fn tick(&mut self, delta: Duration, _cx: &mut Context) {
        if self.open {
            self.elapsed = self.elapsed.saturating_add(delta);
        }
    }
    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        if !self.open {
            return;
        }
        self.area = area;
        let width = area.width.saturating_sub(4).min(92);
        let height = area.height.saturating_sub(2).min(36);
        if width < 8 || height < 5 {
            return;
        }
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let pulse = (self.elapsed.as_secs_f64() * 1.5).sin().mul_add(0.12, 0.12);
        let border = terminalui::lerp_color(theme.border, theme.accent, pulse);
        let chrome = Chrome {
            border: Some(Border {
                set: BorderSet::Rounded,
                color: Some(border),
                radius: Radius::Small,
            }),
            background: Some(theme.secondary),
            radius: Radius::Small,
            padding: Padding {
                horizontal: 1,
                vertical: 0,
            },
            border_gradient: None,
            fill_gradient: None,
        };
        let inner = draw_box(buf, rect, &chrome, theme);
        buf.set_stringn(
            inner.x,
            inner.y,
            "HOST",
            usize::from(inner.width),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        let label_width = (inner.width / 3)
            .clamp(12, 24)
            .min(inner.width.saturating_sub(1));
        let value_width = usize::from(inner.width.saturating_sub(label_width));
        let rows = wrap_fields(&self.fields, value_width);
        let visible = usize::from(inner.height.saturating_sub(3));
        self.scroll_limit = rows.len().saturating_sub(visible.max(1));
        self.scroll = self.scroll.min(self.scroll_limit);
        for (row, (label, value)) in rows.iter().skip(self.scroll).take(visible).enumerate() {
            let y = inner.y + 1 + u16::try_from(row).unwrap_or(0);
            buf.set_stringn(
                inner.x,
                y,
                label,
                usize::from(label_width),
                Style::default().fg(theme.muted),
            );
            buf.set_stringn(
                inner.x + label_width,
                y,
                value,
                value_width,
                Style::default().fg(theme.foreground),
            );
        }
        buf.set_stringn(
            inner.x,
            inner.bottom().saturating_sub(1),
            "↑/↓ scroll  ·  Esc close",
            usize::from(inner.width),
            Style::default().fg(theme.muted),
        );
    }
    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        if !self.open {
            return Outcome::Ignored;
        }
        match event {
            InputEvent::Key(key) => match key.code {
                KeyCode::Esc => self.open = false,
                KeyCode::Up | KeyCode::Char('k') => self.move_scroll(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_scroll(1),
                KeyCode::PageUp => self.move_scroll(-10),
                KeyCode::PageDown => self.move_scroll(10),
                KeyCode::Home => self.scroll = 0,
                KeyCode::End => self.scroll = self.scroll_limit,
                _ => return Outcome::Ignored,
            },
            InputEvent::Mouse(mouse)
                if self.area.contains(Position::new(mouse.column, mouse.row)) =>
            {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.move_scroll(-3),
                    MouseEventKind::ScrollDown => self.move_scroll(3),
                    _ => return Outcome::Ignored,
                }
            }
            _ => return Outcome::Ignored,
        }
        Outcome::Consumed
    }
}

fn wrap_fields(fields: &[Field], value_width: usize) -> Vec<(&str, String)> {
    let mut rows = Vec::new();
    for field in fields {
        let mut text = String::new();
        let mut width = 0;
        let mut first = true;
        for character in field.value.chars() {
            let character_width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
            if width + character_width > value_width && !text.is_empty() {
                rows.push((
                    if first { field.label.as_str() } else { "" },
                    std::mem::take(&mut text),
                ));
                first = false;
                width = 0;
            }
            if character_width <= value_width {
                text.push(character);
                width += character_width;
            }
        }
        rows.push((if first { field.label.as_str() } else { "" }, text));
    }
    rows
}

#[cfg(test)]
#[path = "host_overlay_tests.rs"]
mod tests;
