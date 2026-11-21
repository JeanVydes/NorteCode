//! The slim header chrome: wordmark, tabs, the turn state and the theme.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

use crate::ui_model::{Tab, TurnState};

/// Every tab, in display order. Rendered in the strip.
const TABS: [Tab; 5] = [
    Tab::Chat,
    Tab::Sessions,
    Tab::Models,
    Tab::Telemetry,
    Tab::Settings,
];

/// One-row header rendered above everything.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Header {
    /// The turn the header reports.
    pub(crate) turn: TurnState,
    /// The theme the header reports.
    pub(crate) theme_name: String,
    /// Which tab is active; rendered between the wordmark and the state.
    pub(crate) tab: Tab,
    #[serde(skip)]
    tab_rects: Vec<(Rect, Tab)>,
    #[serde(skip)]
    requested_tab: Option<Tab>,
}

impl Header {
    /// Creates a header reading from the given state.
    pub(crate) fn new(turn: TurnState, theme_name: String) -> Self {
        Self {
            turn,
            theme_name,
            tab: Tab::Chat,
            tab_rects: Vec::new(),
            requested_tab: None,
        }
    }

    /// Take the tab the user clicked, if any; the host applies the switch.
    pub(crate) fn take_requested_tab(&mut self) -> Option<Tab> {
        self.requested_tab.take()
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
        let mark = "◈";
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

        // Tabs start after the wordmark. Short labels keep the strip clear
        // of the state chip on narrow screens.
        self.tab_rects.clear();
        let mut x = area.x + 3;
        let state_width = 2
            + u16::try_from(
                format!(
                    "{}  {} · ⌃T",
                    match self.turn {
                        TurnState::Idle => "ready",
                        TurnState::Connecting => "connecting",
                        TurnState::Thinking => "thinking",
                        TurnState::Streaming => "working",
                        TurnState::AwaitingAnswer => "waiting",
                    },
                    self.theme_name
                )
                .chars()
                .count(),
            )
            .unwrap_or(u16::MAX)
            + 1;
        let status_x = area.x + area.width.saturating_sub(state_width);
        let full_width: u16 = TABS
            .iter()
            .map(|tab| 4 + u16::try_from(tab.label().chars().count()).unwrap_or(0))
            .sum();
        let short = x + full_width > status_x;
        for (index, tab) in TABS.iter().enumerate() {
            let digit = (index + 1).to_string();
            let active = *tab == self.tab;
            let label = if short {
                tab.short_label()
            } else {
                tab.label()
            };
            let text = format!(" {digit} {label} ");
            let style = if active {
                Style::default()
                    .fg(theme.secondary)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted).bg(theme.secondary)
            };
            let width = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
            buf.set_stringn(x, area.y, &text, text.chars().count(), style);
            self.tab_rects.push((Rect::new(x, area.y, width, 1), *tab));
            x += width;
        }

        let (dot, label, color) = match self.turn {
            TurnState::Idle => ("●", "ready", theme.success),
            TurnState::Connecting => ("●", "connecting", theme.warning),
            TurnState::Thinking => ("●", "thinking", theme.accent),
            TurnState::Streaming => ("●", "working", theme.accent),
            TurnState::AwaitingAnswer => ("●", "waiting", theme.warning),
        };
        let state = format!("{label}  {} · ⌃T", self.theme_name);
        let width = 2 + u16::try_from(state.chars().count()).unwrap_or(u16::MAX) + 1;
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

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        if let InputEvent::Mouse(mouse) = event {
            use crossterm::event::{MouseButton, MouseEventKind};
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                for (rect, tab) in &self.tab_rects {
                    if mouse.row == rect.y
                        && mouse.column >= rect.x
                        && mouse.column < rect.x + rect.width
                    {
                        self.requested_tab = Some(*tab);
                        return Outcome::Consumed;
                    }
                }
            }
        }
        Outcome::Ignored
    }
}
