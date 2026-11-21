//! The slim header chrome: wordmark, tabs, the turn state and the theme.

use std::any::Any;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::components::spinner::Spinner;
use terminalui::components::tabs::{Tab as TabView, Tabs, TabsStyle};
use terminalui::{Context, InputEvent, Outcome, Theme, View, node};

use crate::ui::copy::{CONNECTING, READY_TEXT, THEME_KEY, THINKING, WAITING, WORKING};
use crate::ui_model::{Tab, TurnState};

/// Cells reserved between the tabs and the turn state for job activity.
const ACTIVITY_WIDTH: u16 = 6;

fn build_tabs(short: bool) -> Tabs {
    let children = Tab::ALL
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let label = if short {
                tab.short_label()
            } else {
                tab.label()
            };
            node(
                format!("tab-{index}"),
                TabView::with_title(format!("{} {label}", index + 1)),
            )
        })
        .collect();
    Tabs::new(children).with_style(TabsStyle {
        gap: Some(0),
        ..TabsStyle::default()
    })
}

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
    tabs: Tabs,
    #[serde(skip)]
    short_tabs: bool,
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
            tabs: build_tabs(false),
            short_tabs: false,
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
            TurnState::Idle => READY_TEXT,
            TurnState::Connecting => CONNECTING,
            TurnState::Thinking => THINKING,
            TurnState::Streaming => WORKING,
            TurnState::AwaitingAnswer => WAITING,
        }
    }

    fn state_width(&self) -> u16 {
        let text = format!("{}  {} · {THEME_KEY}", self.turn_label(), self.theme_name);
        3 + u16::try_from(text.chars().count()).unwrap_or(u16::MAX)
    }

    fn paint_tabs(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
        status_x: u16,
    ) {
        let available = status_x.saturating_sub(ACTIVITY_WIDTH);
        let full_width: u16 = Tab::ALL
            .iter()
            .map(|tab| 4 + u16::try_from(tab.label().chars().count()).unwrap_or(0))
            .sum();
        let short = area.x + 3 + full_width > available;
        if short != self.short_tabs {
            self.short_tabs = short;
            self.tabs = build_tabs(short);
        }
        self.tabs.set_selected(self.tab.index());
        let x = area.x + 3;
        let width = available.saturating_sub(x);
        let mut tab_cx = *cx;
        self.tabs
            .render(Rect::new(x, area.y, width, 1), buf, &mut tab_cx, theme);
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
        let state = format!("{}  {} · {THEME_KEY}", self.turn_label(), self.theme_name);
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
        self.paint_tabs(area, buf, cx, theme, status_x);
        self.paint_activity(area, buf, cx, theme, status_x);
        self.paint_state(area, buf, theme);
    }

    fn tick(&mut self, delta: Duration, cx: &mut Context) {
        self.spinner.tick(delta, cx);
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        let before = self.tabs.selected();
        let mut tab_cx = *cx;
        let outcome = self.tabs.handle_event(event, &mut tab_cx);
        let after = self.tabs.selected();
        if before != after {
            self.requested_tab = Tab::ALL.get(after).copied();
        }
        outcome
    }
}
