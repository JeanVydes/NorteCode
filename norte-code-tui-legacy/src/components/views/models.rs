use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent};
use norte_code_wire::ModelInfo;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Paragraph,
};

use crate::{
    components::{Component, ComponentAction, ComponentId, ComponentLifecycle, palette, panel},
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

#[derive(Default)]
pub(in crate::components) struct ModelsView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    filter: String,
    filtering: bool,
    expanded: HashSet<String>,
    pending_action: Option<ComponentAction>,
    key_prompt: Option<KeyPrompt>,
    erase_prompt: Option<ErasePrompt>,
}

enum KeyPrompt {
    Edit {
        provider: String,
        value: String,
        error: Option<String>,
        intent: KeyPromptIntent,
    },
}

enum KeyPromptIntent {
    /// After saving the key, switch to this model.
    SwitchModel { model: String },
    /// After saving the key, expand the provider's list of models.
    Expand,
}

enum ErasePrompt {
    FromEnvironment { provider: String, variable: String },
    Removed { provider: String, path: String },
    Missing { provider: String },
    Error { provider: String, message: String },
}

#[derive(Clone, Copy)]
enum ModelRow<'a> {
    Provider {
        name: &'a str,
        model_count: usize,
        expanded: bool,
    },
    Model(&'a ModelInfo),
}

impl ModelsView {
    /// The rows a filter leaves, as text, for a test to read.
    #[cfg(test)]
    pub(super) fn visible_rows_for(&self, models: &[ModelInfo]) -> Vec<String> {
        self.visible_rows(models)
            .into_iter()
            .map(|row| match row {
                ModelRow::Provider { name, .. } => name.to_owned(),
                ModelRow::Model(model) => model.model.clone(),
            })
            .collect()
    }

