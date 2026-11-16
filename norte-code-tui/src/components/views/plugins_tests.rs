use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::PluginsView;
use crate::{
    components::{Component, ComponentAction, PluginOperation, ViewAction},
    ui_model::UiModel,
};

#[test]
fn installs_a_plugin_from_a_repository_and_optional_package() {
    let mut view = PluginsView::default();
    let model = UiModel::default();
    for code in [
        KeyCode::Char('i'),
        KeyCode::Char('h'),
        KeyCode::Char('t'),
        KeyCode::Char('t'),
        KeyCode::Char('p'),
        KeyCode::Char('s'),
        KeyCode::Char(':'),
        KeyCode::Char('/'),
        KeyCode::Char('/'),
        KeyCode::Char('r'),
        KeyCode::Char('e'),
        KeyCode::Char('p'),
        KeyCode::Char('o'),
        KeyCode::Char('/'),
        KeyCode::Char('p'),
        KeyCode::Char('l'),
        KeyCode::Char('u'),
        KeyCode::Char('g'),
        KeyCode::Char('i'),
        KeyCode::Char('n'),
        KeyCode::Enter,
        KeyCode::Char('n'),
        KeyCode::Char('o'),
        KeyCode::Char('r'),
        KeyCode::Char('t'),
        KeyCode::Char('e'),
        KeyCode::Enter,
    ] {
        view.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &model);
    }

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::View(ViewAction::Plugin(PluginOperation::Install {
            url,
            package: Some(package),
        }))) if url == "https://repo/plugin" && package == "norte"
    ));
}
