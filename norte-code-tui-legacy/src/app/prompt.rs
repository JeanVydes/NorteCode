//! The one text prompt the interface asks for.
//!
//! It lives at the application level, not inside a view: the menu can be
//! opened from any tab, and a prompt that belonged to the session view would
//! be invisible from anywhere else while still swallowing the keys.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    text::{Line, Span},
};

use super::App;
use crate::components::{ComponentAction, Modal, palette, prompt::Prompt};
use crate::keys;
use crate::ui_model::UiModel;

/// The prompt that is open, with what the reader typed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PromptState {
    kind: Prompt,
    value: String,
}

impl PromptState {
    /// The prompt a menu command opens.
    pub(crate) fn new(kind: Prompt) -> Self {
        Self {
            kind,
            value: String::new(),
        }
    }

    /// The title of the box.
    const fn title(&self) -> &'static str {
        match self.kind {
            Prompt::Import => "import session",
            Prompt::Budget => "session budget",
            Prompt::Rename => "rename session",
            Prompt::Delete => "delete session",
            Prompt::Rewind => "rewind to a turn",
        }
    }

    /// The line that explains what the value is.
    fn caption(&self, model: &UiModel) -> String {
        match self.kind {
            Prompt::Import => "Path of the bundle to import".to_owned(),
            Prompt::Budget => format!(
                "Limit in USD (action: {})",
                crate::components::command_menu::budget_action_label(model.budget_action)
            ),
            Prompt::Rename => "New title for the session".to_owned(),
            Prompt::Delete => "Delete the session that is open? This cannot be undone.".to_owned(),
            Prompt::Rewind => "Turn id, for example turn_3".to_owned(),
        }
    }

    /// Whether the prompt asks for a value at all.
    const fn takes_input(&self) -> bool {
        !matches!(self.kind, Prompt::Delete)
    }
}

impl App {
    /// Opens a prompt, replacing whatever was open.
    pub(super) fn open_prompt(&mut self, kind: Prompt) {
        self.prompt = Some(PromptState::new(kind));
        self.needs_redraw = true;
    }

    /// Whether a prompt owns the keys.
    pub(super) fn prompt_open(&self) -> bool {
        self.prompt.is_some()
    }

    /// Feeds one key to the open prompt.
    pub(super) fn prompt_key(&mut self, key: KeyEvent) -> Result<(), super::AppError> {
        let Some(mut prompt) = self.prompt.take() else {
            return Ok(());
        };
        if key.code == keys::CANCEL {
            return Ok(());
        }
        if key.code == keys::SUBMIT {
            return self.submit_prompt(prompt);
        }
        match key.code {
            keys::ERASE => {
                prompt.value.pop();
                self.prompt = Some(prompt);
            }
            KeyCode::Char(character) if Self::accepts(prompt.kind, character) => {
                prompt.value.push(character);
                self.prompt = Some(prompt);
            }
            _ => self.prompt = Some(prompt),
        }
        self.needs_redraw = true;
        Ok(())
    }

    fn accepts(kind: Prompt, character: char) -> bool {
        match kind {
            Prompt::Budget => character.is_ascii_digit() || character == '.',
            Prompt::Delete => false,
            Prompt::Import | Prompt::Rename | Prompt::Rewind => true,
        }
    }

    fn submit_prompt(&mut self, prompt: PromptState) -> Result<(), super::AppError> {
        let value = prompt.value.trim().to_owned();
        let action = match prompt.kind {
            Prompt::Import if !value.is_empty() => Some(ComponentAction::ImportSession(value)),
            Prompt::Rename if !value.is_empty() => {
                self.model
                    .active_session
                    .clone()
                    .map(|session| ComponentAction::RenameSession {
                        session,
                        title: value,
                    })
            }
            Prompt::Rewind if !value.is_empty() => Some(ComponentAction::Rewind { turn_id: value }),
            Prompt::Budget => value
                .parse::<f64>()
                .ok()
                .filter(|limit| limit.is_finite() && *limit > 0.0)
                .map(|limit| ComponentAction::SetBudget {
                    limit,
                    action: self.model.budget_action,
                }),
            Prompt::Delete => self
                .model
                .active_session
                .clone()
                .map(ComponentAction::DeleteSession),
            _ => None,
        };
        match action {
            Some(action) => self.handle_component_action(action)?,
            None => self.prompt = Some(prompt),
        }
        Ok(())
    }

    /// Draws the open prompt over a scrim.
    pub(super) fn render_prompt(&self, frame: &mut Frame<'_>, model: &UiModel) {
        let Some(prompt) = &self.prompt else {
            return;
        };
        let theme = model.preferences.theme;
        let mut body = vec![Line::from(Span::styled(
            prompt.caption(model),
            palette::muted_style(theme),
        ))];
        if prompt.takes_input() {
            body.push(Line::default());
            body.push(Line::from(Span::styled(
                format!("{}_", prompt.value),
                palette::text_style(theme),
            )));
        }
        let controls = if prompt.takes_input() {
            "Enter confirms, Esc cancels"
        } else {
            "Enter deletes, Esc cancels"
        };
        Modal {
            title: prompt.title(),
            controls,
            body,
            theme,
            selected: None,
        }
        .render(frame);
    }
}
