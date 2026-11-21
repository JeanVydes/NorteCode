//! The slim header chrome: wordmark, tabs, the turn state and the theme.

use std::any::Any;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::components::spinner::Spinner;
use terminalui::{Context, InputEvent, Outcome, Theme, View};

use crate::ui_model::{Tab, TurnState};

/// Cells reserved between the tabs and the turn state for job activity.
const ACTIVITY_WIDTH: u16 = 6;

fn job_count(count: usize) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

/// One-row header rendered above everything.
#[derive(Serialize)]
pub(crate) struct Header {
    /// The turn the header reports.
    pub(crate) turn: TurnState,
    /// The theme the header reports.
    pub(crate) theme_name: String,
    /// Which tab is active; rendered between the wordmark and the state.
    pub(crate) tab: Tab,
    /// Number of jobs still doing work, shown as a spinner.
    pub(crate) jobs: usize,
    #[serde(skip)]
    spinner: Spinner,
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
            jobs: 0,
            spinner: Spinner::new(),
            tab_rects: Vec::new(),
            requested_tab: None,
        }
    }

    /// Set the number of jobs still doing work.
    pub(crate) fn set_jobs(&mut self, jobs: usize) {
        self.jobs = jobs;
    }

    /// Take the tab the user clicked, if any; the host applies the switch.
    pub(crate) fn take_requested_tab(&mut self) -> Option<Tab> {
        self.requested_tab.take()
    }

    fn turn_label(&self) -> &'static str {
        match self.turn {
            TurnState::Idle => "ready",
            TurnState::Connecting => "connecting",
            TurnState::Thinking => "thinking",
            TurnState::Streaming => "working",
            TurnState::AwaitingAnswer => "waiting",
        }
    }

    fn state_width(&self) -> u16 {
        let text = format!("{}  {} · ⌃T", self.turn_label(), self.theme_name);
        3 + u16::try_from(text.chars().count()).unwrap_or(u16::MAX)
    }

    fn paint_tabs(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, status_x: u16) {
        self.tab_rects.clear();
        let mut x = area.x + 3;
        let full_width: u16 = Tab::ALL
            .iter()
            .map(|tab| 4 + u16::try_from(tab.label().chars().count()).unwrap_or(0))
            .sum();
        let short = x + full_width > status_x.saturating_sub(ACTIVITY_WIDTH);
        for (index, tab) in Tab::ALL.iter().enumerate() {
            let digit = (index + 1).to_string();
            let label = if short {
                tab.short_label()
            } else {
                tab.label()
            };
            let text = format!(" {digit} {label} ");
            let style = if *tab == self.tab {
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
    }

    fn paint_activity(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
        status_x: u16,
    ) {
        if self.jobs == 0 {
            return;
        }
        let jobs_x = status_x.saturating_sub(ACTIVITY_WIDTH);
        self.spinner.render(
            Rect {
                x: jobs_x,
                y: area.y,
                width: 1,
                height: 1,
            },
            buf,
            cx,
            theme,
        );
        let count = job_count(self.jobs);
        buf.set_stringn(
            jobs_x + 2,
            area.y,
            &count,
            count.chars().count(),
            Style::default()
                .fg(theme.accent)
                .bg(theme.secondary)
                .add_modifier(Modifier::BOLD),
        );
    }

    fn paint_state(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (dot, color) = match self.turn {
            TurnState::Idle => ("●", theme.success),
            TurnState::Connecting | TurnState::AwaitingAnswer => ("●", theme.warning),
            TurnState::Thinking | TurnState::Streaming => ("●", theme.accent),
        };
        let state = format!("{}  {} · ⌃T", self.turn_label(), self.theme_name);
        let width = self.state_width();
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

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
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

        let status_x = area.x + area.width.saturating_sub(self.state_width());
        self.paint_tabs(area, buf, theme, status_x);
        self.paint_activity(area, buf, cx, theme, status_x);
        self.paint_state(area, buf, theme);
    }

    fn tick(&mut self, delta: Duration, cx: &mut Context) {
        self.spinner.tick(delta, cx);
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
