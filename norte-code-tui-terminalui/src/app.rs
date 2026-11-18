//! The run loop: terminal, events (input and server), view tree, frames.
//!
//! Each iteration drains the server's events into the model, folds the
//! user's input into the tree, paints, then polls for the next input.
//! Views never call back into the client; the app mediates between them.

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::style::Style;

use crate::client::{ClientEvent, UiCommand, start};
use crate::config::ClientConfig;
use crate::terminal::TerminalSession;
use crate::theme;
use crate::ui;
use crate::ui_model::{SessionAttachmentState, TurnState, UiModel};

use norte_code_wire::ServerEventKind;

const POLL_MS: u64 = 16;

/// Entry point. `run` renders the north star until the user quits.
///
/// # Errors
/// Returns the error the loop reported.
pub(super) fn run() -> io::Result<()> {
    let mut session = TerminalSession::enter()?;

    let mut model = UiModel::detached();
    let mut config: Option<ClientConfig> = None;
    let mut commands = None;
    let mut events = match ClientConfig::load() {
        Ok(loaded) => {
            let (commands_us, events_rx) = start(loaded.clone());
            config = Some(loaded);
            commands = Some(commands_us);
            model.attachment = SessionAttachmentState::Connecting;
            Some(events_rx)
        }
        Err(error) => {
            model.attachment = SessionAttachmentState::Failed {
                reason: format!("{error}"),
            };
            None
        }
    };

    let mut tree = ui::build_tree(&model);

    loop {
        drain_server_events(&mut events, &mut tree, &mut model);
        sync_chrome(&mut tree, &model);

        let theme = theme::theme(model.preferences.theme);
        session.terminal().draw(|frame| {
            let area = frame.area();
            frame.buffer_mut().set_style(area, Style::default().bg(theme.background));
            tree.render(area, frame.buffer_mut(), &theme);
        })?;

        if poll_event(&mut tree, &mut commands, &mut events, &mut model, config.as_ref())? {
            break;
        }

        tree.tick(Duration::from_millis(POLL_MS));
    }

    session.leave();
    Ok(())
}

/// How input events are folded into the model; `true` when the loop quits.
fn poll_event(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    events: &mut Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
    model: &mut UiModel,
    config: Option<&ClientConfig>,
) -> io::Result<bool> {
    if !event::poll(Duration::from_millis(POLL_MS))? {
        return Ok(false);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            match (key.modifiers, key.code) {
                (KeyModifiers::CONTROL, KeyCode::Char('c')) => return Ok(true),
                (KeyModifiers::NONE, KeyCode::PageUp) | (KeyModifiers::NONE, KeyCode::PageDown) => {
                    let delta = if key.code == KeyCode::PageUp { -6 } else { 6 };
                    if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                        stream.scroll(delta);
                    }
                }
                (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                    if let SessionAttachmentState::Failed { .. } = model.attachment {
                        if let Some(cfg) = config {
                            let (commands_us, events_rx) = start(cfg.clone());
                            *commands = Some(commands_us);
                            *events = Some(events_rx);
                            model.attachment = SessionAttachmentState::Connecting;
                        }
                    }
                }
                (KeyModifiers::CONTROL, KeyCode::Char('t')) => {
                    model.preferences.theme = model.preferences.theme.next();
                }
                (KeyModifiers::CONTROL, KeyCode::Enter) => {
                    if let Some(commands) = commands.as_ref() {
                        send_current_prompt(tree, commands, model);
                    }
                }
                _ => {}
            }
            let input = terminalui::InputEvent::from(Event::Key(key));
            let _ = tree.handle_event(&input);
            Ok(false)
        }
        Event::Mouse(mouse) => {
            let input = terminalui::InputEvent::from(Event::Mouse(mouse));
            let _ = tree.handle_event(&input);
            Ok(false)
        }
        other => {
            let input = terminalui::InputEvent::from(other);
            let _ = tree.handle_event(&input);
            Ok(false)
        }
    }
}

fn send_current_prompt(
    tree: &mut terminalui::Tree,
    commands: &tokio::sync::mpsc::Sender<UiCommand>,
    model: &mut UiModel,
) {
    let prompt = tree
        .find::<crate::ui::Composer>("composer")
        .map(|composer| composer.text().trim().to_string())
        .unwrap_or_default();
    if prompt.is_empty() {
        return;
    }
    let SessionAttachmentState::Attached = model.attachment else {
        if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
            stream.push(crate::ui::Item::Notice {
                text: "raep not attached; message not sent (Ctrl+R retries)".into(),
                tone: crate::ui::Tone::Warn,
            });
        }
        return;
    };
    let Some(session) = model.active.clone() else {
        return;
    };
    if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
        stream.push(crate::ui::Item::User { text: prompt.clone() });
        stream.start_thinking();
    }
    let _ = commands.try_send(UiCommand::Send {
        session,
        text: prompt,
        queue_mode: "queue".into(),
    });
    if let Some(composer) = tree.find_mut::<crate::ui::Composer>("composer") {
        composer.clear();
    }
}

