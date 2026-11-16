use crossterm::event::KeyCode;
use norte_code_wire::catalog::{Model, Provider, Wire, WireSchema};
use ratatui::{Frame, layout::Rect};

use super::{CatalogTarget, CatalogView};
use crate::{
    components::{CatalogOperation, ComponentAction, ViewAction, panel},
    keys,
};

#[derive(Default)]
pub(super) enum CatalogForm {
    #[default]
    Closed,
    Provider {
        id: String,
        name: String,
        api: String,
        environment_keys: String,
        field: usize,
    },
    Model {
        provider: String,
        id: String,
        name: String,
        field: usize,
    },
    Confirm(CatalogTarget),
}

impl CatalogView {
    pub(super) fn submit_form(&mut self) {
        match std::mem::take(&mut self.form) {
            CatalogForm::Provider {
                id,
                name,
                api,
                environment_keys,
                field,
            } => self.submit_provider_form(id, name, api, environment_keys, field),
            CatalogForm::Model {
                provider,
                id,
                name,
                field,
            } => self.submit_model_form(provider, id, name, field),
            CatalogForm::Confirm(target) => self.submit_confirmation(target),
            CatalogForm::Closed => self.form = CatalogForm::Closed,
        }
    }

    fn submit_provider_form(
        &mut self,
        id: String,
        name: String,
        api: String,
        environment_keys: String,
        field: usize,
    ) {
        if field < 3 {
            self.form = CatalogForm::Provider {
                id,
                name,
                api,
                environment_keys,
                field: field + 1,
            };
            return;
        }
        if id.trim().is_empty() || api.trim().is_empty() {
            self.notice = Some("Provider id and base URL are required.".to_owned());
            self.form = CatalogForm::Provider {
                id,
                name,
                api,
                environment_keys,
                field: 0,
            };
            return;
        }
        let provider = Provider {
            name: if name.trim().is_empty() {
                id.trim().to_owned()
            } else {
                name.trim().to_owned()
            },
            environment_keys: environment_keys
                .split(',')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
                .collect(),
            wires: vec![Wire {
                api: api.trim().to_owned(),
                schema: WireSchema::OpenAI,
                auth: None,
            }],
            ..Provider::default()
        };
        self.pending_action = Some(ComponentAction::View(ViewAction::Catalog(Box::new(
            CatalogOperation::UpsertProvider {
                id: id.trim().to_owned(),
                provider: Box::new(provider),
            },
        ))));
        self.notice = None;
    }

    fn submit_model_form(&mut self, provider: String, id: String, name: String, field: usize) {
        if field == 0 {
            self.form = CatalogForm::Model {
                provider,
                id,
                name,
                field: 1,
            };
            return;
        }
        if id.trim().is_empty() {
            self.notice = Some("Model id is required.".to_owned());
            self.form = CatalogForm::Model {
                provider,
                id,
                name,
                field: 0,
            };
            return;
        }
        let id = id.trim().to_owned();
        let model = Model {
            provider_id: provider,
            canonical_slug: id.clone(),
            name: if name.trim().is_empty() {
                id.clone()
            } else {
                name.trim().to_owned()
            },
            id,
            ..Model::default()
        };
        self.pending_action = Some(ComponentAction::View(ViewAction::Catalog(Box::new(
            CatalogOperation::UpsertModel(Box::new(model)),
        ))));
        self.notice = None;
    }

    fn submit_confirmation(&mut self, target: CatalogTarget) {
        let operation = match target {
            CatalogTarget::Provider(provider) => CatalogOperation::RemoveProvider(provider),
            CatalogTarget::Model { provider, model } => {
                CatalogOperation::RemoveModel { provider, model }
            }
        };
        self.pending_action = Some(ComponentAction::View(ViewAction::Catalog(Box::new(
            operation,
        ))));
        self.notice = None;
    }

    pub(super) fn edit_form(&mut self, key: KeyCode) {
        match &mut self.form {
            CatalogForm::Provider {
                id,
                name,
                api,
                environment_keys,
                field,
            } => {
                let value = match field {
                    0 => id,
                    1 => name,
                    2 => api,
                    _ => environment_keys,
                };
                edit_value(value, key);
            }
            CatalogForm::Model {
                id, name, field, ..
            } => {
                let value = if *field == 0 { id } else { name };
                edit_value(value, key);
            }
            CatalogForm::Closed | CatalogForm::Confirm(_) => {}
        }
    }

    pub(super) fn render_form(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: crate::ui_model::ThemeMode,
    ) {
        let body = match &self.form {
            CatalogForm::Provider {
                id,
                name,
                api,
                environment_keys,
                field,
            } => {
                let values = [id.as_str(), name.as_str(), api.as_str(), environment_keys];
                let labels = [
                    "Provider id",
                    "Display name",
                    "Base URL",
                    "Environment keys",
                ];
                form_body("Add provider", &labels, &values, *field)
            }
            CatalogForm::Model {
                provider,
                id,
                name,
                field,
            } => form_body(
                "Add model",
                &["Provider", "Model id", "Display name"],
                &[provider, id, name],
                field + 1,
            ),
            CatalogForm::Confirm(CatalogTarget::Provider(provider)) => format!(
                "Remove provider {provider} and its models?\n\nEnter confirms · Esc cancels"
            ),
            CatalogForm::Confirm(CatalogTarget::Model { provider, model }) => {
                format!("Remove model {provider}/{model}?\n\nEnter confirms · Esc cancels")
            }
            CatalogForm::Closed => String::new(),
        };
        let body = self
            .notice
            .as_ref()
            .map_or(body.clone(), |notice| format!("{body}\n\n{notice}"));
        panel::render_text(frame, area, "Provider catalogue", body, theme);
    }
}

fn edit_value(value: &mut String, key: KeyCode) {
    match key {
        keys::ERASE => {
            value.pop();
        }
        KeyCode::Char(character) => value.push(character),
        _ => {}
    }
}

fn form_body<const N: usize>(
    title: &str,
    labels: &[&str; N],
    values: &[&str; N],
    field: usize,
) -> String {
    let fields = labels
        .iter()
        .zip(values)
        .enumerate()
        .map(|(index, (label, value))| {
            let marker = if index == field { ">" } else { " " };
            format!("{marker} {label}: {value}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{title}\n\n{fields}\n\nType · Tab next · Enter next/save · Esc cancel")
}