    fn visible_rows<'a>(&self, models: &'a [ModelInfo]) -> Vec<ModelRow<'a>> {
        let query = self.filter.to_lowercase();
        let mut providers = Vec::new();
        let mut seen = HashSet::new();
        for model in models {
            if seen.insert(model.provider.as_str()) {
                providers.push(model.provider.as_str());
            }
        }

        let mut rows = Vec::new();
        for provider in providers {
            let provider_matches =
                query.is_empty() || provider.to_lowercase().contains(query.as_str());
            let all = models
                .iter()
                .filter(|model| model.provider == provider)
                .collect::<Vec<_>>();
            // A provider the query names shows all of its models; a provider
            // the query does not name shows only the models that match it.
            let matching = if provider_matches {
                all.clone()
            } else {
                all.iter()
                    .copied()
                    .filter(|model| {
                        model.model.to_lowercase().contains(query.as_str())
                            || model.name.to_lowercase().contains(query.as_str())
                    })
                    .collect::<Vec<_>>()
            };
            if !provider_matches && matching.is_empty() {
                continue;
            }

            let expanded = !query.is_empty() || self.expanded.contains(provider);
            rows.push(ModelRow::Provider {
                name: provider,
                model_count: matching.len(),
                expanded,
            });
            if expanded {
                rows.extend(matching.into_iter().map(ModelRow::Model));
            }
        }
        rows
    }

    fn select_row(&mut self, model: &UiModel) {
        match self.visible_rows(&model.models).get(self.selected).copied() {
            Some(ModelRow::Provider { name, .. }) => {
                let provider = name;
                let known = crate::provider_keys::check(model, provider);
                if let crate::provider_keys::ProviderKeyStatus::Missing { .. } = known {
                    self.key_prompt = Some(KeyPrompt::Edit {
                        provider: provider.to_owned(),
                        value: String::new(),
                        error: None,
                        intent: KeyPromptIntent::Expand,
                    });
                    return;
                }
                if !self.expanded.remove(name) {
                    self.expanded.insert(name.to_owned());
                }
                self.clamp_selection(&model.models);
            }
            Some(ModelRow::Model(selected))
                if model.active_child.is_none() && model.active_session.is_some() =>
            {
                let missing = crate::provider_keys::check(model, &selected.provider);
                if let crate::provider_keys::ProviderKeyStatus::Missing { .. } = missing {
                    self.key_prompt = Some(KeyPrompt::Edit {
                        provider: selected.provider.clone(),
                        value: String::new(),
                        error: None,
                        intent: KeyPromptIntent::SwitchModel {
                            model: selected.model.clone(),
                        },
                    });
                    return;
                }
                self.pending_action = Some(ComponentAction::SetModel {
                    provider: selected.provider.clone(),
                    model: selected.model.clone(),
                });
            }
            Some(ModelRow::Model(_)) | None => {}
        }
    }

    fn handle_erase_prompt(&mut self, key: KeyEvent, _model: &UiModel, prompt: ErasePrompt) {
        match key.code {
            keys::CANCEL | keys::SUBMIT => self.erase_prompt = None,
            _ => self.erase_prompt = Some(prompt),
        }
    }

    fn erase_key(&mut self, model: &UiModel) {
        let selected = match self.visible_rows(&model.models).get(self.selected).copied() {
            Some(ModelRow::Provider { name, .. }) => name.to_owned(),
            Some(ModelRow::Model(selected)) => selected.provider.clone(),
            None => return,
        };
        let outcome = crate::provider_keys::erase(model, &selected);
        self.erase_prompt = Some(match outcome {
            crate::provider_keys::ProviderKeyEraseOutcome::FromEnvironment { variable } => {
                ErasePrompt::FromEnvironment {
                    provider: selected,
                    variable,
                }
            }
            crate::provider_keys::ProviderKeyEraseOutcome::Removed { path } => {
                ErasePrompt::Removed {
                    provider: selected,
                    path,
                }
            }
            crate::provider_keys::ProviderKeyEraseOutcome::Missing => {
                ErasePrompt::Missing { provider: selected }
            }
            crate::provider_keys::ProviderKeyEraseOutcome::Error { message } => {
                ErasePrompt::Error {
                    provider: selected,
                    message,
                }
            }
        });
    }

    fn handle_key_prompt(&mut self, key: KeyEvent, model: &UiModel, prompt: KeyPrompt) {
        let KeyPrompt::Edit {
            provider,
            mut value,
            mut error,
            intent,
        } = prompt;
        match key.code {
            keys::CANCEL => self.key_prompt = None,
            keys::ERASE => {
                value.pop();
                error = None;
                self.key_prompt = Some(KeyPrompt::Edit {
                    provider,
                    value,
                    error,
                    intent,
                });
            }
            keys::SUBMIT => {
                let trimmed = value.trim().to_owned();
                if trimmed.is_empty() {
                    self.key_prompt = Some(KeyPrompt::Edit {
                        provider,
                        value,
                        error: Some("the key is empty".to_owned()),
                        intent,
                    });
                    return;
                }
                let saved = save_key(&provider, &trimmed);
                match saved {
                    Ok(()) => {
                        self.key_prompt = None;
                        match intent {
                            KeyPromptIntent::SwitchModel { model } => {
                                self.pending_action =
                                    Some(ComponentAction::SetModel { provider, model });
                            }
                            KeyPromptIntent::Expand => {
                                if !self.expanded.remove(&provider) {
                                    self.expanded.insert(provider);
                                }
                                self.clamp_selection(&model.models);
                            }
                        }
                    }
                    Err(failure) => {
                        self.key_prompt = Some(KeyPrompt::Edit {
                            provider,
                            value,
                            error: Some(format!("could not save: {failure}")),
                            intent,
                        });
                    }
                }
            }
            KeyCode::Char(character) => {
                value.push(character);
                error = None;
                self.key_prompt = Some(KeyPrompt::Edit {
                    provider,
                    value,
                    error,
                    intent,
                });
            }
            _ => {
                self.key_prompt = Some(KeyPrompt::Edit {
                    provider,
                    value,
                    error,
                    intent,
                });
            }
        }
    }

    fn clamp_selection(&mut self, models: &[ModelInfo]) {
        self.selected = self
            .selected
            .min(self.visible_rows(models).len().saturating_sub(1));
    }
}

/// Whether the provider shows its ✓: the picker is honest about which
/// providers are usable without asking the person to find out the hard way.
fn provider_signed_in(model: &UiModel, provider: &str) -> bool {
    !crate::provider_keys::check(model, provider).blocks()
}

fn save_key(provider: &str, key: &str) -> Result<(), String> {
    let path = norte_code_protocol::keys::secrets_path()
        .ok_or("could not resolve the secrets.json path".to_owned())?;
    norte_code_protocol::keys::save_secret(&path, provider, key).map_err(|error| error.to_string())
}

fn current_model(model: &UiModel) -> Option<(&str, &str)> {
    model
        .session_model
        .as_ref()
        .map(|model| (model.provider.as_str(), model.model.as_str()))
        .or_else(|| {
            model.active_session.as_ref().and_then(|session| {
                model
                    .sessions
                    .iter()
                    .find(|summary| summary.session_id == session.as_str())
                    .and_then(|summary| {
                        Some((
                            summary.provider.as_ref()?.as_str(),
                            summary.model.as_ref()?.id.as_str(),
                        ))
                    })
            })
        })
}

fn model_marker(
    model: &ModelInfo,
    current: Option<(&str, &str)>,
    queued: Option<(&str, &str)>,
) -> Option<&'static str> {
    if current.is_some_and(|(provider, id)| provider == model.provider && id == model.model) {
        Some("current")
    } else if queued.is_some_and(|(provider, id)| provider == model.provider && id == model.model) {
        Some("queued")
    } else {
        None
    }
}