fn drain_server_events(
    events: &mut Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) {
    let Some(receiver) = events else {
        return;
    };
    loop {
        match receiver.try_recv() {
            Ok(ClientEvent::Attached { session }) => {
                let title = session.title.clone().unwrap_or_else(|| session.session_id.clone());
                model.attachment = SessionAttachmentState::Attached;
                model.active = Some(norte_code_wire::SessionKey::new(session.session_id));
                if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                    stream.push(crate::ui::Item::Notice {
                        text: format!("attached to {title}"),
                        tone: crate::ui::Tone::Neutral,
                    });
                }
            }
            Ok(ClientEvent::Detached { reason }) => {
                model.attachment = SessionAttachmentState::Failed { reason: reason.clone() };
                model.turn = TurnState::Idle;
                if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                    stream.stop_thinking();
                    stream.push(crate::ui::Item::Notice {
                        text: reason,
                        tone: crate::ui::Tone::Error,
                    });
                }
            }
            Ok(ClientEvent::Sessions(sessions)) => {
                model.sessions = sessions;
            }
            Ok(ClientEvent::Server(event)) => {
                if std::env::var_os("NORTE_TUI_DEBUG").is_some() {
                    let tag = serde_json::to_string(&event.kind).unwrap_or_else(|_| "<unprintable>".into());
                    eprintln!("[event] {}", tag.get(..160).unwrap_or(&tag));
                }
                fold(&event, tree, model)
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                *events = None;
                return;
            }
        }
    }
}

fn fold(event: &norte_code_wire::ServerEvent, tree: &mut terminalui::Tree, model: &mut UiModel) {
    match &event.kind {
        ServerEventKind::TurnStarted { .. } | ServerEventKind::TurnPhase { .. } => {
            if model.turn == TurnState::Idle {
                model.turn = TurnState::Thinking;
                if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                    stream.start_thinking();
                }
            }
        }
        ServerEventKind::ToolStarted {
            tool,
            call_id,
            arguments,
            ..
        } => {
            model.turn = TurnState::Streaming;
            if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                let target = arguments
                    .as_ref()
                    .and_then(|value| {
                        value.get("path").or_else(|| value.get("file")).and_then(|v| v.as_str()).map(str::to_owned)
                    });
                stream.push(crate::ui::Item::Tool {
                    call_id: call_id.clone(),
                    name: tool.clone(),
                    target,
                    state: crate::ui::ToolState::Running,
                    meta: None,
                });
            }
        }
        ServerEventKind::ToolFinished { call_id, .. } => {
            if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                stream.finish_tool(call_id);
            }
        }
        ServerEventKind::Delta { delta, .. } => match delta {
            norte_code_wire::Delta::Text { text, .. } => {
                model.turn = TurnState::Streaming;
                if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                    stream.push_assistant_text(text);
                }
            }
            norte_code_wire::Delta::Reasoning { text, .. } => {
                if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                    stream.push_reasoning_text(text);
                }
            }
            norte_code_wire::Delta::Refusal { text, .. } => {
                if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                    stream.push(crate::ui::Item::Notice {
                        text: format!("refusal · {text}"),
                        tone: crate::ui::Tone::Warn,
                    });
                }
            }
            _ => {}
        },
        ServerEventKind::TurnFinished { .. } | ServerEventKind::TurnStopped { .. } => {
            model.turn = TurnState::Idle;
            if let Some(stream) = tree.find_mut::<crate::ui::Transcript>("stream") {
                stream.stop_thinking();
                stream.complete_running_tools();
            }
        }
        _ => {}
    }
}

fn sync_chrome(tree: &mut terminalui::Tree, model: &UiModel) {
    if let Some(header) = tree.find_mut::<crate::ui::Header>("header") {
        header.turn = model.turn;
        header.theme_name = model.preferences.theme.name().to_string();
    }
    if let Some(rail) = tree.find_mut::<crate::ui::Rail>("rail") {
        rail.attachment = model.attachment.clone();
        rail.set_session(model.active.as_ref().map(|key| key.as_str().to_string()));
    }
}
