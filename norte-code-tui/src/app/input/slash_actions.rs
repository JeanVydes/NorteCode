//! Slash-command execution and composer completion handling.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::client::UiCommand;
use crate::ui;
use crate::ui_model::{Tab, UiModel};

use super::super::event_fold::notice;
use super::slash_command::{self, SlashCommand};

pub(super) fn execute_slash_command(
    tree: &mut terminalui::Tree,
    commands: Option<&tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
    input: &str,
    parts: &[norte_code_wire::UserInputPart],
) {
    let reasoning = model.active_model_info().map(|info| &info.reasoning);
    let command = match slash_command::parse(
        input,
        reasoning,
        &model.plugins,
        &model.sessions,
        &model.models,
    ) {
        Ok(command) => command,
        Err(error) => {
            notice(
                tree,
                format!("{error}; draft kept, nothing sent"),
                ui::Tone::Warn,
            );
            return;
        }
    };
    if !parts.is_empty()
        || tree
            .find::<ui::Composer>("composer")
            .is_some_and(|composer| !composer.pending_attachments().is_empty())
    {
        notice(
            tree,
            "slash commands cannot include attachments; draft kept".into(),
            ui::Tone::Warn,
        );
        return;
    }
    if execute_local_slash_command(tree, commands, model, &command, input) {
        return;
    }
    let Some(commands) = commands else {
        notice(
            tree,
            "the service is not connected; draft kept".into(),
            ui::Tone::Warn,
        );
        return;
    };
    let plugin_operation = matches!(&command, SlashCommand::ManagePlugin(_));
    let reasoning_update = match &command {
        SlashCommand::SetReasoning(reasoning) => Some(reasoning.clone()),
        _ => None,
    };
    let queued = match command {
        SlashCommand::ManagePlugin(op) => commands.try_send(UiCommand::Plugin(op)),
        SlashCommand::New(model) => commands.try_send(UiCommand::NewDraft { model }),
        SlashCommand::Open(session) => commands.try_send(UiCommand::OpenSession { session }),
        SlashCommand::Compact => {
            let Some(session) = active_command_session(tree, model) else {
                return;
            };
            commands.try_send(UiCommand::Compact { session })
        }
        SlashCommand::SetReasoning(reasoning) => {
            let Some(session) = active_command_session(tree, model) else {
                return;
            };
            commands.try_send(UiCommand::SetReasoning { session, reasoning })
        }
        SlashCommand::SetQueueMode(_)
        | SlashCommand::ToggleDebug
        | SlashCommand::Plans
        | SlashCommand::Host
        | SlashCommand::ClearData
        | SlashCommand::Preference(_) => {
            unreachable!("local slash commands are handled before a service command is queued")
        }
        SlashCommand::Plugin { command, arguments } => {
            let Some(session) = active_command_session(tree, model) else {
                return;
            };
            commands.try_send(UiCommand::RunCommand {
                session,
                command,
                arguments,
            })
        }
    };
    if let Err(error) = queued {
        notice(
            tree,
            format!("command could not be queued: {error}; draft kept"),
            ui::Tone::Warn,
        );
        return;
    }
    if reasoning_update.is_some() {
        model.active_reasoning = reasoning_update;
    }
    clear_command_draft(tree, input);
    if plugin_operation {
        notice(
            tree,
            "plugin operation started".to_owned(),
            ui::Tone::Neutral,
        );
    }
}

fn execute_local_slash_command(
    tree: &mut terminalui::Tree,
    commands: Option<&tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
    command: &SlashCommand,
    input: &str,
) -> bool {
    if matches!(command, SlashCommand::Host) {
        if let Some(overlay) = tree.find_mut::<ui::HostOverlay>("host_overlay") {
            overlay.open(model);
        }
        if let Some(commands) = commands {
            let _ = commands.try_send(UiCommand::RefreshViews);
        }
        clear_command_draft(tree, input);
        ui::sync_views(tree, model);
        return true;
    }
    if let SlashCommand::Preference(preference) = command {
        let theme = model.preferences.theme.clone();
        if super::preferences::apply(preference, &mut model.preferences).is_err() {
            notice(tree, "unknown theme; draft kept".to_owned(), ui::Tone::Warn);
            return true;
        }
        if model.preferences.theme != theme {
            ui::sync_theme(tree, model);
        }
        ui::refresh_views(tree, model);
        clear_command_draft(tree, input);
        return true;
    }
    if matches!(command, SlashCommand::ClearData) {
        if let Some(page) = tree.find_mut::<ui::SettingsPage>("settings") {
            page.open_clear_data();
        }
        model.tab = Tab::Settings;
        clear_command_draft(tree, input);
        ui::sync_views(tree, model);
        tree.focus("settings");
        return true;
    }
    if matches!(command, SlashCommand::Plans) {
        if let Some(overlay) = tree.find_mut::<ui::PlanOverlay>("plan_overlay") {
            overlay.open_list(model.plans.clone());
        }
        clear_command_draft(tree, input);
        ui::sync_views(tree, model);
        return true;
    }
    let message = match command {
        SlashCommand::SetReasoning(reasoning) if model.active.is_none() => {
            model.pending_new_session_reasoning = Some(reasoning.clone());
            model.active_reasoning = None;
            Some("reasoning set for the next session".to_owned())
        }
        SlashCommand::SetQueueMode(mode) => {
            model.preferences.send_mode = *mode;
            Some(format!("queue mode: {}", mode.label()))
        }
        SlashCommand::ToggleDebug => {
            let mode = model.effective_debug_overlay().next();
            model.debug_overlay_override = Some(mode);
            Some(format!("debug overlay {}", mode.label()))
        }
        _ => None,
    };
    let Some(message) = message else {
        return false;
    };
    notice(tree, message, ui::Tone::Neutral);
    clear_command_draft(tree, input);
    true
}

