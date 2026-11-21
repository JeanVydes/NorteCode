//! The run loop: terminal, events (input and server), view tree, frames.
//!
//! Each iteration drains the server's events into the model, folds the
//! user's input into the tree, paints, then polls for the next input.
//! Views never call back into the client; the app mediates between them.

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::style::Style;
use terminalui::components::button::Button;

use crate::client::{ClientEvent, UiCommand, start};
use crate::compact_notice;
use crate::config::ClientConfig;
use crate::context::ContextUsage;
use crate::diff::{Diff, EditArgs};
use crate::shell::ShellArgs;
use crate::terminal::TerminalSession;
use crate::theme::{CustomTheme, ThemeCatalog};
use crate::ui;
use crate::ui_model::{SessionAttachmentState, Tab, TurnState, UiModel};

use norte_code_wire::ServerEventKind;

const POLL_MS: u64 = 16;

/// Entry point. `run` renders the north star until the user quits.
///
/// # Errors
/// Returns the error the loop reported.
pub(super) fn run() -> io::Result<()> {
    let mut session = TerminalSession::enter()?;
    crate::terminal::watch_exit_signals();

    let mut model = UiModel::detached();
    let mut config: Option<ClientConfig> = None;
    let mut commands = None;
    let mut events = match ClientConfig::load() {
        Ok(loaded) => {
            model.preferences.catalog = ThemeCatalog {
                custom: loaded
                    .ui
                    .themes
                    .iter()
                    .cloned()
                    .map(CustomTheme::from_config)
                    .collect(),
            };
            model.preferences.theme = model.preferences.catalog.choose(loaded.ui.theme.as_deref());
            model.preferences.rail_position = loaded.ui.rail_position.unwrap_or_default();
            model.service_url.clone_from(&loaded.service.url);
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
        if crate::terminal::SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if drain_server_events(&mut events, &mut commands, &mut tree, &mut model) {
            views_dirty = true;
        }
        if views_dirty {
            ui::refresh_views(&mut tree, &model);
            ui::sync_views(&mut tree, &model);
            views_dirty = false;
        }
        sync_chrome(&mut tree, &model);

        let theme = model.preferences.catalog.theme(&model.preferences.theme);
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
            Ok(handle_key(tree, key, commands, events, model, config))
        }
        Event::Mouse(mouse) => {
            let input = terminalui::InputEvent::from(Event::Mouse(mouse));
            let _ = tree.handle_event(&input);
            drain_compact(tree, commands, model);
            sync_selectors(tree, commands, model);
            drain_tab_request(tree, model);
            drain_models_actions(tree, commands, model);
            drain_plugins_actions(tree, commands);
            drain_message_actions(tree, commands, model);
            drain_plan_requests(tree, model);
            if drain_config_actions(tree, model) {
                ui::refresh_views(tree, model);
            }
            drain_sessions_actions(tree, commands, model);
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

fn drain_plan_requests(tree: &mut terminalui::Tree, model: &UiModel) {
    let Some(id) = tree
        .find_mut::<ui::Rail>("rail")
        .and_then(ui::Rail::take_plan_request)
    else {
        return;
    };
    if let Some(plan) = model.plans.iter().find(|plan| plan.id == id).cloned()
        && let Some(overlay) = tree.find_mut::<ui::PlanOverlay>("plan_overlay")
    {
        overlay.open(plan);
    }
}

fn drain_message_actions(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &UiModel,
) {
    use crate::ui::MessageAction;
    let Some(stream) = tree.find_mut::<ui::Transcript>("stream") else {
        return;
    };
    let actions = stream.take_message_actions();
    let Some(session) = model.active.clone() else {
        return;
    };
    for action in actions {
        match action {
            MessageAction::Copy { text } => {
                let _ = terminalui::osc52_write(&text);
            }
            MessageAction::Fork { turn } => {
                if let Some(commands) = commands.as_ref() {
                    let _ = commands.try_send(UiCommand::Fork {
                        session: session.clone(),
                        turn,
                    });
                }
            }
            MessageAction::Rewind { turn } => {
                if let Some(commands) = commands.as_ref() {
                    let _ = commands.try_send(UiCommand::Rewind {
                        session: session.clone(),
                        turn,
                    });
                }
            }
        }
    }
}

fn ctrl_c(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) -> bool {
    if model.turn != TurnState::Idle {
        return cancel_active_turn(tree, commands, model);
    }
    true
}

fn cycle_theme(tree: &mut terminalui::Tree, model: &mut UiModel) {
    let choice = model.preferences.theme.clone();
    model.preferences.theme = model.preferences.catalog.next(&choice);
    ui::refresh_views(tree, model);
}

fn toggle_list_focus(tree: &mut terminalui::Tree, tab: Tab) {
    // The tree would spend Tab moving focus between the charts and pagers;
    // here it switches the page between its two lists.
    match tab {
        Tab::Telemetry => {
            if let Some(page) = tree.find_mut::<ui::TelemetryPage>("telemetry") {
                page.toggle_focus();
            }
        }
        Tab::Settings => {
            if let Some(page) = tree.find_mut::<ui::SettingsPage>("settings") {
                page.toggle_focus();
            }
        }
        _ => {}
    }
}

/// Handle one key press; returns `true` when the loop quits.
fn handle_key(
    tree: &mut terminalui::Tree,
    key: crossterm::event::KeyEvent,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    events: &mut Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
    model: &mut UiModel,
    config: Option<&ClientConfig>,
) -> bool {
    let mut pass_to_tree = true;
    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            return ctrl_c(tree, commands, model);
        }
        (KeyModifiers::NONE, KeyCode::Tab) if model.tab.is_list_tab() => {
            toggle_list_focus(tree, model.tab);
            pass_to_tree = false;
        }
        (KeyModifiers::CONTROL, KeyCode::Tab) => {
            model.tab = model.tab.next();
            ui::sync_views(tree, model);
            pass_to_tree = false;
        }
        (KeyModifiers::CONTROL | KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::BackTab) => {
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
            cycle_theme(tree, model);
            pass_to_tree = false;
        }
        (KeyModifiers::NONE | KeyModifiers::CONTROL, KeyCode::Enter) => {
            // Plain Enter sends from Chat; Shift+Enter is left to the tree
            // and lands as a newline in the composer.
            let question_open = tree
                .find::<ui::QuestionPrompt>("question_prompt")
                .is_some_and(ui::QuestionPrompt::is_open);
            let prompt_open = tree
                .find::<ui::KeyPrompt>("key_prompt")
                .is_some_and(ui::KeyPrompt::is_open);
            if question_open {
                // Enter goes to the ask-user overlay.
            } else if prompt_open {
                // Enter belongs to the prompt (save).
            } else if model.tab == Tab::Chat {
                send_current_prompt(tree, commands, model);
                pass_to_tree = false;
            }
            // Every other tab lets Enter fall through to the tree, which
            // routes it to the page's own handler (models, sessions, ...).
        }
        (KeyModifiers::CONTROL | KeyModifiers::ALT, KeyCode::Char(digit)) => {
            if let Some(tab) = Tab::from_digit(digit) {
                model.tab = tab;
                ui::sync_views(tree, model);
                pass_to_tree = false;
            }
        }
        (KeyModifiers::NONE, KeyCode::Char(digit))
            if model.tab != Tab::Chat && model.tab != Tab::Models =>
        {
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
    drain_compact(tree, commands, model);
    drain_question_answer(tree, commands, model);
    drain_models_actions(tree, commands, model);
    drain_sessions_actions(tree, commands, model);
    drain_plugins_actions(tree, commands);
    drain_message_actions(tree, commands, model);
    drain_plan_requests(tree, model);
    if drain_config_actions(tree, model) {
        ui::refresh_views(tree, model);
    }
    sync_selectors(tree, commands, model);
    drain_tab_request(tree, model);
    ui::sync_views(tree, model);
    false
}

/// Ship the answer the ask-user overlay just committed, if any.
fn drain_question_answer(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    let Some(prompt) = tree.find_mut::<ui::QuestionPrompt>("question_prompt") else {
        return;
    };
    let Some((question_id, answer)) = prompt.take_pending_answer() else {
        return;
    };
    if let (Some(commands), Some(session)) = (commands.as_ref(), model.active.clone()) {
        let _ = commands.try_send(UiCommand::AnswerQuestion {
            session,
            question: question_id,
            answer,
        });
    }
    model.question = None;
    ui::sync_views(tree, model);
}

/// Apply a tab the header reported as clicked.
fn drain_tab_request(tree: &mut terminalui::Tree, model: &mut UiModel) {
    if let Some(header) = tree.find_mut::<ui::Header>("header")
        && let Some(tab) = header.take_requested_tab()
    {
        model.tab = tab;
        ui::sync_views(tree, model);
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
        selector.select(model.preferences.reasoning.label());
    }
    if let Some(selector) = tree.find_mut::<crate::ui::Selector>("queue") {
        selector.select(model.preferences.send_mode.label());
    }
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
    if !model.active_session_has_model() {
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            stream.push(ui::Item::Notice {
                text: "no model selected; message not sent (3 Models, Enter to choose)".into(),
                tone: ui::Tone::Warn,
            });
        }
        return;
    }
    let Some(session) = model.active.clone() else {
        return;
    };
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::User {
            text: prompt.clone(),
            turn: model.active_turn.as_ref().and_then(|turn| {
                turn.id
                    .strip_prefix("turn_")
                    .unwrap_or(&turn.id)
                    .parse()
                    .ok()
            }),
        });
        stream.start_thinking();
    }
    if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
        composer.push_history(&prompt);
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

fn key_intent_for(provider: String, model_id: Option<String>) -> crate::ui::KeyIntent {
    match model_id {
        Some(model) => crate::ui::KeyIntent::SwitchModel { provider, model },
        None => crate::ui::KeyIntent::Expand { provider },
    }
}

/// Ask the host to switch the attached session's model.
fn set_model(
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &UiModel,
    provider: &str,
    model_id: &str,
) {
    if let (Some(commands), Some(session)) = (commands.as_ref(), model.active.clone()) {
        let _ = commands.try_send(UiCommand::SetModel {
            session,
            provider: provider.to_owned(),
            model: model_id.to_owned(),
        });
    }
}

/// The environment keys the provider names in the catalogue.
fn env_keys(model: &UiModel, provider: &str) -> Vec<String> {
    model
        .catalog
        .as_ref()
        .and_then(|catalog| {
            catalog
                .providers
                .iter()
                .find(|p| p.provider_id == provider)
                .map(|p| p.environment_keys.clone())
        })
        .unwrap_or_default()
}

/// Ask for the provider's key, remembering what to do once saved.
fn open_key_prompt(
    tree: &mut terminalui::Tree,
    provider: &str,
    model_id: Option<String>,
    env_keys: Vec<String>,
) {
    if let Some(prompt) = tree.find_mut::<ui::KeyPrompt>("key_prompt") {
        prompt.open_with_intent(
            provider.to_owned(),
            env_keys,
            key_intent_for(provider.to_owned(), model_id),
        );
    }
}

/// Drain the sessions page's pending actions into the client.
fn drain_sessions_actions(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    let Some(page) = tree.find_mut::<ui::SessionsPage>("sessions") else {
        return;
    };
    for action in page.take_pending() {
        match action {
            ui::SessionsAction::Attach { session } => {
                if let Some(commands) = commands.as_ref() {
                    let _ = commands.try_send(UiCommand::OpenSession { session });
                }
                model.tab = Tab::Chat;
                ui::sync_views(tree, model);
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

/// Ask the host to cancel the running turn; keeps the TUI up.
fn cancel_active_turn(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) -> bool {
    let Some(active) = model.active_turn.take() else {
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            stream.push(ui::Item::Notice {
                text: "a turn seems to be running but its id is unknown; cannot cancel".to_string(),
                tone: ui::Tone::Warn,
            });
        }
        model.turn = TurnState::Idle;
        return false;
    };
    let Some(commands) = commands.as_ref() else {
        model.active_turn = Some(active);
        return false;
    };
    let Some(session) = model.active.clone() else {
        model.active_turn = Some(active);
        return false;
    };
    let _ = commands.try_send(UiCommand::CancelTurn {
        session,
        turn_id: active.id,
    });
    model.turn = TurnState::Idle;
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.stop_thinking();
        stream.push(ui::Item::Notice {
            text: "turn cancelled".to_string(),
            tone: ui::Tone::Neutral,
        });
    }
    false
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
fn drain_plugins_actions(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
) {
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
fn drain_config_actions(tree: &mut terminalui::Tree, model: &mut UiModel) -> bool {
    use crate::ui::{ConfigAction, Setting};

    let actions = tree
        .find_mut::<ui::SettingsPage>("settings")
        .map_or_else(Vec::new, ui::SettingsPage::take_config_actions);
    let mut changed = false;
    for action in actions {
        changed = true;
        let ConfigAction::Cycle(setting) = action;
        match setting {
            Setting::Theme => {
                let choice = model.preferences.theme.clone();
                model.preferences.theme = model.preferences.catalog.next(&choice);
            }
            Setting::Thinking => model.preferences.reasoning = model.preferences.reasoning.next(),
            Setting::SendMode => model.preferences.send_mode = model.preferences.send_mode.next(),
            Setting::DiffMode => model.preferences.diff_mode = model.preferences.diff_mode.next(),
            Setting::RailPosition => {
                model.preferences.rail_position = model.preferences.rail_position.next();
            }
        }
    }
    changed
}

/// Drain the models page's pending actions into the client.
fn drain_models_actions(
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
                let keys = env_keys(model, &provider);
                open_key_prompt(tree, &provider, Some(model_id), keys);
            }
            ui::ModelsAction::AskKeyForExpand { provider } => {
                let keys = env_keys(model, &provider);
                open_key_prompt(tree, &provider, None, keys);
            }
            ui::ModelsAction::ToggleExpand { provider } => {
                if let Some(page) = tree.find_mut::<ui::ModelsPage>("models") {
                    page.toggle_expansion(&provider);
                }
            }
        }
    }
    if let Some(prompt) = tree.find_mut::<ui::KeyPrompt>("key_prompt")
        && let Some(intent) = prompt.take_saved()
    {
        match intent {
            crate::ui::KeyIntent::SwitchModel {
                provider,
                model: model_id,
            } => {
                set_model(commands, model, &provider, &model_id);
            }
            crate::ui::KeyIntent::Expand { provider } => {
                if let Some(page) = tree.find_mut::<ui::ModelsPage>("models") {
                    page.expand(&provider);
                }
            }
        }
    }
}

/// Track the host-reported model of the attached session from a list refresh.
fn refresh_active_model(model: &mut UiModel, sessions: &[norte_code_wire::SessionSummary]) {
    if let Some(active) = model.active.as_ref()
        && let Some(found) = sessions
            .iter()
            .find(|candidate| candidate.session_id == active.as_str())
    {
        model.active_model = found.model.clone();
        model.active_provider = found.provider.clone();
    }
}

fn apply_attached(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    session: &norte_code_wire::SessionSummary,
) -> bool {
    let title = session
        .title
        .clone()
        .unwrap_or_else(|| session.session_id.clone());
    let changed_session = model
        .active
        .as_ref()
        .is_none_or(|active| active.as_str() != session.session_id);
    model.attachment = SessionAttachmentState::Attached;
    model.active_model.clone_from(&session.model);
    model.active_provider.clone_from(&session.provider);
    model.active = Some(norte_code_wire::SessionKey::new(session.session_id.clone()));
    model.attached_title = Some(title.clone());
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream")
        && changed_session
    {
        // Never show another session's content under this label.
        stream.clear_all();
    }
    true
}

fn apply_detached(tree: &mut terminalui::Tree, model: &mut UiModel, reason: String) -> bool {
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
    true
}

fn fold_catalog_event(event: &ClientEvent, model: &mut UiModel) -> bool {
    match event {
        ClientEvent::Models(models) => model.models.clone_from(models),
        ClientEvent::Plans(plans) => model.plans.clone_from(plans),
        ClientEvent::Jobs(jobs) => model.jobs.clone_from(jobs),
        ClientEvent::Telemetry(report) => model.telemetry = Some(report.clone()),
        ClientEvent::TelemetrySession(report) => model.telemetry_session = Some(report.clone()),
        ClientEvent::CostSession(cost) | ClientEvent::CostGlobal(cost) => {
            model.cost = Some(cost.clone());
        }
        ClientEvent::Plugins(plugins) => model.plugins.clone_from(plugins),
        ClientEvent::Catalog(catalog) => model.catalog = Some(catalog.clone()),
        ClientEvent::Agents(agents) => model.agents.clone_from(agents),
        _ => return false,
    }
    true
}

fn fold_host_event(event: &ClientEvent, model: &mut UiModel) {
    match event {
        ClientEvent::ServerInfo(info) => model.server_info = Some(info.clone()),
        ClientEvent::Liveness(live) => model.liveness = Some(live.clone()),
        ClientEvent::Readiness(ready) => model.readiness = Some(ready.clone()),
        ClientEvent::Quota(quota) => model.quota = Some(quota.clone()),
        _ => {}
    }
}

fn drain_server_events(
    events: &mut Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) -> bool {
    let Some(receiver) = events else {
        return false;
    };
    let mut changed = false;
    loop {
        let next_event = receiver.try_recv();
        match next_event {
            Ok(ClientEvent::Attached { session }) => {
                changed = apply_attached(tree, model, &session) || changed;
            }
            Ok(ClientEvent::Detached { reason }) => {
                changed = apply_detached(tree, model, reason) || changed;
            }
            Ok(ClientEvent::Sessions(sessions)) => {
                refresh_active_model(model, &sessions);
                model.sessions = sessions;
                changed = true;
            }
            Ok(
                event @ (ClientEvent::Models(_)
                | ClientEvent::Plans(_)
                | ClientEvent::Jobs(_)
                | ClientEvent::Telemetry(_)
                | ClientEvent::TelemetrySession(_)
                | ClientEvent::CostSession(_)
                | ClientEvent::CostGlobal(_)
                | ClientEvent::Plugins(_)
                | ClientEvent::Catalog(_)
                | ClientEvent::Agents(_)),
            ) => {
                changed = fold_catalog_event(&event, model) || changed;
            }
            Ok(
                event @ (ClientEvent::ServerInfo(_)
                | ClientEvent::Liveness(_)
                | ClientEvent::Readiness(_)
                | ClientEvent::Quota(_)),
            ) => {
                fold_host_event(&event, model);
                changed = true;
            }
            Ok(ClientEvent::Forked { session }) => {
                if let Some(commands) = commands.as_ref() {
                    let _ = commands.try_send(UiCommand::OpenSession { session });
                }
            }
            Ok(ClientEvent::SendFailed { reason }) => {
                if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                    stream.push(ui::Item::Notice {
                        text: reason,
                        tone: ui::Tone::Error,
                    });
                }
                changed = true;
            }
            Ok(ClientEvent::Notice { text }) => {
                if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                    stream.push(ui::Item::Notice {
                        text,
                        tone: ui::Tone::Neutral,
                    });
                }
                changed = true;
            }
            Ok(ClientEvent::History(items)) => {
                changed = fold_history_event(tree, model, items) || changed;
            }
            Ok(ClientEvent::HistoryFailed { reason }) => {
                changed = fold_history_failure(tree, model, &reason) || changed;
            }
            Ok(ClientEvent::Server(event)) => {
                fold(&event, tree, model);
                changed = true;
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return changed,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                *events = None;
                return changed;
            }
        }
    }
}

