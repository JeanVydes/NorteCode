use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Frame, layout::Rect};

use crate::{
    components::{Component, ComponentAction, ComponentId, ComponentLifecycle, panel},
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

#[derive(Default)]
enum SessionPrompt {
    #[default]
    Closed,
    Rename {
        session: norte_code_wire::SessionKey,
        title: String,
    },
    Delete {
        session: norte_code_wire::SessionKey,
        title: String,
    },
}

/// What one row of the list is: a session, a child or an action of the
/// session it was opened under.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Session(usize),
    Child(usize),
    /// The `usize` is the flat index of the session/child the action belongs
    /// to, the `usize` the index of the action.
    Action {
        item: usize,
        action: usize,
    },
}

/// The actions a session offers; the child offers only the first.
const SESSION_ACTIONS: [&str; 3] = ["open", "rename", "delete"];
const CHILD_ACTIONS: [&str; 1] = ["open"];

#[derive(Default)]
pub(in crate::components) struct SessionsView {
    lifecycle: ComponentLifecycle,
    /// The selected row in the flattened list (sessions + children + actions).
    selected: usize,
    pending_action: Option<ComponentAction>,
    prompt: SessionPrompt,
    /// The index into the sessions + children sequence whose action rows are
    /// shown, if any.
    expanded: Option<usize>,
    /// Where the view last drew, so a click can map back to its row.
    last_render_area: Rect,
}

impl SessionsView {
    /// The rows of the list, in display order: the sessions and the children,
    /// with the action rows of the expanded item interleaved.
    fn visible_rows(&self, model: &UiModel) -> Vec<Row> {
        let mut rows =
            Vec::with_capacity(model.sessions.len() + model.children.len() + SESSION_ACTIONS.len());
        for index in 0..model.sessions.len() {
            rows.push(Row::Session(index));
            if self.expanded == Some(index) {
                rows.extend(
                    SESSION_ACTIONS
                        .iter()
                        .enumerate()
                        .map(|(action, _)| Row::Action {
                            item: index,
                            action,
                        }),
                );
            }
        }
        let offset = model.sessions.len();
        for index in 0..model.children.len() {
            rows.push(Row::Child(index));
            if self.expanded == Some(offset + index) {
                rows.extend(
                    CHILD_ACTIONS
                        .iter()
                        .enumerate()
                        .map(|(action, _)| Row::Action {
                            item: offset + index,
                            action,
                        }),
                );
            }
        }
        rows
    }

    /// Runs what the row asks for: toggles the expansion of a session, or
    /// performs an action of the one being expanded.
    fn select_row(&mut self, model: &UiModel) {
        let rows = self.visible_rows(model);
        let Some(selected) = rows.get(self.selected).copied() else {
            return;
        };
        match selected {
            Row::Session(item) | Row::Child(item) => self.toggle_expand(model, item),
            Row::Action { item, action } => self.perform_action(model, item, action),
        }
    }

    fn perform_action(&mut self, model: &UiModel, item: usize, action: usize) {
        if let Some(session) = model.sessions.get(item) {
            let key = norte_code_wire::SessionKey::new(session.session_id.clone());
            match action {
                0 => {
                    self.pending_action = Some(ComponentAction::OpenSession(key));
                }
                1 => {
                    self.prompt = SessionPrompt::Rename {
                        session: key,
                        title: session.title.clone().unwrap_or_default(),
                    };
                }
                2 => {
                    self.prompt = SessionPrompt::Delete {
                        session: key,
                        title: session
                            .title
                            .clone()
                            .unwrap_or_else(|| "Untitled session".to_owned()),
                    };
                }
                _ => {}
            }
            return;
        }
        let index = item.saturating_sub(model.sessions.len());
        if let Some(child) = model.children.get(index)
            && action == 0
        {
            self.pending_action = Some(ComponentAction::OpenChildSession(
                norte_code_wire::SessionKey::new(child.session_id.clone()),
            ));
        }
    }

    fn toggle_expand(&mut self, model: &UiModel, item: usize) {
        if self.expanded == Some(item) {
            self.expanded = None;
        } else {
            self.expanded = Some(item);
        }
        self.clamp_selection(model);
    }

    /// The number of the action this row selects; clamped so a vanished item
    /// never panics.
    fn clamp_selection(&mut self, model: &UiModel) {
        self.selected = self
            .selected
            .min(self.visible_rows(model).len().saturating_sub(1));
    }

    /// The full row the mouse points at; `None` for the panel's title row or
    /// beyond the last item.
    fn row_at(&self, model: &UiModel, row: u16) -> Option<usize> {
        let base = self.last_render_area.y.saturating_add(1);
        let mut offset = 0_u16;
        for (index, row_kind) in self.visible_rows(model).iter().enumerate() {
            let height: u16 = match row_kind {
                Row::Session(_) | Row::Child(_) => 2,
                Row::Action { .. } => 1,
            };
            if row >= base.saturating_add(offset)
                && row < base.saturating_add(offset).saturating_add(height)
            {
                return Some(index);
            }
            offset = offset.saturating_add(height);
        }
        None
    }

    fn selected_session<'a>(
        &self,
        model: &'a UiModel,
    ) -> Option<&'a norte_code_wire::SessionSummary> {
        match self.visible_rows(model).get(self.selected) {
            Some(Row::Session(index)) => model.sessions.get(*index),
            Some(Row::Action { item, .. }) => model.sessions.get(*item),
            _ => None,
        }
    }
}

