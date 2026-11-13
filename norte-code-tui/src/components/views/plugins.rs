use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect};

use crate::{
    components::{
        Component, ComponentAction, ComponentId, ComponentLifecycle, PluginOperation, ViewAction,
        panel,
    },
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PluginMode {
    #[default]
    Listing,
    Repository,
    Package,
    ConfirmRemove,
}

#[derive(Default)]
pub(in crate::components) struct PluginsView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    mode: PluginMode,
    repository: String,
    package: String,
    pending_action: Option<ComponentAction>,
}

impl Component for PluginsView {
    fn id(&self) -> ComponentId {
        ComponentId::Plugins
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn captures_global_keys(&self) -> bool {
        self.mode != PluginMode::Listing
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        if self.mode == PluginMode::Repository || self.mode == PluginMode::Package {
            self.handle_text_input(key.code);
            return;
        }
        if self.mode == PluginMode::ConfirmRemove {
            match key.code {
                keys::CANCEL => self.mode = PluginMode::Listing,
                keys::SUBMIT => {
                    if let Some(plugin) = model.plugins.get(self.selected) {
                        self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(
                            PluginOperation::Remove {
                                name: plugin.name.clone(),
                            },
                        )));
                    }
                    self.mode = PluginMode::Listing;
                }
                _ => {}
            }
            return;
        }
        match key.code {
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                panel::move_selection(&mut self.selected, model.plugins.len(), true);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                panel::move_selection(&mut self.selected, model.plugins.len(), false);
            }
            KeyCode::Char('r') => {
                self.pending_action = Some(ComponentAction::View(ViewAction::RefreshPlugins));
            }
            KeyCode::Char('i') => {
                self.repository.clear();
                self.package.clear();
                self.mode = PluginMode::Repository;
            }
            KeyCode::Char('u') => self.update_selected(model),
            KeyCode::Char('U') => {
                self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(
                    PluginOperation::Update { name: None },
                )));
            }
            KeyCode::Char('e') => self.toggle_selected(model),
            KeyCode::Char('x') if model.plugins.get(self.selected).is_some() => {
                self.mode = PluginMode::ConfirmRemove;
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        match self.mode {
            PluginMode::Repository => {
                panel::render_text(
                    frame,
                    area,
                    "install plugin",
                    format!(
                        "Install only trusted repositories. The host builds plugin code.\n\nRepository URL:\n\n{}",
                        self.repository
                    ),
                    model.preferences.theme,
                );
                return;
            }
            PluginMode::Package => {
                panel::render_text(
                    frame,
                    area,
                    "install plugin",
                    format!(
                        "Install only trusted repositories. The host builds plugin code.\n\nPackage (optional):\n\n{}",
                        self.package
                    ),
                    model.preferences.theme,
                );
                return;
            }
            PluginMode::ConfirmRemove => {
                let name = model
                    .plugins
                    .get(self.selected)
                    .map_or("", |plugin| plugin.name.as_str());
                panel::render_text(
                    frame,
                    area,
                    "remove plugin",
                    format!("Remove {name}? Enter confirms, Esc cancels"),
                    model.preferences.theme,
                );
                return;
            }
            PluginMode::Listing => {}
        }
        let entries = model.plugins.iter().map(TextBuilder::plugin_row).collect();
        panel::render_list(
            frame,
            area,
            entries,
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Plugins,
                phase: self.lifecycle.phase(),
                selected: Some(self.selected),
                focused: true,
                empty_message: TextBuilder::empty_message(ComponentId::Plugins),
            },
        );
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}

impl PluginsView {
    fn handle_text_input(&mut self, code: KeyCode) {
        match code {
            keys::CANCEL => self.mode = PluginMode::Listing,
            keys::ERASE => match self.mode {
                PluginMode::Repository => {
                    self.repository.pop();
                }
                PluginMode::Package => {
                    self.package.pop();
                }
                PluginMode::Listing | PluginMode::ConfirmRemove => {}
            },
            keys::SUBMIT if self.mode == PluginMode::Repository => {
                if !self.repository.trim().is_empty() {
                    self.mode = PluginMode::Package;
                }
            }
            keys::SUBMIT => {
                let url = self.repository.trim().to_owned();
                let package = self.package.trim();
                self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(
                    PluginOperation::Install {
                        url,
                        package: (!package.is_empty()).then(|| package.to_owned()),
                    },
                )));
                self.mode = PluginMode::Listing;
            }
            KeyCode::Char(character) => match self.mode {
                PluginMode::Repository => self.repository.push(character),
                PluginMode::Package => self.package.push(character),
                PluginMode::Listing | PluginMode::ConfirmRemove => {}
            },
            _ => {}
        }
    }

    fn update_selected(&mut self, model: &UiModel) {
        if let Some(plugin) = model.plugins.get(self.selected) {
            self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(
                PluginOperation::Update {
                    name: Some(plugin.name.clone()),
                },
            )));
        }
    }

    fn toggle_selected(&mut self, model: &UiModel) {
        if let Some(plugin) = model.plugins.get(self.selected) {
            let operation = if plugin.enabled {
                PluginOperation::Disable {
                    name: plugin.name.clone(),
                }
            } else {
                PluginOperation::Enable {
                    name: plugin.name.clone(),
                }
            };
            self.pending_action = Some(ComponentAction::View(ViewAction::Plugin(operation)));
        }
    }
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