fn fold_job_event(kind: &norte_code_wire::ServerEventKind, tree: &mut terminalui::Tree) {
    match kind {
        norte_code_wire::ServerEventKind::JobStarted {
            job_id, command, ..
        } => {
            notice(
                tree,
                crate::job_notice::started(job_id, command),
                ui::Tone::Neutral,
            );
        }
        norte_code_wire::ServerEventKind::JobFinished { job, .. } => {
            let text = crate::job_notice::finished(job);
            let tone = crate::job_notice::tone(job);
            notice(tree, text, tone);
        }
        _ => {}
    }
}

fn set_compact_loading(tree: &mut terminalui::Tree, loading: bool) {
    if let Some(button) = tree.find_mut::<Button>("compact") {
        button.set_loading(loading);
    }
}

fn fold_compaction_event(kind: &ServerEventKind, tree: &mut terminalui::Tree, model: &mut UiModel) {
    match kind {
        ServerEventKind::SessionStats {
            context_used,
            context_window,
            ..
        } => {
            model.context = ContextUsage {
                used: *context_used,
                window: *context_window,
            };
        }
        ServerEventKind::RequestReconstructed { usage, .. } => {
            let spent = usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0);
            if spent > 0 {
                model.context.used = spent;
            }
        }
        ServerEventKind::CompactionStarted { reason } => {
            notice(tree, compact_notice::started(*reason), ui::Tone::Neutral);
            set_compact_loading(tree, true);
        }
        ServerEventKind::CompactionFinished {
            compacted,
            summary,
            error,
            ..
        } => {
            let (text, tone) =
                compact_notice::finished(*compacted, summary.as_deref(), error.as_deref());
            notice(tree, text, tone);
            set_compact_loading(tree, false);
        }
        _ => {}
    }
}

