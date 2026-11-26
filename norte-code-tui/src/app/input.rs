//! Input dispatch: terminal events into tree events and app actions.

#[path = "input/key.rs"]
mod key;
#[path = "input/modals.rs"]
mod modals;
#[path = "input/navigation.rs"]
mod navigation;
#[path = "input/palette.rs"]
mod palette;
mod plugin_command;
mod preferences;
#[path = "input/selectors.rs"]
mod selectors;
#[path = "input/slash_actions.rs"]
mod slash_actions;
#[path = "input/slash_command.rs"]
mod slash_command;
#[path = "input/slash_suggestions.rs"]
mod slash_suggestions;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};

use crate::client::{ClientEvent, UiCommand};
use crate::config::ClientConfig;
use crate::ui;
use crate::ui_model::{Tab, UiModel};

use super::drain::{
    drain_composer_actions, drain_config_actions, drain_message_actions, drain_models_actions,
    drain_plugins_actions, drain_sessions_actions, drain_toasts,
};
use super::{InputKind, PollOutcome};

use key::drain_key_prompt;
use modals::{drain_plan_review, drain_question_answer};
use palette::drain_palette;
use selectors::drain_tab_request;

pub(in crate::app) use key::{apply_key_intent, handle_key, open_key_prompt, set_model};
pub(in crate::app) use modals::drain_plan_requests;
pub(in crate::app) use navigation::{digit_switches_tab, toggle_list_focus};

/// How input events are folded into the model.
pub(in crate::app) fn poll_event(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    events: &mut Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
    model: &mut UiModel,
    config: Option<&ClientConfig>,
    timeout: Duration,
) -> io::Result<PollOutcome> {
    if !event::poll(timeout)? {
        return Ok(PollOutcome::Idle);
    }
    let previous = model.preferences.clone();
    let outcome = match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            let quit = handle_key(tree, key, commands, events, model, config);
            Ok(if quit {
                PollOutcome::Quit
            } else {
                PollOutcome::Event(InputKind::Key)
            })
        }
        Event::Mouse(mouse) if clear_overlay_open(tree) => {
            let action = tree
                .find_mut::<ui::ClearDataOverlay>("clear_data_overlay")
                .and_then(|overlay| overlay.mouse(mouse));
            handle_clear_action(tree, commands.as_ref(), action);
            Ok(PollOutcome::Event(InputKind::Mouse))
        }
        Event::Mouse(mouse) => {
            if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
                composer.hover(mouse.column, mouse.row);
            }
            let input = terminalui::InputEvent::from(Event::Mouse(mouse));
            let _ = tree.handle_event(&input);
            drain_tab_request(tree, model);
            drain_models_actions(tree, commands, model);
            drain_plugins_actions(tree, commands);
            drain_message_actions(tree, commands, model);
            drain_composer_actions(tree, commands, model);
            drain_toasts(tree);
            drain_plan_requests(tree, model);
            if drain_config_actions(tree, model) {
                ui::refresh_views(tree, model);
            }
            drain_sessions_actions(tree, commands, model);
            ui::sync_views(tree, model);
            Ok(PollOutcome::Event(InputKind::Mouse))
        }
        other if clear_overlay_open(tree) => {
            let _ = other;
            Ok(PollOutcome::Event(InputKind::Other))
        }
        other => {
            let input = terminalui::InputEvent::from(other);
            let _ = tree.handle_event(&input);
            drain_composer_actions(tree, commands, model);
            ui::sync_views(tree, model);
            Ok(PollOutcome::Event(InputKind::Other))
        }
    };
    if model.preferences != previous {
        ui::refresh_views(tree, model);
        if let Err(error) = crate::config::save_preferences(&model.preferences) {
            super::event_fold::toast(
                tree,
                &format!("Could not save preferences: {error}"),
                ui::Tone::Warn,
            );
        }
    }
    outcome
}

pub(in crate::app) fn cycle_theme(tree: &mut terminalui::Tree, model: &mut UiModel) {
    let choice = model.preferences.theme.clone();
    model.preferences.theme = model.preferences.catalog.next(&choice);
    ui::refresh_views(tree, model);
    ui::sync_theme(tree, model);
}

pub(in crate::app) fn scroll_page(tree: &mut terminalui::Tree, tab: Tab, delta: isize) {
    if tab == Tab::Telemetry {
        if let Some(page) = tree.find_mut::<ui::TelemetryPage>("telemetry") {
            page.scroll_chart_view(delta);
        }
    } else if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.scroll(delta);
    }
}

pub(in crate::app) fn settle_input(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    drain_palette(tree, commands, model);
    if let Some(prompt) = tree.find_mut::<ui::KeyPrompt>("key_prompt") {
        prompt.drain();
    }
    drain_key_prompt(tree, commands.as_ref());
    drain_plan_review(tree, commands, model);
    drain_question_answer(tree, commands, model);
    drain_models_actions(tree, commands, model);
    drain_sessions_actions(tree, commands, model);
    let modal_focus = tree
        .find::<ui::SessionsPage>("sessions")
        .and_then(ui::SessionsPage::focus_id)
        .or_else(|| {
            tree.find::<ui::ModelsPage>("models")
                .and_then(ui::ModelsPage::focus_id)
        })
        .map(str::to_owned)
        .or_else(|| (model.tab == Tab::Sessions).then(|| "sessions".to_owned()))
        .or_else(|| (model.tab == Tab::Models).then(|| "models".to_owned()));
    if let Some(id) = modal_focus {
        tree.focus(&id);
    }
    drain_plugins_actions(tree, commands);
    drain_message_actions(tree, commands, model);
    drain_composer_actions(tree, commands, model);
    drain_toasts(tree);
    drain_plan_requests(tree, model);
    if drain_config_actions(tree, model) {
        ui::refresh_views(tree, model);
    }
    drain_tab_request(tree, model);
    ui::sync_views(tree, model);
}

fn clear_overlay_open(tree: &terminalui::Tree) -> bool {
    tree.find::<ui::ClearDataOverlay>("clear_data_overlay")
        .is_some_and(ui::ClearDataOverlay::is_open)
}

fn handle_clear_action(
    tree: &mut terminalui::Tree,
    commands: Option<&tokio::sync::mpsc::Sender<UiCommand>>,
    action: Option<ui::ClearAction>,
) {
    let Some(action) = action else {
        return;
    };
    let error = match action {
        ui::ClearAction::Cancel => None,
        ui::ClearAction::Confirm => match commands {
            Some(commands) => commands
                .try_send(UiCommand::ClearData)
                .err()
                .map(|error| format!("Could not start: {error}")),
            None => Some("Service disconnected; nothing was cleared".to_owned()),
        },
    };
    if let Some(overlay) = tree.find_mut::<ui::ClearDataOverlay>("clear_data_overlay") {
        if let Some(error) = error {
            overlay.fail(error);
        } else {
            overlay.close();
        }
    }
}

fn handle_clear_key(
    tree: &mut terminalui::Tree,
    commands: Option<&tokio::sync::mpsc::Sender<UiCommand>>,
    key: crossterm::event::KeyEvent,
) -> bool {
    if !clear_overlay_open(tree) {
        return false;
    }
    let action = tree
        .find_mut::<ui::ClearDataOverlay>("clear_data_overlay")
        .and_then(|overlay| overlay.key(key));
    handle_clear_action(tree, commands, action);
    true
}
