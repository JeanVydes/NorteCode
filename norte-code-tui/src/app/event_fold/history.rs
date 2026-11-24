//! History replay and the attach notice shown with it.

use crate::ui;

/// Replay a session's history into the stream, replacing what is there.
pub(in crate::app) fn fold_history_event(
    tree: &mut terminalui::Tree,
    model: &crate::ui_model::UiModel,
    items: Vec<norte_code_wire::HistoryItem>,
) -> bool {
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.replace_history(items);
    }
    push_attach_notice(tree, model);
    true
}

/// Report a history fetch that failed; an empty stream beats a stale one.
pub(in crate::app) fn fold_history_failure(
    tree: &mut terminalui::Tree,
    model: &crate::ui_model::UiModel,
    reason: &crate::client_error::ClientError,
) -> bool {
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::Notice {
            text: format!("could not load the session's history · {reason}"),
            tone: ui::Tone::Warn,
        });
    }
    push_attach_notice(tree, model);
    true
}

/// The attach notice, pushed along with the replayed history.
fn push_attach_notice(tree: &mut terminalui::Tree, model: &crate::ui_model::UiModel) {
    let Some(title) = model.attached_title.clone() else {
        return;
    };
    super::notice(tree, format!("attached to {title}"), ui::Tone::Neutral);
    if !model.active_session_has_model() {
        super::notice(
            tree,
            "no model selected; 3 Models, Enter to choose".into(),
            ui::Tone::Warn,
        );
    }
}
