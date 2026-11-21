//! The command palette: toggle, key routing, and action dispatch.

use crossterm::event::{Event, KeyCode, KeyModifiers};

use crate::client::UiCommand;
use crate::ui;
use crate::ui_model::UiModel;

use super::{cycle_theme, settle_input};

/// Feed the command palette first. Returns true when it handled the key.
pub(in crate::app) fn handle_palette_key(
    tree: &mut terminalui::Tree,
    key: crossterm::event::KeyEvent,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) -> bool {
    if (key.modifiers, key.code) == (KeyModifiers::CONTROL, KeyCode::Char('p')) {
        let return_focus = tree.focused_id().map(str::to_owned);
        let restored = tree
            .find_mut::<ui::CommandPalette>("command_palette")
            .and_then(|palette| palette.toggle(return_focus.as_deref()));
        if let Some(id) = restored {
            tree.focus(&id);
        } else if tree
            .find::<ui::CommandPalette>("command_palette")
            .is_some_and(ui::CommandPalette::is_open)
        {
            tree.focus("command_palette");
        }
        settle_input(tree, commands, model);
        return true;
    }
    if tree
        .find::<ui::CommandPalette>("command_palette")
        .is_some_and(ui::CommandPalette::is_open)
    {
        if (key.modifiers, key.code) == (KeyModifiers::CONTROL, KeyCode::Char('c')) {
            tree.find_mut::<ui::CommandPalette>("command_palette")
                .expect("command palette")
                .close();
        } else {
            let input = terminalui::InputEvent::from(Event::Key(key));
            tree.handle_event(&input);
        }
        restore_palette_focus(tree);
        settle_input(tree, commands, model);
        return true;
    }
    false
}

fn restore_palette_focus(tree: &mut terminalui::Tree) {
    let focus = tree
        .find_mut::<ui::CommandPalette>("command_palette")
        .and_then(ui::CommandPalette::take_return_focus);
    if let Some(id) = focus {
        tree.focus(&id);
    }
}

pub(in crate::app) fn drain_palette(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    let taken = tree
        .find_mut::<ui::CommandPalette>("command_palette")
        .and_then(ui::CommandPalette::take_action);
    let Some((action, focus)) = taken else {
        return;
    };
    if let Some(id) = focus {
        tree.focus(&id);
    }
    match action {
        ui::PaletteAction::SwitchTab(tab) => {
            model.tab = tab;
            ui::sync_views(tree, model);
        }
        ui::PaletteAction::CycleTheme => cycle_theme(tree, model),
        ui::PaletteAction::Refresh => {
            if let Some(commands) = commands.as_ref() {
                let _ = commands.try_send(UiCommand::Refresh);
                let _ = commands.try_send(UiCommand::RefreshViews);
            }
        }
        ui::PaletteAction::NewSession => {
            if let Some(commands) = commands.as_ref() {
                let _ = commands.try_send(UiCommand::CreateSession { title: None });
            }
        }
        ui::PaletteAction::Compact => {
            tree.press("compact");
        }
    }
}
