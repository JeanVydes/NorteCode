//! Key presses: shortcuts, prompts, and the composer send path.

use crossterm::event::{Event, KeyCode, KeyModifiers};

use crate::client::{ClientEvent, UiCommand, start};
use crate::config::ClientConfig;
use crate::ui;
use crate::ui_model::{SessionAttachmentState, Tab, TurnState, UiModel};

use super::super::event_fold::{notice, toast};
use super::palette::handle_palette_key;
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
    question_open || prompt_open || plan_open
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
    if handle_palette_key(tree, key, commands, model) {
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
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            let copied = tree
                .find_mut::<ui::Composer>("composer")
                .is_some_and(ui::Composer::copy_selection);
            if !copied {
                return true;
            }
            pass_to_tree = false;
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
        (KeyModifiers::NONE, KeyCode::PageUp) => {
            scroll_page(tree, model.tab, -6);
            pass_to_tree = false;
        }
        (KeyModifiers::NONE, KeyCode::PageDown) => {
            scroll_page(tree, model.tab, 6);
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
            set_model(commands, model, &provider, &model_id);
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

/// Ask the host to switch the attached session's model.
pub(in crate::app) fn set_model(
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
    let (prompt, parts) = tree
        .find::<ui::Composer>("composer")
        .map(|composer| {
            (
                composer.expanded_text().trim().to_string(),
                composer.wire_parts(),
            )
        })
        .unwrap_or_default();
    if prompt.is_empty() {
        return;
    }
    let SessionAttachmentState::Attached = model.attachment else {
        notice(
            tree,
            "raep not attached; message not sent (Ctrl+R retries)".into(),
            ui::Tone::Warn,
        );
        return;
    };
    if !model.active_session_has_model() {
        notice(
            tree,
            "no model selected; message not sent (3 Models, Enter to choose)".into(),
            ui::Tone::Warn,
        );
        return;
    }
    let Some(session) = model.active.clone() else {
        return;
    };
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::User {
            text: prompt.clone(),
            turn: model
                .active_turn
                .as_ref()
                .and_then(|turn| crate::ui::parse_turn(Some(&turn.id))),
        });
        stream.start_thinking();
    }
    if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
        composer.push_history(&prompt);
    }
    if let Some(commands) = commands.as_ref() {
        let queue_mode = model.preferences.send_mode.label();
        let _ = commands.try_send(UiCommand::Send {
            session,
            text: prompt,
            parts,
            queue_mode: queue_mode.to_string(),
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
