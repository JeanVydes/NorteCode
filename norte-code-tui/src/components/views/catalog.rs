use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect, text::Line};

#[path = "catalog/form.rs"]
mod form;

use form::CatalogForm;

use crate::{
    components::{Component, ComponentAction, ComponentId, ComponentLifecycle, ViewAction, panel},
    keys,
    ui_model::UiModel,
};

#[derive(Clone, Copy)]
enum CatalogRow {
    Provider(usize),
    Model(usize),
}

#[derive(Clone)]
pub(super) enum CatalogTarget {
    Provider(String),
    Model { provider: String, model: String },
}

#[derive(Default)]
pub(in crate::components) struct CatalogView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    form: CatalogForm,
    notice: Option<String>,
    pending_action: Option<ComponentAction>,
}

impl CatalogView {
    fn rows(model: &UiModel) -> Vec<CatalogRow> {
        let mut rows = Vec::new();
        for (provider_index, provider) in model.catalog.providers.iter().enumerate() {
            rows.push(CatalogRow::Provider(provider_index));
            rows.extend(
                model
                    .catalog
                    .models
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.provider == provider.provider_id)
                    .map(|(model_index, _)| CatalogRow::Model(model_index)),
            );
        }
        rows
    }

    fn selected_provider(&self, model: &UiModel) -> Option<String> {
        match Self::rows(model).get(self.selected).copied()? {
            CatalogRow::Provider(index) => model.catalog.providers.get(index),
            CatalogRow::Model(index) => {
                let entry = model.catalog.models.get(index)?;
                model
                    .catalog
                    .providers
                    .iter()
                    .find(|provider| provider.provider_id == entry.provider)
            }
        }
        .filter(|provider| provider.from_file)
        .map(|provider| provider.provider_id.clone())
    }

    fn target(&self, model: &UiModel) -> Option<CatalogTarget> {
        match Self::rows(model).get(self.selected).copied()? {
            CatalogRow::Provider(index) => {
                let provider = model.catalog.providers.get(index)?;
                provider
                    .from_file
                    .then(|| CatalogTarget::Provider(provider.provider_id.clone()))
            }
            CatalogRow::Model(index) => {
                let model = model.catalog.models.get(index)?;
                model.from_file.then(|| CatalogTarget::Model {
                    provider: model.provider.clone(),
                    model: model.model.clone(),
                })
            }
        }
    }
}

impl Component for CatalogView {
    fn id(&self) -> ComponentId {
        ComponentId::Catalog
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn captures_global_keys(&self) -> bool {
        !matches!(&self.form, CatalogForm::Closed)
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        if !matches!(&self.form, CatalogForm::Closed) {
            match key.code {
                keys::CANCEL => {
                    self.form = CatalogForm::Closed;
                    self.notice = None;
                }
                keys::SUBMIT => self.submit_form(),
                KeyCode::Tab => match &mut self.form {
                    CatalogForm::Provider { field, .. } => *field = (*field + 1).min(3),
                    CatalogForm::Model { field, .. } => *field = (*field + 1).min(1),
                    CatalogForm::Closed | CatalogForm::Confirm(_) => {}
                },
                code => self.edit_form(code),
            }
            return;
        }

        match key.code {
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                panel::move_selection(&mut self.selected, Self::rows(model).len(), true);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                panel::move_selection(&mut self.selected, Self::rows(model).len(), false);
            }
            KeyCode::Char('r') => {
                self.pending_action = Some(ComponentAction::View(ViewAction::RefreshCatalog));
            }
            KeyCode::Char('a') => {
                self.form = CatalogForm::Provider {
                    id: String::new(),
                    name: String::new(),
                    api: String::new(),
                    environment_keys: String::new(),
                    field: 0,
                };
                self.notice = None;
            }
            KeyCode::Char('m') => {
                if let Some(provider) = self.selected_provider(model) {
                    self.form = CatalogForm::Model {
                        provider,
                        id: String::new(),
                        name: String::new(),
                        field: 0,
                    };
                    self.notice = None;
                } else {
                    self.notice = Some("Select a provider managed by the host files.".to_owned());
                }
            }
            KeyCode::Char('d') => {
                if let Some(target) = self.target(model) {
                    self.form = CatalogForm::Confirm(target);
                    self.notice = None;
                } else {
                    self.notice = Some("Plugin catalogue entries are read-only.".to_owned());
                }
            }
            _ => {}
        }
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        if !matches!(&self.form, CatalogForm::Closed) {
            self.render_form(frame, area, model.preferences.theme);
            return;
        }
        let rows = Self::rows(model);
        self.selected = self.selected.min(rows.len().saturating_sub(1));
        let entries: Vec<ratatui::text::Line<'static>> = rows
            .iter()
            .map(|row| match row {
                CatalogRow::Provider(index) => {
                    let provider = &model.catalog.providers[*index];
                    let source = if provider.from_file {
                        "file"
                    } else {
                        "plugin, read-only"
                    };
                    Line::from(format!(
                        "{}  {}  {}  ·  {} models",
                        provider.name, provider.provider_id, source, provider.model_count
                    ))
                }
                CatalogRow::Model(index) => {
                    let model = &model.catalog.models[*index];
                    let source = if model.from_file {
                        "file"
                    } else {
                        "plugin, read-only"
                    };
                    Line::from(format!("  {}  {}  {}", model.name, model.model, source))
                }
            })
            .collect();
        panel::render_list(
            frame,
            area,
            entries.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Catalog,
                phase: self.lifecycle.phase(),
                selected: Some(self.selected),
                focused: true,
                empty_message: self
                    .notice
                    .as_deref()
                    .unwrap_or("No catalogue entries loaded"),
            },
        );
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
