//! Client events: connection state, catalogue data, and channel draining.

use std::sync::Arc;

use crate::attachment::AttachmentNotice;
use crate::client::{ClientEvent, UiCommand};
use crate::ui;
use crate::ui_model::{SessionAttachmentState, TurnState, UiModel};

use super::super::DrainOutcome;
use super::super::input::apply_key_intent;
use super::history::{fold_history_event, fold_history_failure};
use super::{fold, push_attachment_notice, toast};

/// Track the host-reported model of the attached session from a list refresh.
fn refresh_active_model(model: &mut UiModel, sessions: &[norte_code_wire::SessionSummary]) {
    if let Some(active) = model.active.as_ref()
        && let Some(found) = sessions
            .iter()
            .find(|candidate| candidate.session_id == active.as_str())
    {
        model.active_model = found.model.clone();
        model.active_provider = found.provider.clone();
        model.active_reasoning = found.reasoning.clone();
    }
}

fn apply_attached(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    session: &norte_code_wire::SessionSummary,
    preserve_transcript: bool,
) -> bool {
    let title = session
        .title
        .clone()
        .unwrap_or_else(|| session.session_id.clone());
    let changed_session = model
        .active
        .as_ref()
        .is_none_or(|active| active.as_str() != session.session_id);
    model.attachment = SessionAttachmentState::Attached;
    model.active_model.clone_from(&session.model);
    model.active_provider.clone_from(&session.provider);
    model.active_reasoning.clone_from(&session.reasoning);
    model.active = Some(norte_code_wire::SessionKey::new(session.session_id.clone()));
    model.pending_new_session_model = None;
    model.pending_new_session_reasoning = None;
    model.attached_title = Some(title.clone());
    if changed_session {
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            if !preserve_transcript {
                // Never show another session's content under this label.
                stream.clear_all();
            }
            stream.stop_thinking();
        }
        model.active_turn = None;
        model.turn = TurnState::Idle;
        model.plans.clear();
        model.todos.clear();
        model.jobs.clear();
        model.context = crate::context::ContextUsage::default();
        model.question = None;
        model.cost = None;
    }
    true
}

fn apply_detached(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    reason: &crate::client_error::ClientError,
) -> bool {
    let reason = reason.to_string();
    model.attachment = SessionAttachmentState::Failed {
        reason: reason.clone(),
    };
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::Notice {
            text: reason,
            tone: ui::Tone::Error,
        });
    }
    true
}

fn fold_catalog_event(event: &ClientEvent, model: &mut UiModel) -> bool {
    match event {
        ClientEvent::Models(models) => model.models.clone_from(models),
        ClientEvent::Plans(plans) => model.plans.clone_from(plans),
        ClientEvent::Jobs(jobs) => model.jobs.clone_from(jobs),
        ClientEvent::Telemetry(report) => {
            model.telemetry_usage = crate::session_stats::shared_by_session(&report.records);
            model.telemetry = Some(Arc::clone(report));
        }
        ClientEvent::CostSession(cost) | ClientEvent::CostGlobal(cost) => {
            model.cost = Some(cost.clone());
        }
        ClientEvent::Plugins(plugins) => model.plugins.clone_from(plugins),
        ClientEvent::Catalog(catalog) => model.catalog = Some(catalog.clone()),
        ClientEvent::Agents(agents) => model.agents.clone_from(agents),
        _ => return false,
    }
    true
}

fn fold_host_event(event: &ClientEvent, model: &mut UiModel) {
    match event {
        ClientEvent::ServerInfo(info) => model.server_info = Some(info.clone()),
        ClientEvent::Liveness(live) => model.liveness = Some(live.clone()),
        ClientEvent::Readiness(ready) => model.readiness = Some(ready.clone()),
        ClientEvent::Quota(quota) => model.quota = Some(quota.clone()),
        _ => {}
    }
}

pub(in crate::app) fn fold_upload_event(event: &ClientEvent, tree: &mut terminalui::Tree) -> bool {
    match event {
        ClientEvent::Uploaded {
            attachment,
            receipt,
        } => {
            if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
                composer.insert_upload(attachment, receipt.clone());
            }
            true
        }
        ClientEvent::UploadFailed { reason } => {
            push_attachment_notice(
                tree,
                &AttachmentNotice::UploadFailed {
                    reason: reason.to_string(),
                },
            );
            true
        }
        _ => false,
    }
}

fn fold_secret_event(
    event: &ClientEvent,
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) -> bool {
    match event {
        ClientEvent::SecretSaved { provider } => {
            let intent = tree
                .find_mut::<ui::KeyPrompt>("key_prompt")
                .and_then(|prompt| {
                    let matches = prompt.provider() == Some(provider.as_str());
                    matches.then(|| prompt.saved()).flatten()
                });
            if let Some(intent) = intent {
                apply_key_intent(tree, commands, model, intent);
            }
            if let Some(commands) = commands.as_ref() {
                let _ = commands.try_send(UiCommand::RefreshViews);
            }
            true
        }
        ClientEvent::SecretSaveFailed { reason } => {
            if let Some(prompt) = tree.find_mut::<ui::KeyPrompt>("key_prompt") {
                let theme = model.preferences.catalog.theme(&model.preferences.theme);
                prompt.fail(reason.to_string(), theme.destructive);
            }
            true
        }
        ClientEvent::SecretDeleted { provider } => {
            ui::invalidate_key(provider);
            toast(
                tree,
                &format!("key removed for {provider}"),
                ui::Tone::Neutral,
            );
            if let Some(commands) = commands.as_ref() {
                let _ = commands.try_send(UiCommand::RefreshViews);
            }
            true
        }
        ClientEvent::SecretDeleteFailed { reason } => {
            toast(tree, &reason.to_string(), ui::Tone::Error);
            true
        }
        _ => false,
    }
}

