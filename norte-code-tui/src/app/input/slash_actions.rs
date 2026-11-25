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
    model: &UiModel,
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
    if !parts.is_empty() || !model.pending_attachments.is_empty() {
        notice(
            tree,
            "slash commands cannot include attachments; draft kept".into(),
            ui::Tone::Warn,
        );
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
    let queued = match command {
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
        slash_command::suggestions(
            &input,
            model.active_model_info().map(|info| &info.reasoning),
            &model.plugins,
            &model.sessions,
            &model.models,
        )
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
    let navigates_choices = tree
        .find::<ui::Composer>("composer")
        .map(ui::Composer::expanded_text)
        .is_some_and(|text| {
            text == "/open" || text.starts_with("/open ") || text.starts_with("/new ")
        });
    let Some(suggestions) = tree.find_mut::<ui::CommandSuggestions>("command_suggestions") else {
        return false;
    };
    if !suggestions.is_open() {
        return false;
    }
    match key.code {
        KeyCode::Tab => {
            let completion = suggestions.selected().map(str::to_owned);
            if let Some(completion) = completion
                && let Some(composer) = tree.find_mut::<ui::Composer>("composer")
            {
                composer.complete_command(&completion);
            }
            sync_slash_suggestions(tree, model);
            true
        }
        KeyCode::Up => {
            if !navigates_choices {
                return false;
            }
            suggestions.move_selection(-1);
            true
        }
        KeyCode::Down => {
            if !navigates_choices {
                return false;
            }
            suggestions.move_selection(1);
            true
        }
        KeyCode::Esc => {
            suggestions.set(Vec::new());
            true
        }
        _ => false,
    }
}
