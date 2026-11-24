//! Turn and tool events that drive the transcript stream.

use norte_code_wire::ServerEventKind;

use crate::client::UiCommand;
use crate::diff::{Diff, EditArgs};
use crate::shell::ShellArgs;
use crate::ui;
use crate::ui_model::{TurnState, UiModel};

use super::notice;

pub(in crate::app) fn fold_turn_event(
    kind: &ServerEventKind,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
) {
    if let ServerEventKind::TurnStarted { .. } = kind {
        model.turn = TurnState::Connecting;
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            stream.start_thinking();
        }
        return;
    }
    if let ServerEventKind::InputAccepted { turn_id, .. } = kind {
        model.turn = TurnState::Connecting;
        model.active_turn = Some(crate::ui_model::ActiveTurn {
            id: turn_id.clone(),
        });
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            stream.start_thinking();
        }
        return;
    }
    if let ServerEventKind::TurnPhase { phase, .. } = kind {
        model.turn = match phase {
            norte_code_wire::TurnPhaseName::Queued
            | norte_code_wire::TurnPhaseName::CallingProvider => TurnState::Connecting,
            norte_code_wire::TurnPhaseName::ExecutingTool => TurnState::Streaming,
            norte_code_wire::TurnPhaseName::AwaitingApproval => TurnState::AwaitingAnswer,
        };
        return;
    }
    if let ServerEventKind::QuestionAnswered { .. } = kind {
        model.question = None;
        if model.turn == TurnState::AwaitingAnswer {
            model.turn = TurnState::Connecting;
        }
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            stream.start_thinking();
        }
        notice(tree, "question answered".to_string(), ui::Tone::Neutral);
        return;
    }
    if let ServerEventKind::TurnFinished { .. } | ServerEventKind::TurnStopped { .. } = kind {
        finish_turn(tree, model, commands);
    }
}

pub(in crate::app) fn fold_tool_event(
    kind: &ServerEventKind,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
) {
    if let ServerEventKind::ToolStarted {
        tool,
        call_id,
        arguments,
        source,
        origin,
        ..
    } = kind
    {
        fold_tool_started(
            tool,
            call_id,
            arguments.as_ref(),
            source.as_deref(),
            origin.as_ref(),
            tree,
            model,
        );
        return;
    }
    if let ServerEventKind::ToolOutput { call_id, text, .. } = kind {
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            stream.append_console_line(call_id, text.clone());
        }
        return;
    }
    if let ServerEventKind::ToolFinished {
        call_id,
        tool,
        is_error,
        ..
    } = kind
    {
        fold_tool_finished(call_id, tool, *is_error, tree, commands);
        return;
    }
    if let ServerEventKind::Delta { delta, .. } = kind {
        fold_delta(delta, tree, model);
    }
}

/// Finish a tool row and refetch the rail as soon as a plan changes.
fn fold_tool_finished(
    call_id: &str,
    tool: &str,
    is_error: bool,
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
) {
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.finish_tool(call_id);
    }
    if tool == "plan"
        && !is_error
        && let Some(commands) = commands.as_ref()
    {
        let _ = commands.try_send(UiCommand::RefreshRail);
    }
}

fn fold_tool_started(
    tool: &str,
    call_id: &str,
    arguments: Option<&serde_json::Value>,
    source: Option<&str>,
    origin: Option<&norte_code_wire::payload::PluginToolOrigin>,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) {
    model.turn = TurnState::Streaming;
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        let target = arguments.and_then(|value| {
            value
                .get("path")
                .or_else(|| value.get("file"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });
        let edit =
            arguments.and_then(|value| serde_json::from_value::<EditArgs>(value.clone()).ok());
        let shell =
            arguments.and_then(|value| serde_json::from_value::<ShellArgs>(value.clone()).ok());
        let command = shell.as_ref().and_then(ShellArgs::command);
        let target = edit
            .as_ref()
            .and_then(|edit| edit.path.clone())
            .or(target)
            .or_else(|| command.map(str::to_owned));
        let console = if matches!(tool, "shell" | "bash") {
            command.map(|command| {
                Box::new(terminalui::components::console::Console::new(command).with_max_visible(6))
            })
        } else {
            None
        };
        stream.push(ui::Item::Tool {
            call_id: call_id.to_owned(),
            name: tool.to_owned(),
            target,
            state: ui::ToolState::Running,
            meta: origin.map(|origin| format!("plugin: {}", origin.plugin)),
            diff: edit.as_ref().and_then(|edit| Diff::from_call(source, edit)),
            console,
        });
    }
}

fn fold_delta(delta: &norte_code_wire::Delta, tree: &mut terminalui::Tree, model: &mut UiModel) {
    match delta {
        norte_code_wire::Delta::Text { text, .. } => {
            model.turn = TurnState::Streaming;
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.stop_thinking();
                stream.push_assistant_text(text, model.active_turn.as_ref());
            }
        }
        norte_code_wire::Delta::Reasoning { text, .. } => {
            model.turn = TurnState::Thinking;
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.push_reasoning_text(text);
            }
        }
        norte_code_wire::Delta::Refusal { text, .. } => {
            notice(tree, format!("refusal · {text}"), ui::Tone::Warn);
        }
        _ => {}
    }
}

fn finish_turn(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
) {
    model.turn = TurnState::Idle;
    model.active_turn = None;
    model.question = None;
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.stop_thinking();
        stream.complete_running_tools();
    }
    if let Some(commands) = commands {
        let _ = commands.try_send(UiCommand::RefreshRail);
    }
}