fn fold_send_failure(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    reason: &crate::client_error::ClientError,
) {
    if matches!(reason, crate::client_error::ClientError::SendPrompt(_))
        && model.active_turn.is_none()
    {
        // The host refused the message, so no turn will ever end
        // the loading row the send opened.
        model.turn = TurnState::Idle;
        if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
            stream.stop_thinking();
        }
    }
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.push(ui::Item::Notice {
            text: reason.to_string(),
            tone: ui::Tone::Error,
        });
    }
}

#[allow(clippy::too_many_lines)]
pub(in crate::app) fn drain_server_events(
    events: &mut Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) -> DrainOutcome {
    let Some(receiver) = events else {
        return DrainOutcome::default();
    };
    let mut changed = false;
    let mut count = 0usize;
    loop {
        let next_event = receiver.try_recv();
        if next_event.is_ok() {
            count += 1;
        }
        match next_event {
            Ok(ClientEvent::Attached {
                session,
                preserve_transcript,
            }) => {
                changed = apply_attached(tree, model, &session, preserve_transcript) || changed;
            }
            Ok(ClientEvent::Ready) => {
                model.attachment = SessionAttachmentState::Ready;
                changed = true;
            }
            Ok(ClientEvent::Detached { reason }) => {
                changed = apply_detached(tree, model, &reason) || changed;
            }
            Ok(ClientEvent::Reconnected) => {
                model.attachment = SessionAttachmentState::Attached;
                changed = true;
            }
            Ok(ClientEvent::Sessions(sessions)) => {
                refresh_active_model(model, &sessions);
                model.sessions = sessions;
                changed = true;
            }
            Ok(ClientEvent::DraftStarted { model: selection }) => {
                model.active = None;
                model.pending_new_session_model = selection;
                model.pending_new_session_reasoning = None;
                model.active_model = None;
                model.active_provider = None;
                model.active_reasoning = None;
                model.active_turn = None;
                model.turn = TurnState::Idle;
                model.plans.clear();
                model.todos.clear();
                model.jobs.clear();
                model.context = crate::context::ContextUsage::default();
                model.question = None;
                model.attached_title = None;
                model.attachment = SessionAttachmentState::Ready;
                if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                    stream.clear_all();
                }
                if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
                    composer.clear_all();
                }
                changed = true;
            }
            Ok(ClientEvent::DataCleared) => {
                model.sessions.clear();
                model.active = None;
                model.pending_new_session_model = None;
                model.pending_new_session_reasoning = None;
                model.active_model = None;
                model.active_provider = None;
                model.active_reasoning = None;
                model.active_turn = None;
                model.turn = TurnState::Idle;
                model.plans.clear();
                model.todos.clear();
                model.jobs.clear();
                model.telemetry = None;
                model.telemetry_usage = crate::session_stats::SharedUsage::default();
                model.cost = None;
                model.quota = None;
                model.question = None;
                model.context = crate::context::ContextUsage::default();
                model.attached_title = None;
                model.attachment = SessionAttachmentState::Ready;
                if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                    stream.clear_all();
                }
                if let Some(composer) = tree.find_mut::<ui::Composer>("composer") {
                    composer.clear_all();
                }
                changed = true;
            }
            Ok(
                event @ (ClientEvent::Models(_)
                | ClientEvent::Plans(_)
                | ClientEvent::Jobs(_)
                | ClientEvent::Telemetry(_)
                | ClientEvent::CostSession(_)
                | ClientEvent::CostGlobal(_)
                | ClientEvent::Plugins(_)
                | ClientEvent::Catalog(_)
                | ClientEvent::Agents(_)),
            ) => {
                changed = fold_catalog_event(&event, model) || changed;
            }
            Ok(
                event @ (ClientEvent::ServerInfo(_)
                | ClientEvent::Liveness(_)
                | ClientEvent::Readiness(_)
                | ClientEvent::Quota(_)),
            ) => {
                fold_host_event(&event, model);
                changed = true;
            }
            Ok(ClientEvent::Forked { session }) => {
                if let Some(commands) = commands.as_ref() {
                    let _ = commands.try_send(UiCommand::OpenSession { session });
                }
            }
            Ok(ClientEvent::SendFailed { reason }) => {
                fold_send_failure(tree, model, &reason);
                changed = true;
            }
            Ok(event @ (ClientEvent::Uploaded { .. } | ClientEvent::UploadFailed { .. })) => {
                changed = fold_upload_event(&event, tree) || changed;
            }
            Ok(
                event @ (ClientEvent::SecretSaved { .. }
                | ClientEvent::SecretSaveFailed { .. }
                | ClientEvent::SecretDeleted { .. }
                | ClientEvent::SecretDeleteFailed { .. }),
            ) => {
                changed = fold_secret_event(&event, tree, commands, model) || changed;
            }
            Ok(ClientEvent::Notice { text }) => {
                if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                    stream.push(ui::Item::Notice {
                        text,
                        tone: ui::Tone::Neutral,
                    });
                }
                changed = true;
            }
            Ok(ClientEvent::History(items)) => {
                changed = fold_history_event(tree, model, items) || changed;
            }
            Ok(ClientEvent::HistoryFailed { reason }) => {
                changed = fold_history_failure(tree, model, &reason) || changed;
            }
            Ok(ClientEvent::Server(event)) => {
                fold(&event, tree, model, commands);
                changed = true;
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                return DrainOutcome {
                    changed,
                    events: count,
                };
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                *events = None;
                return DrainOutcome {
                    changed,
                    events: count,
                };
            }
        }
    }
}
