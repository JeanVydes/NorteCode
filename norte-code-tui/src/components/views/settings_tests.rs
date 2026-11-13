use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::SettingsView;
use crate::{
    components::{Component, ComponentAction, Preference, ViewAction},
    ui_model::UiModel,
};

#[test]
fn enter_toggles_the_selected_setting() {
    let mut view = SettingsView::default();
    view.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &UiModel::default(),
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::View(ViewAction::TogglePreference(
            Preference::Theme
        )))
    ));
}

#[test]
fn budget_action_is_available_as_a_persistable_setting() {
    let mut view = SettingsView::default();
    for _ in 0..7 {
        view.handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &UiModel::default(),
        );
    }
    view.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &UiModel::default(),
    );

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::View(ViewAction::TogglePreference(
            Preference::BudgetAction
        )))
    ));
}
