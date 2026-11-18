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
use crate::ui_model::{SessionAttachmentState, Tab, TurnState, UiModel};

use norte_code_wire::ServerEventKind;

use crate::ui::Inspector;

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
    let mut views_dirty = true;

    loop {
        if drain_server_events(&mut events, &mut tree, &mut model) {
            views_dirty = true;
        }
        if views_dirty {
            ui::refresh_views(&mut tree, &model);
            ui::sync_views(&mut tree, &model);
            views_dirty = false;
        }
        sync_chrome(&mut tree, &model);

        let theme = theme::theme(model.preferences.theme);
        session.terminal().draw(|frame| {
            let area = frame.area();
            frame
                .buffer_mut()
                .set_style(area, Style::default().bg(theme.background));
            tree.render(area, frame.buffer_mut(), &theme);
        })?;

        if poll_event(
            &mut tree,
            &mut commands,
            &mut events,
            &mut model,
            config.as_ref(),
        )? {
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
            let mut pass_to_tree = true;
            match (key.modifiers, key.code) {
                (KeyModifiers::CONTROL, KeyCode::Char('c')) => return Ok(true),
                (KeyModifiers::CONTROL, KeyCode::Tab) => {
                    model.tab = model.tab.next();
                    ui::sync_views(tree, model);
                    pass_to_tree = false;
                }
                (KeyModifiers::CONTROL, KeyCode::BackTab) => {
                    model.tab = model.tab.previous();
                    ui::sync_views(tree, model);
                    pass_to_tree = false;
                }
                (KeyModifiers::NONE, KeyCode::PageUp) => {
                    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                        stream.scroll(-6);
                    }
                    pass_to_tree = false;
                }
                (KeyModifiers::NONE, KeyCode::PageDown) => {
                    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                        stream.scroll(6);
                    }
                    pass_to_tree = false;
                }
                (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                    if let SessionAttachmentState::Failed { .. } = model.attachment {
                        if let Some(cfg) = config {
                            let (commands_us, events_rx) = start(cfg.clone());
                            *commands = Some(commands_us);
                            *events = Some(events_rx);
                            model.attachment = SessionAttachmentState::Connecting;
                        }
                    } else if let Some(commands) = commands.as_ref() {
                        let _ = commands.try_send(UiCommand::Refresh);
                        let _ = commands.try_send(UiCommand::RefreshViews);
                    }
                    pass_to_tree = false;
                }
                (KeyModifiers::CONTROL, KeyCode::Char('t')) => {
                    model.preferences.theme = model.preferences.theme.next();
                    pass_to_tree = false;
                }
                (KeyModifiers::CONTROL, KeyCode::Enter) => {
                    send_current_prompt(tree, commands, model);
                    pass_to_tree = false;
                }
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    let prompt_open = tree
                        .find::<ui::KeyPrompt>("key_prompt")
                        .is_some_and(ui::KeyPrompt::is_open);
                    if !prompt_open && model.tab != Tab::Chat {
                        handle_enter(tree, commands, model);
                        pass_to_tree = false;
                    }
                }
                (KeyModifiers::NONE, KeyCode::Char(digit)) => {
                    if let Some(tab) = Tab::from_digit(digit) {
                        model.tab = tab;
                        ui::sync_views(tree, model);
                        pass_to_tree = false;
                    }
                }
                _ => {}
            }
            if pass_to_tree {
                let input = terminalui::InputEvent::from(Event::Key(key));
                let _ = tree.handle_event(&input);
            }
            sync_selectors(tree, commands, model);
            ui::sync_views(tree, model);
            Ok(false)
        }
        Event::Mouse(mouse) => {
            let input = terminalui::InputEvent::from(Event::Mouse(mouse));
            let _ = tree.handle_event(&input);
            sync_selectors(tree, commands, model);
            ui::sync_views(tree, model);
            Ok(false)
        }
        other => {
            let input = terminalui::InputEvent::from(other);
            let _ = tree.handle_event(&input);
            sync_selectors(tree, commands, model);
            ui::sync_views(tree, model);
            Ok(false)
        }
    }
}

/// After the tree dispatched, persist any selector cycle into the model
/// and notify the agent of changes the server understands.
fn sync_selectors(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    if let Some(selector) = tree.find_mut::<crate::ui::Selector>("reasoning") {
        let next = match selector.value() {
            "low" => crate::ui_model::ReasoningLevel::Low,
            "medium" => crate::ui_model::ReasoningLevel::Medium,
            "high" => crate::ui_model::ReasoningLevel::High,
            _ => crate::ui_model::ReasoningLevel::Auto,
        };
        if next != model.preferences.reasoning {
            model.preferences.reasoning = next;
            if let (Some(cmd), Some(session)) = (commands.as_ref(), model.active.clone()) {
                let effort = selector.value().to_string();
                let _ = cmd.try_send(UiCommand::SetReasoning { session, effort });
            }
        }
    }
    if let Some(selector) = tree.find_mut::<crate::ui::Selector>("queue") {
        model.preferences.send_mode = match selector.value() {
            "queue" => crate::ui_model::SendMode::Queue,
            "interrupt" => crate::ui_model::SendMode::Interrupt,
            "reject" => crate::ui_model::SendMode::Reject,
            _ => crate::ui_model::SendMode::Steer,
        };
    }
}