fn fold(event: &norte_code_wire::ServerEvent, tree: &mut terminalui::Tree, model: &mut UiModel) {
    if fold_error_event(&event.kind, tree, model) {
        return;
    }
    match &event.kind {
        ServerEventKind::TurnStarted { .. } => {
            model.turn = TurnState::Connecting;
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.start_thinking();
            }
        }
        ServerEventKind::InputAccepted { turn_id, .. } => {
            model.turn = TurnState::Connecting;
            model.active_turn = Some(crate::ui_model::ActiveTurn {
                id: turn_id.clone(),
            });
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
            source,
            ..
        } => fold_tool_started(
            tool,
            call_id,
            arguments.as_ref(),
            source.as_deref(),
            tree,
            model,
        ),
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
        ServerEventKind::PluginProgress {
            operation,
            step,
            total,
            stage,
        } => {
            model.plugin_job = Some(crate::ui_model::PluginJob {
                operation: operation.clone(),
                step: *step,
                total: *total,
                stage: stage.clone(),
            });
        }
        ServerEventKind::PluginChanged { .. } => {
            model.plugin_job = None;
        }
        ServerEventKind::QuestionAnswered { .. } => {
            model.question = None;
            if model.turn == TurnState::AwaitingAnswer {
                model.turn = TurnState::Connecting;
            }
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.start_thinking();
                stream.push(ui::Item::Notice {
                    text: "question answered".to_string(),
                    tone: ui::Tone::Neutral,
                });
            }
        }
        ServerEventKind::TurnFinished { .. } | ServerEventKind::TurnStopped { .. } => {
            model.turn = TurnState::Idle;
            model.active_turn = None;
            model.question = None;
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.stop_thinking();
                stream.complete_running_tools();
            }
        }
        _ => {
            fold_job_event(&event.kind, tree);
            fold_compaction_event(&event.kind, tree, model);
        }
    }
}

