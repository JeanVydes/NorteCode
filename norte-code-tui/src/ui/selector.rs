//! A one-row cycler for the status strip: a dim label and a vivid value
//! that steps through a fixed list. `Space`, `Enter`, `Left`/`Right` or a
//! click advance it.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// A focusable one-row cycler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Selector {
    /// Dim caption drawn before the value.
    pub(crate) label: String,
    options: Vec<String>,
    index: usize,
    area: Rect,
}

impl Selector {
    /// Creates a selector cycling through `options`, starting at `initial`.
    pub(crate) fn new(label: impl Into<String>, options: Vec<&str>, initial: usize) -> Self {
        let index = initial.min(options.len().saturating_sub(1));
        Self {
            label: label.into(),
            options: options.into_iter().map(str::to_string).collect(),
            index,
            area: Rect::new(0, 0, 0, 0),
        }
    }

    /// The value currently shown.
    pub(crate) fn value(&self) -> &str {
        &self.options[self.index]
    }

    /// Step to the next (or previous, with `backwards`) option.
    pub(crate) fn cycle(&mut self, backwards: bool) {
        if self.options.is_empty() {
            return;
        }
        if backwards {
            self.index = self.index.checked_sub(1).unwrap_or(self.options.len() - 1);
        } else {
            self.index = (self.index + 1) % self.options.len();
        }
    }

    /// Width in cells of the whole selector.
    pub(crate) fn width(&self) -> u16 {
        let cells = self.label.chars().count() + self.value().chars().count() + 6;
        u16::try_from(cells).unwrap_or(u16::MAX)
    }
}

impl View for Selector {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "selector"
    }

    fn measure(&self) -> Option<(u16, u16)> {
        Some((self.width(), 1))
    }

    fn outline(&self) -> Option<String> {
        Some(format!("{}='{}'", self.label, self.value()))
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        self.area = area;
        let focused = cx.is_focused();
        let surface = theme.secondary;
        buf.set_style(area, Style::default().bg(surface));
        let label = &self.label;
        buf.set_stringn(
            area.x,
            area.y,
            label,
            label.chars().count(),
            Style::default().fg(theme.muted).bg(surface),
        );
        let value = self.value().to_string();
        let value_x = area.x + u16::try_from(label.chars().count()).unwrap_or(u16::MAX) + 1;
        let mut value_style = Style::default()
            .fg(theme.accent)
            .bg(surface)
            .add_modifier(Modifier::BOLD);
        if focused {
            value_style = value_style.add_modifier(Modifier::REVERSED);
        }
        buf.set_stringn(value_x, area.y, &value, value.chars().count(), value_style);
        let caret_x = value_x + u16::try_from(value.chars().count()).unwrap_or(u16::MAX) + 1;
        buf.set_stringn(
            caret_x,
            area.y,
            "▾",
            1,
            Style::default().fg(theme.muted).bg(surface),
        );
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        let focused = cx.is_focused();
        match event {
            InputEvent::Mouse(mouse) => {
                use crossterm::event::{MouseButton, MouseEventKind};
                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && mouse.column >= self.area.x
                    && mouse.column < self.area.x + self.area.width
                    && mouse.row == self.area.y
                {
                    self.cycle(false);
                    return Outcome::Consumed;
                }
                Outcome::Ignored
            }
            InputEvent::Key(key) => {
                use crossterm::event::{KeyCode, KeyEventKind};
                if key.kind != KeyEventKind::Press || !focused {
                    return Outcome::Ignored;
                }
                match key.code {
                    KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right => {
                        self.cycle(false);
                        Outcome::Consumed
                    }
                    KeyCode::Left => {
                        self.cycle(true);
                        Outcome::Consumed
                    }
                    _ => Outcome::Ignored,
                }
            }
            _ => Outcome::Ignored,
        }
    }

    fn focusable(&self) -> bool {
        true
    }
}