fn send_current_prompt(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    let prompt = tree
        .find::<ui::Composer>("composer")
        .map(|composer| composer.text().trim().to_string())
        .unwrap_or_default();
    if prompt.is_empty() {
        return;
    }
    let SessionAttachmentState::Attached = model.attachment else {
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            stream.push(ui::Item::Notice {
                text: "raep not attached; message not sent (Ctrl+R retries)".into(),
                tone: ui::Tone::Warn,
            });
        }
        return;
    };
    let Some(session) = model.active.clone() else {
        return;
    };
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::User {
            text: prompt.clone(),
        });
        stream.start_thinking();
    }
    if let Some(commands) = commands.as_ref() {
        let queue_mode = match model.preferences.send_mode {
            crate::ui_model::SendMode::Steer => "steer",
            crate::ui_model::SendMode::Queue => "queue",
            crate::ui_model::SendMode::Interrupt => "interrupt",
            crate::ui_model::SendMode::Reject => "reject",
        };
        let _ = commands.try_send(UiCommand::Send {
            session,
            text: prompt,
            queue_mode: queue_mode.to_string(),
        });
    }
    if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
        composer.clear();
    }
}

fn handle_enter(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    if model.tab == Tab::Models {
        handle_models_enter(tree, commands, model);
    }
}

fn handle_models_enter(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    let index = tree
        .find::<Inspector>("models")
        .and_then(Inspector::selected);
    let Some(index) = index else { return };
    let Some(model_info) = model.models.get(index) else {
        return;
    };
    let provider_id = model_info.provider.clone();
    let model_id = model_info.model.clone();

    // Does this provider need a key, and is one already on disk/in the environment?
    let provider_row = model.catalog.as_ref().and_then(|catalog| {
        catalog
            .providers
            .iter()
            .find(|p| p.provider_id == provider_id)
    });
    let env_keys = provider_row
        .map(|provider| provider.environment_keys.clone())
        .unwrap_or_default();

    if ui::provider_has_key(&provider_id, &env_keys) {
        // Key exists: switch this session to the model.
        if let (Some(commands), Some(session)) = (commands.as_ref(), model.active.clone()) {
            let _ = commands.try_send(UiCommand::SetModel {
                session,
                provider: provider_id.clone(),
                model: model_id.clone(),
            });
        }
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            stream.push(ui::Item::Notice {
                text: format!("model set · {provider_id} · {model_id}"),
                tone: ui::Tone::Neutral,
            });
        }
    } else {
        // No key: open the masked prompt so the user can save one.
        if let Some(prompt) = tree.find_mut::<ui::KeyPrompt>("key_prompt") {
            prompt.open(provider_id.clone(), env_keys);
        }
    }
}

fn drain_server_events(
    events: &mut Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) -> bool {
    let Some(receiver) = events else {
        return false;
    };
    let mut changed = false;
    loop {
        match receiver.try_recv() {
            Ok(ClientEvent::Attached { session }) => {
                let title = session
                    .title
                    .clone()
                    .unwrap_or_else(|| session.session_id.clone());
                model.attachment = SessionAttachmentState::Attached;
                model.active = Some(norte_code_wire::SessionKey::new(session.session_id));
                if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                    stream.push(ui::Item::Notice {
                        text: format!("attached to {title}"),
                        tone: ui::Tone::Neutral,
                    });
                }
                changed = true;
            }
            Ok(ClientEvent::Detached { reason }) => {
                model.attachment = SessionAttachmentState::Failed {
                    reason: reason.clone(),
                };
                model.turn = TurnState::Idle;
                if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                    stream.stop_thinking();
                    stream.push(ui::Item::Notice {
                        text: reason,
                        tone: ui::Tone::Error,
                    });
                }
                changed = true;
            }
            Ok(ClientEvent::Sessions(sessions)) => {
                model.sessions = sessions;
                changed = true;
            }
            Ok(ClientEvent::Models(models)) => {
                model.models = models;
                changed = true;
            }
            Ok(ClientEvent::Plans(plans)) => {
                model.plans = plans;
                changed = true;
            }
            Ok(ClientEvent::Jobs(jobs)) => {
                model.jobs = jobs;
                changed = true;
            }
            Ok(ClientEvent::Telemetry(report)) => {
                model.telemetry = Some(report);
                changed = true;
            }
            Ok(ClientEvent::TelemetrySession(report)) => {
                model.telemetry_session = Some(report);
                changed = true;
            }
            Ok(ClientEvent::CostSession(cost)) => {
                model.cost = Some(cost);
                changed = true;
            }
            Ok(ClientEvent::CostGlobal(cost)) => {
                // Aggregate: the fleet cost is the sum of session reports when available;
                // for now keep the global total under the session field so the page shows it.
                model.cost = Some(cost);
                changed = true;
            }
            Ok(ClientEvent::Plugins(plugins)) => {
                model.plugins = plugins;
                changed = true;
            }
            Ok(ClientEvent::Catalog(catalog)) => {
                model.catalog = Some(catalog);
                changed = true;
            }
            Ok(ClientEvent::Agents(agents)) => {
                model.agents = agents;
                changed = true;
            }
            Ok(ClientEvent::Server(event)) => fold(&event, tree, model),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return changed,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                *events = None;
                return changed;
            }
        }
    }
}