impl Component for SessionsView {
    fn id(&self) -> ComponentId {
        ComponentId::Sessions
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn captures_global_keys(&self) -> bool {
        !matches!(&self.prompt, SessionPrompt::Closed)
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        if !matches!(&self.prompt, SessionPrompt::Closed) {
            let prompt = std::mem::take(&mut self.prompt);
            match (prompt, key.code) {
                (_, keys::CANCEL) => {}
                (SessionPrompt::Rename { session, title }, keys::SUBMIT)
                    if !title.trim().is_empty() =>
                {
                    self.pending_action = Some(ComponentAction::RenameSession {
                        session: session.clone(),
                        title: title.trim().to_owned(),
                    });
                }
                (SessionPrompt::Rename { session, mut title }, keys::ERASE) => {
                    title.pop();
                    self.prompt = SessionPrompt::Rename { session, title };
                }
                (SessionPrompt::Rename { session, mut title }, KeyCode::Char(character)) => {
                    title.push(character);
                    self.prompt = SessionPrompt::Rename { session, title };
                }
                (SessionPrompt::Delete { session, .. }, keys::SUBMIT) => {
                    self.pending_action = Some(ComponentAction::DeleteSession(session.clone()));
                }
                (prompt, _) => self.prompt = prompt,
            }
            return;
        }
        match key.code {
            KeyCode::Char(keys::NEW_SESSION_CHAR) => {
                self.pending_action = Some(ComponentAction::CreateSession);
            }
            keys::SUBMIT => self.select_row(model),
            keys::CANCEL | keys::FOCUS_NEXT => {
                if self.expanded.is_some() {
                    self.expanded = None;
                    self.clamp_selection(model);
                }
            }
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                let len = self.visible_rows(model).len();
                panel::move_selection(&mut self.selected, len, true);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                let len = self.visible_rows(model).len();
                panel::move_selection(&mut self.selected, len, false);
            }
            KeyCode::Char('r') => {
                if let Some(session) = self.selected_session(model) {
                    self.prompt = SessionPrompt::Rename {
                        session: norte_code_wire::SessionKey::new(session.session_id.clone()),
                        title: session.title.clone().unwrap_or_default(),
                    };
                }
            }
            KeyCode::Char('d') => {
                if let Some(session) = self.selected_session(model) {
                    self.prompt = SessionPrompt::Delete {
                        session: norte_code_wire::SessionKey::new(session.session_id.clone()),
                        title: session
                            .title
                            .clone()
                            .unwrap_or_else(|| "Untitled session".to_owned()),
                    };
                }
            }
            KeyCode::Char('f') => {
                if let Some(session) = self.selected_session(model) {
                    self.pending_action = Some(ComponentAction::ForkSession(
                        norte_code_wire::SessionKey::new(session.session_id.clone()),
                    ));
                }
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, model: &UiModel) {
        if !matches!(&self.prompt, SessionPrompt::Closed) {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                let len = self.visible_rows(model).len();
                panel::move_selection(&mut self.selected, len, true);
            }
            MouseEventKind::ScrollUp => {
                let len = self.visible_rows(model).len();
                panel::move_selection(&mut self.selected, len, false);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(index) = self.row_at(model, mouse.row) else {
                    return;
                };
                self.selected = index;
                self.select_row(model);
            }
            _ => {}
        }
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        self.last_render_area = area;
        match &self.prompt {
            SessionPrompt::Rename { title, .. } => {
                panel::render_text(
                    frame,
                    area,
                    "Rename session",
                    format!("{title}_\n\nType a title · Enter saves · Esc cancels"),
                    model.preferences.theme,
                );
                return;
            }
            SessionPrompt::Delete { title, .. } => {
                panel::render_text(
                    frame,
                    area,
                    "Delete session?",
                    format!("{title}\n\nEnter confirms deletion · Esc cancels"),
                    model.preferences.theme,
                );
                return;
            }
            SessionPrompt::Closed => {}
        }
        let rows = self.visible_rows(model);
        let entries = rows
            .iter()
            .map(|row| match *row {
                Row::Session(index) => {
                    let session = &model.sessions[index];
                    TextBuilder::numbered_session_row(
                        index + 1,
                        session,
                        self.expanded == Some(index),
                    )
                }
                Row::Child(index) => {
                    let child = &model.children[index];
                    TextBuilder::numbered_child_session_row(
                        model.sessions.len() + index + 1,
                        child,
                        self.expanded == Some(model.sessions.len() + index),
                    )
                }
                Row::Action { item, action } => {
                    let labels = if model.sessions.get(item).is_some() {
                        &SESSION_ACTIONS[..]
                    } else {
                        &CHILD_ACTIONS[..]
                    };
                    TextBuilder::session_action_row(
                        labels.get(action).copied().unwrap_or(""),
                        false,
                        model.preferences.theme,
                    )
                }
            })
            .collect::<Vec<_>>();
        panel::render_list(
            frame,
            area,
            entries,
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Sessions,
                phase: self.lifecycle.phase(),
                selected: Some(self.selected),
                focused: true,
                empty_message: TextBuilder::empty_message(ComponentId::Sessions),
            },
        );
    }
}

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
