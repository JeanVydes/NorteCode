//! Tab and list navigation helpers.

use crate::ui;
use crate::ui_model::{Tab, UiModel};

pub(in crate::app) fn digit_switches_tab(tree: &terminalui::Tree, model: &UiModel) -> bool {
    model.tab != Tab::Chat
        && model.tab != Tab::Models
        && !(model.tab == Tab::Telemetry && telemetry_rankings_focused(tree))
}

pub(in crate::app) fn toggle_list_focus(tree: &mut terminalui::Tree, tab: Tab) {
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
        Tab::Chat | Tab::Sessions | Tab::Models => {}
    }
}

fn telemetry_rankings_focused(tree: &terminalui::Tree) -> bool {
    tree.find::<ui::TelemetryPage>("telemetry")
        .is_some_and(ui::TelemetryPage::is_leaderboard_focused)
}
