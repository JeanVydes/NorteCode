//! Key presses: shortcuts, prompts, and the composer send path.

use crossterm::event::{Event, KeyCode, KeyModifiers};

use crate::client::{ClientEvent, UiCommand, start};
use crate::config::ClientConfig;
use crate::ui;
use crate::ui_model::{SessionAttachmentState, Tab, TurnState, UiModel};

use super::super::event_fold::{notice, toast};
use super::palette::handle_palette_key;
use super::slash_actions::{
    execute_slash_command, handle_slash_suggestion_key, sync_slash_suggestions,
};
use super::{cycle_theme, digit_switches_tab, scroll_page, settle_input, toggle_list_focus};

fn models_page(tree: &terminalui::Tree) -> Option<&ui::ModelsPage> {
    tree.find::<ui::ModelsPage>("models")
}

fn models_page_mut(tree: &mut terminalui::Tree) -> Option<&mut ui::ModelsPage> {
    tree.find_mut::<ui::ModelsPage>("models")
}

fn switch_tab(tree: &mut terminalui::Tree, model: &mut UiModel, tab: Tab) {
    model.tab = tab;
    ui::sync_views(tree, model);
    if tree
        .find::<terminalui::Pane>("root")
        .is_none_or(|root| root.visible() == 0)
    {
        let target = match tab {
            Tab::Chat => "composer",
            Tab::Sessions => "sessions",
            Tab::Models => "models",
            Tab::Telemetry => "telemetry",
            Tab::Settings => "settings",
        };
        tree.focus(target);
    }
}

fn toggle_delete_choice(tree: &mut terminalui::Tree, model: &UiModel) -> bool {
    if model.tab != Tab::Models || !models_page(tree).is_some_and(ui::ModelsPage::confirming_delete)
    {
        return false;
    }
    if let Some(page) = models_page_mut(tree) {
        page.toggle_dialog_choice();
    }
    true
}

fn warn_about_unremovable_key(tree: &mut terminalui::Tree, model: &UiModel) -> bool {
    if model.tab != Tab::Models
        || models_page(tree).is_some_and(ui::ModelsPage::can_delete_selected_key)
    {
        return false;
    }
    toast(tree, "only saved keys can be removed", ui::Tone::Warn);
    true
}

fn overlay_owns_enter(tree: &terminalui::Tree) -> bool {
    let question_open = tree
        .find::<ui::QuestionPrompt>("question_prompt")
        .is_some_and(ui::QuestionPrompt::is_open);
    let prompt_open = tree
        .find::<ui::KeyPrompt>("key_prompt")
        .is_some_and(ui::KeyPrompt::is_open);
    let plan_open = tree
        .find::<ui::PlanOverlay>("plan_overlay")
        .is_some_and(ui::PlanOverlay::is_open);
    let host_open = tree
        .find::<ui::HostOverlay>("host_overlay")
        .is_some_and(ui::HostOverlay::is_open);
    question_open || prompt_open || plan_open || host_open
}

fn keep_chat_focus(
    tree: &mut terminalui::Tree,
    model: &UiModel,
    key: crossterm::event::KeyEvent,
) -> bool {
    if model.tab == Tab::Chat
        && !overlay_owns_enter(tree)
        && matches!(
            (key.modifiers, key.code),
            (KeyModifiers::NONE, KeyCode::Tab) | (KeyModifiers::SHIFT, KeyCode::BackTab)
        )
    {
        tree.focus("composer");
        true
    } else {
        false
    }
}

fn reconnect(
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    events: &mut Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
    model: &mut UiModel,
    config: Option<&ClientConfig>,
) {
    if let SessionAttachmentState::Failed { .. } = model.attachment {
        if let Some(cfg) = config {
            let (commands_tx, events_rx) = start(cfg.clone());
            *commands = Some(commands_tx);
            *events = Some(events_rx);
            model.attachment = SessionAttachmentState::Connecting;
        }
    } else if let Some(commands) = commands.as_ref() {
        let _ = commands.try_send(UiCommand::Refresh);
        let _ = commands.try_send(UiCommand::RefreshViews);
    }
}