/// Replay a session's history into the stream, replacing what is there.
fn fold_history_event(
    tree: &mut terminalui::Tree,
    model: &crate::ui_model::UiModel,
    items: Vec<norte_code_wire::HistoryItem>,
) -> bool {
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.replace_history(items);
        push_attach_notice(stream, model);
    }
    true
}

/// Report a history fetch that failed; an empty stream beats a stale one.
fn fold_history_failure(
    tree: &mut terminalui::Tree,
    model: &crate::ui_model::UiModel,
    reason: &str,
) -> bool {
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::Notice {
            text: format!("could not load the session's history · {reason}"),
            tone: ui::Tone::Warn,
        });
        push_attach_notice(stream, model);
    }
    true
}

/// The attach notice, pushed along with the replayed history.
fn push_attach_notice(stream: &mut ui::Transcript, model: &crate::ui_model::UiModel) {
    let Some(title) = model.attached_title.clone() else {
        return;
    };
    stream.push(ui::Item::Notice {
        text: format!("attached to {title}"),
        tone: ui::Tone::Neutral,
    });
    if !model.active_session_has_model() {
        stream.push(ui::Item::Notice {
            text: "no model selected; 3 Models, Enter to choose".into(),
            tone: ui::Tone::Warn,
        });
    }
}

