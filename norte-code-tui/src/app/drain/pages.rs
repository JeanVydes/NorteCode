//! Page actions: sessions, models, plugins, and settings.

use terminalui::components::button::Button;

use crate::client::UiCommand;
use crate::ui;
use crate::ui_model::{Tab, UiModel};

use super::super::input::{open_key_prompt, set_model};

/// Drain the sessions page's pending actions into the client.
pub(in crate::app) fn drain_sessions_actions(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    if tree
        .find_mut::<Button>("new_session")
        .is_some_and(Button::take_pressed)
    {
        create_session(tree, commands.as_ref(), model);
    }
    let Some(page) = tree.find_mut::<ui::SessionsPage>("sessions") else {
        return;
    };
    for action in page.take_pending() {
        match action {
            ui::SessionsAction::Create => create_session(tree, commands.as_ref(), model),
            ui::SessionsAction::Attach { session } => {
                if let Some(commands) = commands.as_ref() {
                    let _ = commands.try_send(UiCommand::OpenSession { session });
                }
                switch_to_chat(tree, model);
            }
            ui::SessionsAction::Rename { session, title } => {
                if let Some(commands) = commands.as_ref() {
                    let _ = commands.try_send(UiCommand::RenameSession { session, title });
                }
            }
            ui::SessionsAction::Delete { session } => {
                let own = model
                    .active
                    .as_ref()
                    .is_some_and(|active| active.as_str() == session.as_str());
                if own {
                    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                        stream.push(ui::Item::Notice {
                            text: "cannot delete the attached session; attach to another one first"
                                .to_string(),
                            tone: ui::Tone::Warn,
                        });
                    }
                    continue;
                }
                if let Some(commands) = commands.as_ref() {
                    let _ = commands.try_send(UiCommand::DeleteSession { session });
                }
            }
        }
    }
}

fn create_session(
    tree: &mut terminalui::Tree,
    commands: Option<&tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    if let Some(commands) = commands {
        let _ = commands.try_send(UiCommand::NewDraft { model: None });
    }
    switch_to_chat(tree, model);
}

fn switch_to_chat(tree: &mut terminalui::Tree, model: &mut UiModel) {
    model.tab = Tab::Chat;
    ui::sync_views(tree, model);
}

/// Ship a plugin operation to the host; it reports back through the stream.
fn send_plugin_op(
    tree: &mut terminalui::Tree,
    commands: Option<&tokio::sync::mpsc::Sender<UiCommand>>,
    op: crate::client::PluginOp,
    summary: String,
) {
    if let Some(commands) = commands {
        let _ = commands.try_send(UiCommand::Plugin(op));
    }
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::Notice {
            text: summary,
            tone: ui::Tone::Neutral,
        });
    }
}

/// Drain the settings page's pending plugin actions into the client.
pub(in crate::app) fn drain_plugins_actions(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
) {
    let clear_data = tree
        .find_mut::<ui::SettingsPage>("settings")
        .is_some_and(ui::SettingsPage::take_clear_data_requested);
    if clear_data && let Some(commands) = commands.as_ref() {
        let _ = commands.try_send(UiCommand::ClearData);
    }
    let actions = tree
        .find_mut::<ui::SettingsPage>("settings")
        .map_or_else(Vec::new, ui::SettingsPage::take_plugin_actions);
    for action in actions {
        match action {
            ui::PluginsAction::Install { url, package } => {
                send_plugin_op(
                    tree,
                    commands.as_ref(),
                    crate::client::PluginOp::Install {
                        url: url.clone(),
                        package: package.clone(),
                    },
                    format!("plugin install started · {url}"),
                );
            }
            ui::PluginsAction::Update { name } => {
                let summary = match &name {
                    Some(name) => format!("plugin update started · {name}"),
                    None => "updating every plugin".to_owned(),
                };
                send_plugin_op(
                    tree,
                    commands.as_ref(),
                    crate::client::PluginOp::Update { name },
                    summary,
                );
            }
            ui::PluginsAction::Remove { name } => {
                send_plugin_op(
                    tree,
                    commands.as_ref(),
                    crate::client::PluginOp::Remove { name: name.clone() },
                    format!("plugin remove started · {name}"),
                );
            }
            ui::PluginsAction::SetEnabled { name, enabled } => {
                let verb = if enabled { "enabling" } else { "disabling" };
                send_plugin_op(
                    tree,
                    commands.as_ref(),
                    crate::client::PluginOp::SetEnabled {
                        name: name.clone(),
                        enabled,
                    },
                    format!("{verb} plugin · {name}"),
                );
            }
        }
    }
}