/// Handle one key press; returns `true` when the loop quits.
pub(in crate::app) fn handle_key(
    tree: &mut terminalui::Tree,
    key: crossterm::event::KeyEvent,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    events: &mut Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
    model: &mut UiModel,
    config: Option<&ClientConfig>,
) -> bool {
    let mut pass_to_tree = true;
    if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('q') {
        return true;
    }
    if handle_palette_key(tree, key, commands, model) {
        return false;
    }
    if !overlay_owns_enter(tree) && handle_slash_suggestion_key(tree, key, model) {
        settle_input(tree, commands, model);
        return false;
    }
    if key.modifiers == KeyModifiers::CONTROL
        && matches!(key.code, KeyCode::Char('d') | KeyCode::Delete)
        && warn_about_unremovable_key(tree, model)
    {
        return false;
    }
    if key.modifiers == KeyModifiers::NONE
        && key.code == KeyCode::Tab
        && toggle_delete_choice(tree, model)
    {
        return false;
    }
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Tab) | (KeyModifiers::SHIFT, KeyCode::BackTab)
            if keep_chat_focus(tree, model, key) =>
        {
            return false;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            let copied = tree
                .find_mut::<ui::Composer>("composer")
                .is_some_and(ui::Composer::copy_selection);
            pass_to_tree = !copied;
        }
        (KeyModifiers::NONE, KeyCode::Tab) if model.tab.is_list_tab() => {
            toggle_list_focus(tree, model.tab);
            pass_to_tree = false;
        }
        (KeyModifiers::CONTROL, KeyCode::Tab) => {
            switch_tab(tree, model, model.tab.next());
            pass_to_tree = false;
        }
        (KeyModifiers::CONTROL | KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::BackTab) => {
            switch_tab(tree, model, model.tab.previous());
            pass_to_tree = false;
        }
        (KeyModifiers::NONE, KeyCode::PageUp) if !overlay_owns_enter(tree) => {
            scroll_page(tree, model.tab, -6);
            pass_to_tree = false;
        }
        (KeyModifiers::NONE, KeyCode::PageDown) if !overlay_owns_enter(tree) => {
            scroll_page(tree, model.tab, 6);
            pass_to_tree = false;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
            reconnect(commands, events, model, config);
            pass_to_tree = false;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('t')) => {
            cycle_theme(tree, model);
            pass_to_tree = false;
        }
        (KeyModifiers::NONE | KeyModifiers::CONTROL, KeyCode::Enter) => {
            // Plain Enter sends from Chat; Shift+Enter is left to the tree
            // and lands as a newline in the composer.
            if !overlay_owns_enter(tree) && model.tab == Tab::Chat {
                send_current_prompt(tree, commands, model);
                pass_to_tree = false;
            }
            // Other tabs let Enter fall through to the page's own handler.
        }
        (KeyModifiers::CONTROL | KeyModifiers::ALT, KeyCode::Char(digit)) => {
            if let Some(tab) = Tab::from_digit(digit) {
                switch_tab(tree, model, tab);
                pass_to_tree = false;
            }
        }
        (KeyModifiers::NONE, KeyCode::Char(digit)) if digit_switches_tab(tree, model) => {
            if let Some(tab) = Tab::from_digit(digit) {
                switch_tab(tree, model, tab);
                pass_to_tree = false;
            }
        }
        _ => {}
    }
    let cancel_on_escape = key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE;
    if pass_to_tree {
        let input = terminalui::InputEvent::from(Event::Key(key));
        let outcome = tree.handle_event(&input);
        if cancel_on_escape && outcome.is_ignored() && model.turn != TurnState::Idle {
            let _ = cancel_active_turn(tree, commands, model);
        }
    }
    settle_input(tree, commands, model);
    sync_slash_suggestions(tree, model);
    false
}

pub(in crate::app) fn drain_key_prompt(
    tree: &mut terminalui::Tree,
    commands: Option<&tokio::sync::mpsc::Sender<UiCommand>>,
) {
    let Some(action) = tree
        .find_mut::<ui::KeyPrompt>("key_prompt")
        .and_then(ui::KeyPrompt::take_action)
    else {
        return;
    };
    match action {
        ui::KeyPromptAction::Submit { provider, key } => {
            if let Some(commands) = commands {
                let _ = commands.try_send(UiCommand::SaveSecret { provider, key });
            }
        }
        ui::KeyPromptAction::Cancel => {}
    }
}

