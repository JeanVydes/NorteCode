use crossterm::event::{KeyCode, KeyEvent};
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

#[derive(Default)]
pub(in crate::components) struct SessionsView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    pending_action: Option<ComponentAction>,
    prompt: SessionPrompt,
}

impl SessionsView {
    fn open_index(&mut self, index: usize, model: &UiModel) {
        if let Some(session) = model.sessions.get(index) {
            self.pending_action = Some(ComponentAction::OpenSession(
                norte_code_wire::SessionKey::new(session.session_id.clone()),
            ));
        } else if let Some(child) = model
            .children
            .get(index.saturating_sub(model.sessions.len()))
        {
            self.pending_action = Some(ComponentAction::OpenChildSession(
                norte_code_wire::SessionKey::new(child.session_id.clone()),
            ));
        }
    }

    fn selected_session<'a>(
        &self,
        model: &'a UiModel,
    ) -> Option<&'a norte_code_wire::SessionSummary> {
        model.sessions.get(self.selected)
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
            KeyCode::Char(number @ '1'..='9') => {
                if let Some(index) = number.to_digit(10).and_then(|n| n.checked_sub(1)) {
                    self.open_index(usize::try_from(index).unwrap_or_default(), model);
                }
            }
            keys::SUBMIT => self.open_index(self.selected, model),
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                panel::move_selection(
                    &mut self.selected,
                    model.sessions.len() + model.children.len(),
                    true,
                );
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                panel::move_selection(
                    &mut self.selected,
                    model.sessions.len() + model.children.len(),
                    false,
                );
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

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
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
        let entries = model
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| TextBuilder::numbered_session_row(index + 1, session))
            .chain(model.children.iter().enumerate().map(|(index, child)| {
                TextBuilder::numbered_child_session_row(model.sessions.len() + index + 1, child)
            }))
            .collect();
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
