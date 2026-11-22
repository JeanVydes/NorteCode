//! Chrome synchronization: header and rail state from the model.

use crate::ui;
use crate::ui_model::UiModel;

pub(in crate::app) fn sync_debug_overlay(
    tree: &mut terminalui::Tree,
    snapshot: &crate::metrics::MetricsSnapshot,
    enabled: bool,
) {
    if let Some(overlay) = tree.find_mut::<ui::DebugOverlay>("debug_overlay") {
        overlay.set_enabled(enabled);
        overlay.set_snapshot(snapshot);
    }
}

pub(in crate::app) fn sync_chrome(tree: &mut terminalui::Tree, model: &UiModel) {
    let jobs = model
        .jobs
        .iter()
        .filter(|job| job.status.is_running())
        .count();
    let theme_name = crate::theme::theme_name(&model.preferences.theme);
    if let Some(header) = tree.find_mut::<ui::Header>("header") {
        if header.turn != model.turn {
            header.turn = model.turn;
        }
        if header.theme_name != theme_name {
            theme_name.clone_into(&mut header.theme_name);
        }
        if header.tab != model.tab {
            header.tab = model.tab;
        }
        if header.jobs != jobs {
            header.set_jobs(jobs);
        }
    }
    if let Some(rail) = tree.find_mut::<ui::Rail>("rail") {
        if rail.attachment != model.attachment {
            rail.attachment = model.attachment.clone();
        }
        let session = model
            .active
            .as_ref()
            .map(norte_code_wire::SessionKey::as_str);
        if rail.session.as_deref() != session {
            rail.set_session(session.map(str::to_owned));
        }
        let model_id = model.active_model.as_ref().map(|model| model.id.as_str());
        if rail.model.as_deref() != model_id {
            rail.model = model_id.map(str::to_owned);
        }
        if rail.provider.as_deref() != model.active_provider.as_deref() {
            rail.provider.clone_from(&model.active_provider);
        }
        let context = model.effective_context();
        if rail.context() != context {
            rail.set_context(context);
        }
        if rail.plans() != model.plans.as_slice() {
            rail.set_plans(model.plans.clone());
        }
        if rail.todos() != model.todos.as_slice() {
            rail.set_todos(model.todos.clone());
        }
        let plugin_status: Vec<String> = model
            .plugins
            .iter()
            .filter_map(|plugin| plugin.status.clone())
            .collect();
        if rail.plugin_status() != plugin_status.as_slice() {
            rail.set_plugin_status(plugin_status);
        }
        let plugin_banner: Vec<String> = model
            .plugins
            .iter()
            .filter_map(|plugin| plugin.banner.clone())
            .collect();
        if rail.plugin_banner() != plugin_banner.as_slice() {
            rail.set_plugin_banner(plugin_banner);
        }
        let compact = model.active.is_some();
        if rail.compact_enabled() != compact {
            rail.set_compact_enabled(compact);
        }
    }
}