fn fold(event: &norte_code_wire::ServerEvent, tree: &mut terminalui::Tree, model: &mut UiModel) {
    match &event.kind {
        ServerEventKind::TurnStarted { .. } | ServerEventKind::InputAccepted { .. } => {
            model.turn = TurnState::Connecting;
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.start_thinking();
            }
        }
        ServerEventKind::TurnPhase { phase, .. } => {
            model.turn = match phase {
                norte_code_wire::TurnPhaseName::AwaitingApproval => TurnState::AwaitingAnswer,
                norte_code_wire::TurnPhaseName::ExecutingTool => TurnState::Streaming,
                _ => TurnState::Connecting,
            };
        }
        ServerEventKind::ToolStarted {
            tool,
            call_id,
            arguments,
            ..
        } => fold_tool_started(tool, call_id, arguments.as_ref(), tree, model),
        ServerEventKind::ToolOutput { call_id, text, .. } => {
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.append_console_line(call_id, text.clone());
            }
        }
        ServerEventKind::ToolFinished { call_id, .. } => {
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.finish_tool(call_id);
            }
        }
        ServerEventKind::Delta { delta, .. } => fold_delta(delta, tree, model),
        ServerEventKind::TurnFinished { .. } | ServerEventKind::TurnStopped { .. } => {
            model.turn = TurnState::Idle;
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.stop_thinking();
                stream.complete_running_tools();
            }
        }
        _ => {}
    }
}

fn fold_tool_started(
    tool: &str,
    call_id: &str,
    arguments: Option<&serde_json::Value>,
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
        let old_string = arguments.and_then(|v| {
            v.get("old_string")
                .or_else(|| v.get("old"))
                .and_then(|s| s.as_str())
                .map(str::to_owned)
        });
        let new_string = arguments.and_then(|v| {
            v.get("new_string")
                .or_else(|| v.get("new"))
                .and_then(|s| s.as_str())
                .map(str::to_owned)
        });
        if let (Some(removed), Some(added)) = (old_string, new_string) {
            stream.push(ui::Item::Diff {
                removed: removed.lines().map(str::to_string).collect(),
                added: added.lines().map(str::to_string).collect(),
            });
        }
        if matches!(tool, "shell" | "bash")
            && let Some(command) = arguments.and_then(|v| v.get("command").and_then(|s| s.as_str()))
        {
            stream.push(ui::Item::Shell {
                call_id: call_id.to_owned(),
                console: terminalui::components::console::Console::new(command).with_max_visible(6),
            });
        }
        stream.push(ui::Item::Tool {
            call_id: call_id.to_owned(),
            name: tool.to_owned(),
            target,
            state: ui::ToolState::Running,
            meta: None,
        });
    }
}

fn fold_delta(delta: &norte_code_wire::Delta, tree: &mut terminalui::Tree, model: &mut UiModel) {
    match delta {
        norte_code_wire::Delta::Text { text, .. } => {
            model.turn = TurnState::Streaming;
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.stop_thinking();
                stream.push_assistant_text(text);
            }
        }
        norte_code_wire::Delta::Reasoning { text, .. } => {
            model.turn = TurnState::Thinking;
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.push_reasoning_text(text);
            }
        }
        norte_code_wire::Delta::Refusal { text, .. } => {
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.push(ui::Item::Notice {
                    text: format!("refusal · {text}"),
                    tone: ui::Tone::Warn,
                });
            }
        }
        _ => {}
    }
}

fn sync_chrome(tree: &mut terminalui::Tree, model: &UiModel) {
    if let Some(header) = tree.find_mut::<ui::Header>("header") {
        header.turn = model.turn;
        header.theme_name = model.preferences.theme.name().to_string();
        header.tab = model.tab;
    }
    if let Some(rail) = tree.find_mut::<ui::Rail>("rail") {
        rail.attachment = model.attachment.clone();
        rail.set_session(model.active.as_ref().map(|key| key.as_str().to_string()));
    }
}