pub(in crate::app) fn apply_key_intent(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
    intent: ui::KeyIntent,
) {
    match intent {
        ui::KeyIntent::SwitchModel {
            provider,
            model: model_id,
        } => {
            set_model(tree, commands, model, &provider, &model_id);
        }
        ui::KeyIntent::Expand { provider } => {
            if let Some(page) = tree.find_mut::<ui::ModelsPage>("models") {
                page.expand(&provider);
            }
        }
    }
}

fn key_intent_for(provider: String, model_id: Option<String>) -> crate::ui::KeyIntent {
    match model_id {
        Some(model) => crate::ui::KeyIntent::SwitchModel { provider, model },
        None => crate::ui::KeyIntent::Expand { provider },
    }
}

/// Select a model for the active session or the next sessionless draft.
pub(in crate::app) fn set_model(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
    provider: &str,
    model_id: &str,
) {
    if let Some(session) = model.active.clone() {
        if let Some(commands) = commands.as_ref() {
            let _ = commands.try_send(UiCommand::SetModel {
                session,
                provider: provider.to_owned(),
                model: model_id.to_owned(),
            });
        }
    } else {
        model.pending_new_session_model = Some(crate::client::NewSessionModel {
            provider: provider.to_owned(),
            model: model_id.to_owned(),
        });
        ui::update_models(tree, model);
    }
}

/// Ask for the provider's key, remembering what to do once saved.
pub(in crate::app) fn open_key_prompt(
    tree: &mut terminalui::Tree,
    provider: &str,
    model_id: Option<String>,
    env_keys: &[String],
) {
    if let Some(prompt) = tree.find_mut::<ui::KeyPrompt>("key_prompt") {
        prompt.open_with_intent(
            provider.to_owned(),
            env_keys,
            key_intent_for(provider.to_owned(), model_id),
        );
    }
}

fn send_current_prompt(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    let (raw_prompt, parts, attachments) = tree
        .find::<ui::Composer>("composer")
        .map(|composer| {
            (
                composer.expanded_text(),
                composer.wire_parts(),
                composer.pending_attachments(),
            )
        })
        .unwrap_or_default();
    if raw_prompt.starts_with('/') && !raw_prompt.starts_with("//") {
        execute_slash_command(tree, commands.as_ref(), model, &raw_prompt, &parts);
        sync_slash_suggestions(tree, model);
        return;
    }
    let prompt = raw_prompt.trim().to_owned();
    if prompt.is_empty() && parts.is_empty() && attachments.is_empty() {
        return;
    }
    if !matches!(
        model.attachment,
        SessionAttachmentState::Attached | SessionAttachmentState::Ready
    ) {
        notice(
            tree,
            "raep not attached; message not sent (Ctrl+R retries)".into(),
            ui::Tone::Warn,
        );
        return;
    }
    if model.active.is_some() && !model.active_session_has_model() {
        notice(
            tree,
            "no model selected; message not sent (3 Models, Enter to choose)".into(),
            ui::Tone::Warn,
        );
        return;
    }
    let display_prompt = if prompt.is_empty() {
        format!("sent {} attachment(s)", attachments.len().max(parts.len()))
    } else {
        prompt.clone()
    };
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::User {
            text: display_prompt,
            turn: model
                .active_turn
                .as_ref()
                .and_then(|turn| crate::ui::parse_turn(Some(&turn.id))),
        });
        stream.start_thinking();
    }
    if !prompt.is_empty()
        && let Some(composer) = tree.find_mut::<ui::Composer>("composer")
    {
        composer.push_history(&prompt);
    }
    if let Some(commands) = commands.as_ref() {
        let queue_mode = model.preferences.send_mode.label();
        let _ = commands.try_send(UiCommand::Send {
            session: model.active.clone(),
            text: prompt,
            parts,
            attachments,
            queue_mode: queue_mode.to_string(),
            new_session_model: model.pending_new_session_model.clone(),
            new_session_reasoning: model.pending_new_session_reasoning.clone(),
        });
    }
    if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
        composer.clear();
    }
}

/// Ask the host to cancel the running turn; keeps the TUI up.
fn cancel_active_turn(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) -> bool {
    let Some(active) = model.active_turn.take() else {
        notice(
            tree,
            "a turn seems to be running but its id is unknown; cannot cancel".to_string(),
            ui::Tone::Warn,
        );
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
    toast(tree, "turn cancelled", ui::Tone::Neutral);
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.stop_thinking();
    }
    notice(tree, "turn cancelled".to_string(), ui::Tone::Neutral);
    false
}
