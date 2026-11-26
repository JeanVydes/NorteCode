//! Confirmation for clearing the connected service's operational data.
use std::{any::Any, time::Duration};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
};
use serde::Serialize;
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius, draw_box};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize)]
enum Choice {
    #[default]
    Cancel,
    Clear,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClearAction {
    Cancel,
    Confirm,
}

#[derive(Default, Serialize)]
pub(crate) struct ClearDataOverlay {
    open: bool,
    choice: Choice,
    error: Option<String>,
    #[serde(skip)]
    elapsed: Duration,
    #[serde(skip)]
    cancel_area: Rect,
    #[serde(skip)]
    confirm_area: Rect,
}
impl ClearDataOverlay {
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.choice = Choice::Cancel;
        self.error = None;
        self.elapsed = Duration::ZERO;
    }
    pub(crate) fn close(&mut self) {
        self.open = false;
    }
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }
    pub(crate) fn fail(&mut self, message: String) {
        self.error = Some(message);
    }
    pub(crate) fn key(&mut self, key: KeyEvent) -> Option<ClearAction> {
        if key.modifiers != KeyModifiers::NONE && key.code != KeyCode::BackTab {
            return None;
        }
        match key.code {
            KeyCode::Esc => Some(ClearAction::Cancel),
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                self.choice = if self.choice == Choice::Cancel {
                    Choice::Clear
                } else {
                    Choice::Cancel
                };
                None
            }
            KeyCode::Enter => Some(if self.choice == Choice::Cancel {
                ClearAction::Cancel
            } else {
                ClearAction::Confirm
            }),
            _ => None,
        }
    }
    pub(crate) fn mouse(&mut self, mouse: MouseEvent) -> Option<ClearAction> {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return None;
        }
        let pos = Position::new(mouse.column, mouse.row);
        if self.cancel_area.contains(pos) {
            Some(ClearAction::Cancel)
        } else if self.confirm_area.contains(pos) {
            Some(ClearAction::Confirm)
        } else {
            None
        }
    }
}
impl ClearDataOverlay {
    fn render_compact(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.cancel_area = Rect::default();
        self.confirm_area = Rect::default();
        if area.width == 0 || area.height == 0 {
            return;
        }
        let style = Style::default().bg(theme.secondary).fg(theme.destructive);
        let message = if self.choice == Choice::Cancel {
            "Clear data? Cancel  ←→ Enter Esc"
        } else {
            "Clear data? CLEAR  ←→ Enter Esc"
        };
        let y = area.y + area.height / 2;
        buf.set_stringn(
            area.x,
            y,
            " ".repeat(usize::from(area.width)),
            usize::from(area.width),
            style,
        );
        buf.set_stringn(area.x, y, message, usize::from(area.width), style);
    }

    fn render_actions(&mut self, inner: Rect, buf: &mut Buffer, theme: &Theme) {
        let plain = Style::default().bg(theme.secondary).fg(theme.foreground);
        let muted = plain.fg(theme.muted);
        let button_y = inner.bottom().saturating_sub(2);
        let cancel_x = inner.x;
        let confirm_x = inner
            .x
            .saturating_add(if inner.width >= 28 { 14 } else { 10 });
        self.cancel_area = Rect::new(cancel_x, button_y, 10.min(inner.width), 1);
        self.confirm_area = Rect::new(
            confirm_x,
            button_y,
            14.min(inner.right().saturating_sub(confirm_x)),
            1,
        );
        let cancel_style = if self.choice == Choice::Cancel {
            plain
                .bg(theme.accent)
                .fg(theme.background)
                .add_modifier(Modifier::BOLD)
        } else {
            muted
        };
        let confirm_style = if self.choice == Choice::Clear {
            plain
                .bg(theme.destructive)
                .fg(theme.background)
                .add_modifier(Modifier::BOLD)
        } else {
            plain.fg(theme.destructive)
        };
        buf.set_stringn(
            cancel_x,
            button_y,
            "  Cancel  ",
            usize::from(self.cancel_area.width),
            cancel_style,
        );
        buf.set_stringn(
            confirm_x,
            button_y,
            "  Clear data  ",
            usize::from(self.confirm_area.width),
            confirm_style,
        );
        let footer = self
            .error
            .as_deref()
            .unwrap_or("←/→ choose  ·  Enter select  ·  Esc cancel");
        buf.set_stringn(
            inner.x,
            inner.bottom().saturating_sub(1),
            footer,
            usize::from(inner.width),
            if self.error.is_some() {
                plain.fg(theme.destructive)
            } else {
                muted
            },
        );
    }
}

impl View for ClearDataOverlay {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "clear_data_overlay"
    }
    fn tick(&mut self, delta: Duration, _: &mut Context) {
        if self.open {
            self.elapsed = self.elapsed.saturating_add(delta);
        }
    }
    fn render(&mut self, area: Rect, buf: &mut Buffer, _: &mut Context, theme: &Theme) {
        if !self.open {
            return;
        }
        let width = area.width.saturating_sub(2).min(68);
        let height = area.height.saturating_sub(2).min(14);
        if width < 24 || height < 10 {
            self.render_compact(area, buf, theme);
            return;
        }
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let pulse = (self.elapsed.as_secs_f64() * 2.0).sin().mul_add(0.12, 0.32);
        let chrome = Chrome {
            border: Some(Border {
                set: BorderSet::Rounded,
                color: Some(terminalui::lerp_color(
                    theme.border,
                    theme.destructive,
                    pulse,
                )),
                radius: Radius::Small,
            }),
            background: Some(theme.secondary),
            radius: Radius::Small,
            padding: Padding {
                horizontal: 2,
                vertical: 0,
            },
            border_gradient: None,
            fill_gradient: None,
        };
        let inner = draw_box(buf, rect, &chrome, theme);
        let plain = Style::default().bg(theme.secondary).fg(theme.foreground);
        let title = plain.fg(theme.destructive).add_modifier(Modifier::BOLD);
        let muted = plain.fg(theme.muted);
        let wide = inner.width >= 60 && inner.height >= 10;
        let lines = if wide {
            [
                ("CLEAR SERVICE DATA", title),
                ("", plain),
                (
                    "Erase sessions, telemetry, replay events and uploads.",
                    plain,
                ),
                ("This affects everyone connected to this service.", muted),
                ("", plain),
                (
                    "Configuration, providers, models, plugins and skills stay.",
                    muted,
                ),
            ]
        } else {
            [
                ("CLEAR SERVICE DATA", title),
                ("Sessions & metrics", plain),
                ("Replays & uploads", plain),
                ("All service users", plain.fg(theme.destructive)),
                ("Settings kept", muted),
                ("", plain),
            ]
        };
        for (row, (text, style)) in lines.iter().enumerate() {
            if row < usize::from(inner.height.saturating_sub(3)) {
                buf.set_stringn(
                    inner.x,
                    inner.y + u16::try_from(row).unwrap_or(0),
                    text,
                    usize::from(inner.width),
                    *style,
                );
            }
        }
        self.render_actions(inner, buf, theme);
    }
    fn handle_event(&mut self, _: &InputEvent, _: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}

#[cfg(test)]
#[path = "clear_data_overlay_tests.rs"]
mod tests;