impl Component for ModelsView {
    fn id(&self) -> ComponentId {
        ComponentId::Models
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn captures_global_keys(&self) -> bool {
        self.erase_prompt.is_some() || self.key_prompt.is_some() || self.filtering
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        if let Some(prompt) = self.erase_prompt.take() {
            self.handle_erase_prompt(key, model, prompt);
            return;
        }
        if let Some(prompt) = self.key_prompt.take() {
            self.handle_key_prompt(key, model, prompt);
            return;
        }
        if self.filtering {
            match key.code {
                keys::CANCEL => {
                    self.filtering = false;
                    self.filter.clear();
                    self.selected = 0;
                }
                keys::SUBMIT => self.filtering = false,
                keys::ERASE => {
                    self.filter.pop();
                    self.selected = 0;
                }
                KeyCode::Char(character) => {
                    self.filter.push(character);
                    self.selected = 0;
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char(keys::SEARCH_CHAR) => {
                self.filtering = true;
                self.selected = 0;
            }
            keys::CANCEL if !self.filter.is_empty() => {
                self.filter.clear();
                self.selected = 0;
            }
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                let count = self.visible_rows(&model.models).len();
                panel::move_selection(&mut self.selected, count, true);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                let count = self.visible_rows(&model.models).len();
                panel::move_selection(&mut self.selected, count, false);
            }
            keys::SUBMIT => self.select_row(model),
            KeyCode::Delete | keys::ERASE => self.erase_key(model),
            _ => {}
        }
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }

    #[allow(clippy::too_many_lines)]
    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        if let Some(prompt) = &self.erase_prompt {
            match prompt {
                ErasePrompt::FromEnvironment { provider, variable } => {
                    panel::render_text(
                        frame,
                        area,
                        "Key comes from the environment",
                        format!(
                            "{provider}: the key is published by ${variable}.\n\nThe TUI does not unset environment variables. Unset it in your shell (or your shell profile) to remove it:\n\n    unset {variable}\n\nPress Esc to close."
                        ),
                        model.preferences.theme,
                    );
                    return;
                }
                ErasePrompt::Removed { provider, path } => {
                    panel::render_text(
                        frame,
                        area,
                        "API key deleted",
                        format!(
                            "The key for {provider} was removed from\n{path}\n\nPress Esc to close."
                        ),
                        model.preferences.theme,
                    );
                    return;
                }
                ErasePrompt::Missing { provider } => {
                    panel::render_text(
                        frame,
                        area,
                        "No stored key",
                        format!("No key is stored for {provider}.\n\nPress Esc to close."),
                        model.preferences.theme,
                    );
                    return;
                }
                ErasePrompt::Error { provider, message } => {
                    panel::render_text(
                        frame,
                        area,
                        "Could not delete key",
                        format!(
                            "The key for {provider} could not be deleted.\n\n{message}\n\nPress Esc to close."
                        ),
                        model.preferences.theme,
                    );
                    return;
                }
            }
        }
        if let Some(KeyPrompt::Edit {
            value,
            provider,
            error,
            ..
        }) = &self.key_prompt
        {
            let bullets = "•".repeat(value.chars().count());
            let body = format!(
                "{provider} has no key in the environment or secrets.json.\n\nPaste it here and press Enter:\n\n{bullets}\n\nEsc cancels · the key is never shown"
            );
            let body = match error {
                Some(message) => format!("{body}\n\n{message}"),
                None => body,
            };
            panel::render_text(
                frame,
                area,
                "API key required",
                body,
                model.preferences.theme,
            );
            return;
        }
        let rows = self.visible_rows(&model.models);
        self.selected = self.selected.min(rows.len().saturating_sub(1));
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);
        let filter_label = if self.filtering {
            format!("/{}_", self.filter)
        } else if self.filter.is_empty() {
            "/ filter providers and models".to_owned()
        } else {
            format!("/{}  · Esc clears", self.filter)
        };
        frame.render_widget(
            Paragraph::new(filter_label).style(palette::panel_style(model.preferences.theme)),
            areas[0],
        );

        let current_model = current_model(model);
        let queued_model = model
            .queued_model
            .as_ref()
            .map(|queued| (queued.provider.as_str(), queued.model.as_str()));
        let entries: Vec<ratatui::text::Line<'static>> = rows
            .iter()
            .map(|row| match row {
                ModelRow::Provider {
                    name,
                    model_count,
                    expanded,
                } => TextBuilder::model_provider_row(
                    name,
                    *model_count,
                    *expanded,
                    provider_signed_in(model, name),
                ),
                ModelRow::Model(model) => TextBuilder::model_option_row(
                    model,
                    model_marker(model, current_model, queued_model),
                ),
            })
            .collect();
        panel::render_list(
            frame,
            areas[1],
            entries.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Models,
                phase: self.lifecycle.phase(),
                selected: Some(self.selected),
                focused: true,
                empty_message: if self.filter.is_empty() {
                    TextBuilder::empty_message(ComponentId::Models)
                } else {
                    TextBuilder::no_model_matches()
                },
            },
        );
    }
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
