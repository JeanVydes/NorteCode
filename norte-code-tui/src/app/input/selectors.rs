//! Header and selector changes after the tree has handled an event.

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