fn clear_command_draft(tree: &mut terminalui::Tree, input: &str) {
    if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
        composer.push_history(input);
        composer.clear();
    }
}

fn active_command_session(
    tree: &mut terminalui::Tree,
    model: &UiModel,
) -> Option<norte_code_wire::SessionKey> {
    let session = model.active.clone();
    if session.is_none() {
        notice(
            tree,
            "this command needs an active session; draft kept".into(),
            ui::Tone::Warn,
        );
    }
    session
}

pub(super) fn sync_slash_suggestions(tree: &mut terminalui::Tree, model: &UiModel) {
    let values = if model.tab == Tab::Chat {
        let input = tree
            .find::<ui::Composer>("composer")
            .map(ui::Composer::expanded_text)
            .unwrap_or_default();
        let mut suggestions = slash_command::suggestions(
            &input,
            model.active_model_info().map(|info| &info.reasoning),
            &model.plugins,
            &model.sessions,
            &model.models,
        );
        if let Some((label, prefix)) = input.split_once(' ') {
            suggestions.extend(
                super::preferences::values(label, &model.preferences)
                    .into_iter()
                    .filter(|value| value.to_lowercase().starts_with(&prefix.to_lowercase()))
                    .map(|value| ui::CommandSuggestion {
                        completion: format!("{label} {value}"),
                        label: value,
                    }),
            );
        }
        suggestions
    } else {
        Vec::new()
    };
    if let Some(suggestions) = tree.find_mut::<ui::CommandSuggestions>("command_suggestions") {
        suggestions.set(values);
    }
}

pub(super) fn handle_slash_suggestion_key(
    tree: &mut terminalui::Tree,
    key: crossterm::event::KeyEvent,
    model: &UiModel,
) -> bool {
    if model.tab != Tab::Chat || key.modifiers != KeyModifiers::NONE {
        return false;
    }
    if !tree
        .find::<ui::Composer>("composer")
        .is_some_and(ui::Composer::cursor_at_end)
    {
        return false;
    }
    sync_slash_suggestions(tree, model);
    if !tree
        .find::<ui::CommandSuggestions>("command_suggestions")
        .is_some_and(ui::CommandSuggestions::is_open)
    {
        return false;
    }
    match key.code {
        KeyCode::Tab => complete_selected_suggestion(tree, model),
        KeyCode::Enter => {
            let current = tree
                .find::<ui::Composer>("composer")
                .map(ui::Composer::expanded_text)
                .unwrap_or_default();
            let completion = tree
                .find::<ui::CommandSuggestions>("command_suggestions")
                .and_then(ui::CommandSuggestions::selected)
                .map(str::to_owned);
            let Some(completion) = completion.filter(|completion| completion != &current) else {
                return false;
            };
            if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
                composer.complete_command(&completion);
            }
            sync_slash_suggestions(tree, model);
            true
        }
        KeyCode::Up => {
            if let Some(suggestions) =
                tree.find_mut::<ui::CommandSuggestions>("command_suggestions")
            {
                suggestions.move_selection(-1);
                true
            } else {
                false
            }
        }
        KeyCode::Down => {
            if let Some(suggestions) =
                tree.find_mut::<ui::CommandSuggestions>("command_suggestions")
            {
                suggestions.move_selection(1);
                true
            } else {
                false
            }
        }
        KeyCode::Esc => {
            if let Some(suggestions) =
                tree.find_mut::<ui::CommandSuggestions>("command_suggestions")
            {
                suggestions.set(Vec::new());
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

fn complete_selected_suggestion(tree: &mut terminalui::Tree, model: &UiModel) -> bool {
    let completion = tree
        .find::<ui::CommandSuggestions>("command_suggestions")
        .and_then(ui::CommandSuggestions::selected)
        .map(str::to_owned);
    if let Some(completion) = completion
        && let Some(composer) = tree.find_mut::<ui::Composer>("composer")
    {
        composer.complete_command(&completion);
    }
    sync_slash_suggestions(tree, model);
    true
}