/// Apply the preference changes the config tab reported.
pub(in crate::app) fn drain_config_actions(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) -> bool {
    use crate::ui::{ConfigAction, Setting};

    let actions = tree
        .find_mut::<ui::SettingsPage>("settings")
        .map_or_else(Vec::new, ui::SettingsPage::take_config_actions);
    let mut changed = false;
    let mut theme_changed = false;
    for action in actions {
        changed = true;
        match action {
            ConfigAction::SetChartValue(setting, value) => match setting {
                Setting::ChartDensity => model.preferences.chart_density = value.clamp(2, 6),
                Setting::ChartContrast => model.preferences.chart_contrast = value.clamp(1, 16),
                Setting::ChartDotSize => model.preferences.chart_dot_size = value.min(2),
                _ => {}
            },
            ConfigAction::Cycle(setting) => match setting {
                Setting::Theme => {
                    let choice = model.preferences.theme.clone();
                    model.preferences.theme = model.preferences.catalog.next(&choice);
                    theme_changed = true;
                }
                Setting::Thinking => {
                    model.preferences.reasoning = model.preferences.reasoning.next();
                }
                Setting::SendMode => {
                    model.preferences.send_mode = model.preferences.send_mode.next();
                }
                Setting::DiffMode => {
                    model.preferences.diff_mode = model.preferences.diff_mode.next();
                }
                Setting::RailPosition => {
                    model.preferences.rail_position = model.preferences.rail_position.next();
                }
                Setting::Debug => model.preferences.debug = model.preferences.debug.next(),
                Setting::ShowToolOutput => {
                    model.preferences.show_tool_output = !model.preferences.show_tool_output;
                }
                Setting::ToolVerbosity => {
                    model.preferences.tool_verbosity = !model.preferences.tool_verbosity;
                }
                Setting::ChartDensity | Setting::ChartContrast => {}
                Setting::ChartDotSize => {
                    model.preferences.chart_dot_size = (model.preferences.chart_dot_size + 1) % 3;
                }
            },
        }
    }
    if theme_changed {
        ui::sync_theme(tree, model);
    }
    changed
}

/// Drain the models page's pending actions into the client.
pub(in crate::app) fn drain_models_actions(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    let Some(page) = tree.find_mut::<ui::ModelsPage>("models") else {
        return;
    };
    for action in page.take_pending() {
        match action {
            ui::ModelsAction::SetModel {
                provider,
                model: model_id,
            } => {
                set_model(commands, model, &provider, &model_id);
            }
            ui::ModelsAction::AskKeyForModel {
                provider,
                model: model_id,
            } => {
                let keys = model.environment_keys(&provider);
                open_key_prompt(tree, &provider, Some(model_id), &keys);
            }
            ui::ModelsAction::AskKeyForExpand { provider } => {
                let keys = model.environment_keys(&provider);
                open_key_prompt(tree, &provider, None, &keys);
            }
            ui::ModelsAction::ToggleExpand { provider } => {
                if let Some(page) = tree.find_mut::<ui::ModelsPage>("models") {
                    page.toggle_expansion(&provider);
                }
            }
            ui::ModelsAction::DeleteKey { provider } => {
                if let Some(commands) = commands.as_ref() {
                    let _ = commands.try_send(UiCommand::DeleteSecret { provider });
                }
            }
        }
    }
}