/// Apply the host's confirmation that the session's model changed.
fn fold_model_changed(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    provider: &str,
    new_model: &str,
) -> bool {
    if let Some(active) = model.active.as_ref()
        && let Some(summary) = model
            .sessions
            .iter_mut()
            .find(|summary| summary.session_id == active.as_str())
    {
        summary.provider = Some(provider.to_owned());
        summary.model = Some(norte_code_wire::AgentRef {
            id: new_model.to_owned(),
            version: String::new(),
        });
    }
    model.active_provider = Some(provider.to_owned());
    model.active_model = Some(norte_code_wire::AgentRef {
        id: new_model.to_owned(),
        version: String::new(),
    });
    notice(
        tree,
        format!("model · {provider} · {new_model}"),
        ui::Tone::Neutral,
    )
}

/// Handle the failure-ish arms of the stream; `true` when one applied.
fn fold_error_event(
    kind: &ServerEventKind,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) -> bool {
    match kind {
        ServerEventKind::Error { message, .. }
            if message.starts_with("RAEP sequence gap")
                || message.contains("fell behind and dropped") =>
        {
            // The reader resyncs on the next frame; not a turn failure.
            notice(
                tree,
                "stream skipped events · resyncing".to_owned(),
                ui::Tone::Neutral,
            )
        }
        ServerEventKind::Error { message, .. } => stop_with_notice(
            tree,
            model,
            format!("host error · {message}"),
            ui::Tone::Error,
        ),
        ServerEventKind::BudgetExceeded {
            scope,
            spent,
            limit,
            unit,
            action,
        } => stop_with_notice(
            tree,
            model,
            format!("budget exceeded · {scope:?} · {spent:.2}/{limit:.2} {unit:?} · {action:?}"),
            ui::Tone::Error,
        ),
        ServerEventKind::BudgetWarning {
            scope,
            spent,
            limit,
            unit,
        } => notice(
            tree,
            format!("budget warning · {scope:?} · {spent:.2}/{limit:.2} {unit:?}"),
            ui::Tone::Warn,
        ),
        ServerEventKind::QuestionAsked {
            question,
            prompt,
            options,
            multiple,
            ..
        } => {
            model.turn = TurnState::AwaitingAnswer;
            model.question = Some(crate::ui_model::PendingQuestion {
                question: question.clone(),
                prompt: prompt.clone(),
                options: options.clone(),
                multiple: *multiple,
            });
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.stop_thinking();
                stream.push(ui::Item::Notice {
                    text: format!("the model asks · {prompt}"),
                    tone: ui::Tone::Warn,
                });
            }
            true
        }
        ServerEventKind::ModelQueued {
            provider,
            model: queued,
        } => notice(
            tree,
            format!("model queued · {provider} · {queued}"),
            ui::Tone::Neutral,
        ),
        ServerEventKind::ModelChanged {
            provider,
            model: new_model,
        } => fold_model_changed(tree, model, provider, new_model),
        ServerEventKind::RetryScheduled {
            attempt, reason, ..
        } => {
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.push_retry(format!("retry {attempt} · {reason}"), ui::Tone::Warn);
            }
            true
        }
        ServerEventKind::RetryStarted { attempt, .. } => {
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.push_retry(format!("retry {attempt} running"), ui::Tone::Neutral);
            }
            true
        }
        ServerEventKind::ReasoningDowngraded { from, to, reason } => notice(
            tree,
            format!("reasoning {from} → {to} · {reason}"),
            ui::Tone::Warn,
        ),
        _ => false,
    }
}

