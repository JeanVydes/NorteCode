//! Transient feedback for composer commands.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use serde::Serialize;
use std::{any::Any, time::Duration};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

const LIFETIME: Duration = Duration::from_secs(5);
const SLIDE: Duration = Duration::from_millis(300);

#[derive(Default, Serialize)]
pub(crate) struct CommandDock {
    message: String,
    progress: bool,
    visible: bool,
    chat_visible: bool,
    #[serde(skip)]
    elapsed: Duration,
}
impl CommandDock {
    pub(crate) fn show(&mut self, message: impl Into<String>, progress: bool) {
        self.message = message.into();
        self.progress = progress;
        self.visible = true;
        self.elapsed = Duration::ZERO;
    }
    pub(crate) fn finish_progress(&mut self, message: impl Into<String>) {
        if self.visible && self.progress {
            self.show(message, false);
        }
    }
    pub(crate) fn is_active(&self) -> bool {
        self.visible
    }
    pub(crate) fn set_chat_visible(&mut self, visible: bool) {
        self.chat_visible = visible;
    }
}
impl View for CommandDock {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "command_dock"
    }
    fn tick(&mut self, delta: Duration, _: &mut Context) {
        if self.visible {
            self.elapsed = self.elapsed.saturating_add(delta);
            if !self.progress && self.elapsed >= LIFETIME + SLIDE {
                self.visible = false;
            }
        }
    }
    fn render(&mut self, area: Rect, buf: &mut Buffer, _: &mut Context, theme: &Theme) {
        if !self.visible || !self.chat_visible || area.width < 8 || area.height < 10 {
            return;
        }
        // Reveal upwards from the composer, then retract back underneath it.
        let fraction = if self.elapsed < SLIDE {
            self.elapsed.as_secs_f64() / SLIDE.as_secs_f64()
        } else if !self.progress && self.elapsed > LIFETIME {
            (LIFETIME + SLIDE)
                .saturating_sub(self.elapsed)
                .as_secs_f64()
                / SLIDE.as_secs_f64()
        } else {
            1.0
        };
        let rows = if fraction < 0.33 {
            0
        } else if fraction < 0.66 {
            1
        } else {
            2
        };
        if rows == 0 {
            return;
        }
        let inset = u16::from(area.width >= 60);
        let x = area.x + inset;
        let width = area.width.saturating_sub(inset * 2);
        let bottom = area.bottom().saturating_sub(7);
        let y = bottom.saturating_sub(rows);
        let style = Style::default().bg(theme.secondary).fg(theme.foreground);
        for row in y..bottom {
            buf.set_stringn(
                x,
                row,
                " ".repeat(usize::from(width)),
                usize::from(width),
                style,
            );
        }
        if rows == 2 {
            buf.set_stringn(
                x,
                y,
                "─".repeat(usize::from(width)),
                usize::from(width),
                style.fg(theme.border),
            );
        }
        let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let icon = if self.progress {
            spinner[(self.elapsed.as_millis() / 100 % 10) as usize]
        } else {
            "✓"
        };
        buf.set_stringn(
            x + 1,
            bottom - 1,
            icon,
            1,
            style.fg(theme.accent).add_modifier(Modifier::BOLD),
        );
        buf.set_stringn(
            x + 3,
            bottom - 1,
            &self.message,
            usize::from(width.saturating_sub(4)),
            style,
        );
    }
    fn handle_event(&mut self, _: &InputEvent, _: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}

#[cfg(test)]
#[path = "command_dock_tests.rs"]
mod tests;
