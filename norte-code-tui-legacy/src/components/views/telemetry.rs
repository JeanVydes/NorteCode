//! Requests, grouped by the session that made them.
//!
//! The left half holds two lists, sessions above requests. `Enter` focuses one
//! of them, which is what lets the arrows walk its rows and turn its pages;
//! `Esc` lets go again. The right half answers for whatever is focused.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

use super::telemetry_detail;
use super::telemetry_pages::Pages;
use crate::{
    components::{
        Component, ComponentAction, ComponentId, ComponentLifecycle, ViewAction, palette, panel,
    },
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

/// How many rows one page shows.
pub(super) const PAGE_SIZE: usize = 10;

/// Which list the arrows act on, once the reader focused one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Focus {
    /// No list owns the arrows: they move the focus between the lists.
    #[default]
    Lists,
    Sessions,
    Requests,
}

#[derive(Default)]
pub(in crate::components) struct TelemetryView {
    lifecycle: ComponentLifecycle,
    focus: Focus,
    /// The session the reader picked.
    session: Pages,
    /// The request the reader picked, inside the selected session.
    request: Pages,
    pending_action: Option<ComponentAction>,
    /// Where the two lists were drawn, so a click maps to a row.
    sessions_area: Rect,
    requests_area: Rect,
}

impl Component for TelemetryView {
    fn id(&self) -> ComponentId {
        ComponentId::Telemetry
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn captures_global_keys(&self) -> bool {
        // While a list is focused the arrows belong to it, not to the tabs.
        self.focus != Focus::Lists
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, model: &UiModel) {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let sessions = session_list(model);
        let selected = sessions.get(self.session.selected()).cloned();
        let requests = selected
            .as_deref()
            .map_or_else(Vec::new, |session| requests_of(model, session));

        if let Some(index) = row_at(self.sessions_area, mouse, self.session.page()) {
            if index < sessions.len() {
                self.focus = Focus::Sessions;
                self.session.set_selected(index);
                self.request.reset();
                self.request.clamp(requests.len());
            }
            return;
        }
        if let Some(index) = row_at(self.requests_area, mouse, self.request.page())
            && index < requests.len()
        {
            self.focus = Focus::Requests;
            self.request.set_selected(index);
        }
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        let sessions = session_list(model);
        let selected = sessions.get(self.session.selected()).cloned();
        let requests = selected
            .as_deref()
            .map_or(0, |session| requests_of(model, session).len());
        match key.code {
            keys::CANCEL if self.focus != Focus::Lists => self.focus = Focus::Lists,
            keys::SUBMIT => {
                self.focus = match self.focus {
                    Focus::Lists | Focus::Requests => Focus::Sessions,
                    Focus::Sessions => {
                        self.request.reset();
                        Focus::Requests
                    }
                };
            }
            KeyCode::Tab if self.focus != Focus::Lists => {
                self.focus = if self.focus == Focus::Sessions {
                    Focus::Requests
                } else {
                    Focus::Sessions
                };
            }
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => match self.focus {
                Focus::Lists => self.focus = Focus::Sessions,
                Focus::Sessions => self.session.down(sessions.len()),
                Focus::Requests => self.request.down(requests),
            },
            keys::NAV_UP | keys::NAV_UP_ARROW => match self.focus {
                Focus::Lists => {}
                Focus::Sessions => self.session.up(),
                Focus::Requests => self.request.up(),
            },
            keys::NAV_RIGHT | keys::NAV_LEFT | KeyCode::Char('h' | 'l') => {
                let next = matches!(key.code, keys::NAV_RIGHT | KeyCode::Char('l'));
                match self.focus {
                    Focus::Lists => self.focus = Focus::Sessions,
                    Focus::Sessions if next => self.session.next_page(sessions.len()),
                    Focus::Sessions => self.session.previous_page(),
                    Focus::Requests if next => self.request.next_page(requests),
                    Focus::Requests => self.request.previous_page(),
                }
            }
            KeyCode::Char('r') => {
                self.pending_action = Some(ComponentAction::View(ViewAction::RefreshTelemetry));
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let sessions = session_list(model);
        self.session.clamp(sessions.len());
        let selected = sessions.get(self.session.selected()).cloned();
        let requests = selected
            .as_deref()
            .map_or_else(Vec::new, |session| requests_of(model, session));
        self.request.clamp(requests.len());

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        let lists = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Min(4)])
            .split(columns[0]);

        let session_rows = self
            .session
            .page_items(&sessions)
            .into_iter()
            .map(|session| TextBuilder::telemetry_session_row(model, &session))
            .collect::<Vec<_>>();
        self.sessions_area = lists[0];
        panel::render_list(
            frame,
            lists[0],
            session_rows.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Sessions,
                phase: self.lifecycle.phase(),
                selected: Some(self.session.row()),
                focused: self.focus == Focus::Sessions,
                empty_message: TextBuilder::empty_message(ComponentId::Sessions),
            },
        );
        Self::render_counter(
            frame,
            lists[0],
            model,
            &self.session,
            sessions.len(),
            "sessions",
            self.focus == Focus::Sessions,
        );

        let request_rows = self
            .request
            .page_items(&requests)
            .into_iter()
            .map(|record| TextBuilder::telemetry_row(&record))
            .collect::<Vec<_>>();
        self.requests_area = lists[1];
        panel::render_list(
            frame,
            lists[1],
            request_rows.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Telemetry,
                phase: self.lifecycle.phase(),
                selected: Some(self.request.row()),
                focused: self.focus == Focus::Requests,
                empty_message: "No requests in this session",
            },
        );
        Self::render_counter(
            frame,
            lists[1],
            model,
            &self.request,
            requests.len(),
            "requests",
            self.focus == Focus::Requests,
        );

