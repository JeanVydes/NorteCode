//! Drains view-owned actions into client commands.

#[path = "drain/pages.rs"]
mod pages;

use terminalui::components::button::Button;

use crate::attachment::ComposerAction;
use crate::client::UiCommand;
use crate::ui;
use crate::ui_model::UiModel;

use super::event_fold::push_attachment_notice;
pub(in crate::app) use pages::{
    drain_config_actions, drain_models_actions, drain_plugins_actions, drain_sessions_actions,
};

pub(in crate::app) fn drain_message_actions(
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
                let _ = crate::clipboard::system().write(&text);
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

pub(in crate::app) fn drain_composer_actions(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &UiModel,
) {
    let actions = tree
        .find_mut::<ui::Composer>("composer")
        .map(ui::Composer::take_actions)
        .unwrap_or_default();
    for action in actions {
        match action {
            ComposerAction::Notice(notice) => push_attachment_notice(tree, &notice),
            ComposerAction::Attach(attachment) => {
                if let Err(error) = crate::attachment::validate(model, &attachment.mime) {
                    push_attachment_notice(tree, &error.into());
                    continue;
                }
                if let (Some(commands), Some(session)) = (commands.as_ref(), model.active.clone()) {
                    let _ = commands.try_send(UiCommand::Upload {
                        session,
                        attachment,
                    });
                }
            }
        }
    }
}

pub(in crate::app) fn drain_compact(
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