fn stop_with_notice(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    text: String,
    tone: ui::Tone,
) -> bool {
    model.turn = TurnState::Idle;
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.stop_thinking();
        stream.complete_running_tools();
        stream.push(ui::Item::Notice { text, tone });
    }
    true
}

fn notice(tree: &mut terminalui::Tree, text: String, tone: ui::Tone) -> bool {
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::Notice { text, tone });
    }
    true
}

fn drain_compact(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &UiModel,
) {
    let Some(button) = tree.find_mut::<Button>("compact") else {
        return;
    };
    if button.take_pressed()
        && let Some(session) = model.active.as_ref()
        && let Some(commands) = commands.as_ref()
    {
        let _ = commands.try_send(UiCommand::Compact {
            session: session.clone(),
        });
        button.set_loading(true);
    }
}

fn fold_tool_started(
    tool: &str,
    call_id: &str,
    arguments: Option<&serde_json::Value>,
    source: Option<&str>,
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
            meta: None,
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
        header.theme_name = crate::theme::theme_name(&model.preferences.theme).to_string();
        header.tab = model.tab;
    }
    if let Some(rail) = tree.find_mut::<ui::Rail>("rail") {
        rail.attachment = model.attachment.clone();
        rail.set_session(model.active.as_ref().map(|key| key.as_str().to_string()));
        rail.model = model.active_model.as_ref().map(|model| model.id.clone());
        rail.provider.clone_from(&model.active_provider);
        rail.set_context(model.effective_context());
        rail.set_jobs(model.jobs.clone());
        rail.set_plans(model.plans.clone());
        rail.set_compact_enabled(model.active.is_some());
    }
}