        telemetry_detail::render(
            frame,
            columns[1],
            model,
            selected.as_deref(),
            &requests,
            &self.request,
        );
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}

impl TelemetryView {
    /// The page the session list is on, for a test to read.
    #[cfg(test)]
    pub(super) fn page_for_test(&self) -> usize {
        self.session.page()
    }

    /// The count and the page of one list, at the bottom of its panel.
    #[allow(clippy::too_many_arguments)]
    fn render_counter(
        frame: &mut Frame<'_>,
        area: Rect,
        model: &UiModel,
        pages: &Pages,
        total: usize,
        noun: &str,
        focused: bool,
    ) {
        if area.height < 2 {
            return;
        }
        let text = format!(
            "{} {noun} · page {}/{}",
            total,
            pages.page() + 1,
            pages.total()
        );
        let style = if focused {
            palette::focus_style(model.preferences.theme)
        } else {
            palette::muted_style(model.preferences.theme)
        };
        frame.render_widget(
            ratatui::widgets::Paragraph::new(text).style(style),
            Rect {
                y: area.bottom().saturating_sub(2),
                height: 1,
                ..area
            },
        );
    }
}

/// The index of the row a click landed on, given the page the list shows.
///
/// The title and the border sit above the first row, and the list starts one
/// row below the panel top.
fn row_at(area: Rect, mouse: MouseEvent, page: usize) -> Option<usize> {
    let position = ratatui::layout::Position {
        x: mouse.column,
        y: mouse.row,
    };
    if !area.contains(position) {
        return None;
    }
    let row = mouse.row.saturating_sub(area.y).saturating_sub(1);
    let index = page.saturating_mul(PAGE_SIZE) + usize::from(row);
    Some(index)
}

/// The sessions the report covers, in the order they first appear.
fn session_list(model: &UiModel) -> Vec<String> {
    let mut sessions: Vec<String> = Vec::new();
    if let Some(report) = &model.telemetry {
        for record in &report.records {
            if !sessions.iter().any(|session| session == &record.session) {
                sessions.push(record.session.clone());
            }
        }
    }
    sessions
}

/// The requests of one session, oldest first.
fn requests_of(model: &UiModel, session: &str) -> Vec<norte_code_wire::TelemetryRecord> {
    model
        .telemetry
        .as_ref()
        .map(|report| {
            report
                .records
                .iter()
                .filter(|record| record.session == session)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod tests;
