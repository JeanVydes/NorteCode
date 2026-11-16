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
            let matching = models
                .iter()
                .filter(|model| model.provider == provider)
                .filter(|model| {
                    provider_matches
                        || model.model.to_lowercase().contains(query.as_str())
                        || model.name.to_lowercase().contains(query.as_str())
                })
                .collect::<Vec<_>>();
            if !query.is_empty() && !provider_matches && matching.is_empty() {
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
                if !self.expanded.remove(name) {
                    self.expanded.insert(name.to_owned());
                }
                self.clamp_selection(&model.models);
            }
            Some(ModelRow::Model(selected))
                if model.active_child.is_none() && model.active_session.is_some() =>
            {
                self.pending_action = Some(ComponentAction::SetModel {
                    provider: selected.provider.clone(),
                    model: selected.model.clone(),
                });
            }
            Some(ModelRow::Model(_)) | None => {}
        }
    }

    fn clamp_selection(&mut self, models: &[ModelInfo]) {
        self.selected = self
            .selected
            .min(self.visible_rows(models).len().saturating_sub(1));
    }
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
        self.filtering
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
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
            KeyCode::Char(keys::MODEL_SEARCH_CHAR) => {
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
            _ => {}
        }
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
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
                } => TextBuilder::model_provider_row(name, *model_count, *expanded),
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
