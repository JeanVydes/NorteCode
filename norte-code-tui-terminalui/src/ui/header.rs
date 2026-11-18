//! The slim header chrome: wordmark on the left, the turn state and the
//! active theme on the right.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

use crate::ui_model::TurnState;

/// One-row header rendered above everything.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Header {
    /// The turn the header reports.
    pub(crate) turn: TurnState,
    /// The theme the header reports.
    pub(crate) theme_name: String,
}

impl Header {
    /// Creates a header reading from the given state.
    pub(crate) fn new(turn: TurnState, theme_name: String) -> Self {
        Self { turn, theme_name }
    }
}

impl View for Header {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "header"
    }

    fn measure(&self) -> Option<(u16, u16)> {
        Some((0, 1))
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        buf.set_style(area, Style::default().bg(theme.secondary));
        let mark = "◈ NORTE";
        buf.set_stringn(
            area.x + 1,
            area.y,
            mark,
            mark.chars().count(),
            Style::default()
                .fg(theme.accent)
                .bg(theme.secondary)
                .add_modifier(Modifier::BOLD),
        );
        buf.set_stringn(
            area.x + 9,
            area.y,
            "mission control",
            "mission control".chars().count(),
            Style::default().fg(theme.muted).bg(theme.secondary),
        );

        let (dot, label, color) = match self.turn {
            TurnState::Idle => ("●", "ready", theme.success),
            TurnState::Connecting => ("●", "connecting", theme.warning),
            TurnState::Thinking => ("●", "thinking", theme.accent),
            TurnState::Streaming => ("●", "working", theme.accent),
            TurnState::AwaitingAnswer => ("●", "waiting", theme.warning),
        };
        let state = format!("{label}  {} · ⌃T", self.theme_name);
        let width = 2 + state.chars().count() as u16 + 1;
        let x = area.x + area.width.saturating_sub(width);
        buf.set_stringn(
            x,
            area.y,
            dot,
            1,
            Style::default()
                .fg(color)
                .bg(theme.secondary)
                .add_modifier(Modifier::BOLD),
        );
        buf.set_stringn(
            x + 2,
            area.y,
            &state,
            state.chars().count(),
            Style::default().fg(theme.muted).bg(theme.secondary),
        );
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}
