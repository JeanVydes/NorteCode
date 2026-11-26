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
    if let Some(host) = tree.find_mut::<ui::HostOverlay>("host_overlay") {
        host.sync(model);
    }
    if let Some(overlay) = tree.find_mut::<ui::PlanOverlay>("plan_overlay") {
        overlay.sync_plans(&model.plans);
    }
    if let Some(transcript) = tree.find_mut::<ui::Transcript>("stream") {
        transcript.set_sessionless(model.active.is_none());
    }
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
        if rail.title != model.attached_title {
            rail.set_title(model.attached_title.clone());
        }
        let selection = model.effective_model_selection();
        let model_id = selection.map(|(_, model)| model);
        if rail.model.as_deref() != model_id {
            rail.model = model_id.map(str::to_owned);
        }
        let provider = selection.map(|(provider, _)| provider);
        if rail.provider.as_deref() != provider {
            rail.provider = provider.map(str::to_owned);
        }
        let thinking = model.thinking_mode();
        let queue_mode = model.preferences.send_mode.label();
        if rail.thinking != thinking || rail.queue_mode != queue_mode {
            rail.set_modes(thinking, queue_mode.to_owned());
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
        let views = model
            .plugin_rail
            .as_ref()
            .filter(|(selection, _)| model.plugin_widget_scope().as_ref() == Some(selection))
            .map_or(&[][..], |(_, views)| views.as_slice());
        rail.set_plugin_views(views);
    }
}
