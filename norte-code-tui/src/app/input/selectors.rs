//! Header and selector changes after the tree has handled an event.

use crate::client::UiCommand;
use crate::ui;
use crate::ui_model::UiModel;

/// Apply a tab the header reported as clicked.
pub(in crate::app) fn drain_tab_request(tree: &mut terminalui::Tree, model: &mut UiModel) {
    if let Some(header) = tree.find_mut::<ui::Header>("header")
        && let Some(tab) = header.take_requested_tab()
    {
        model.tab = tab;
        ui::sync_views(tree, model);
    }
}

/// After the tree dispatched, persist any selector cycle into the model
/// and notify the agent of changes the server understands.
pub(in crate::app) fn sync_selectors(
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
        let next = crate::ui_model::ReasoningLevel::from_label(selector.value())
            .unwrap_or(crate::ui_model::ReasoningLevel::Auto);
        if next != model.preferences.reasoning {
            model.preferences.reasoning = next;
            if let (Some(cmd), Some(session)) = (commands.as_ref(), model.active.clone()) {
                let effort = selector.value().to_string();
                let _ = cmd.try_send(UiCommand::SetReasoning { session, effort });
            }
        }
    }
    if let Some(selector) = tree.find_mut::<crate::ui::Selector>("queue") {
        model.preferences.send_mode = crate::ui_model::SendMode::from_label(selector.value())
            .unwrap_or(crate::ui_model::SendMode::Steer);
    }
}
