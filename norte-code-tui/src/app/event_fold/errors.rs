//! Error, budget, question, and model-change events.

use norte_code_wire::ServerEventKind;

use crate::ui;
use crate::ui_model::{TurnState, UiModel};

use super::{notice, toast};

/// Handle the failure-ish arms of the stream; `true` when one applied.
pub(in crate::app) fn fold_error_event(
    kind: &ServerEventKind,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) -> bool {
    match kind {
        ServerEventKind::Error {
            code: norte_code_wire::ErrorCode::ReplayUnavailable,
            message,
        } => notice(tree, message.clone(), ui::Tone::Warn),
        ServerEventKind::Error { message, .. }
            if message.starts_with("RAEP sequence gap")
                || message.contains("fell behind and dropped") =>
        {
            // The reader resyncs on the next frame; not a turn failure.
            notice(
                tree,
                "stream skipped events · resyncing".to_owned(),
                ui::Tone::Neutral,
            )
        }
        ServerEventKind::Error { message, .. } => stop_with_notice(
            tree,
            model,
            format!("host error · {message}"),
            ui::Tone::Error,
        ),
        ServerEventKind::BudgetExceeded {
            scope,
            spent,
            limit,
            unit,
            action,
        } => stop_with_notice(
            tree,
            model,
            format!("budget exceeded · {scope:?} · {spent:.2}/{limit:.2} {unit:?} · {action:?}"),
            ui::Tone::Error,
        ),
        ServerEventKind::BudgetWarning {
            scope,
            spent,
            limit,
            unit,
        } => notice(
            tree,
            format!("budget warning · {scope:?} · {spent:.2}/{limit:.2} {unit:?}"),
            ui::Tone::Warn,
        ),
        ServerEventKind::QuestionAsked {
            question,
            prompt,
            options,
            multiple,
            ..
        } => {
            fold_question_event(question, prompt, options, *multiple, tree, model);
            true
        }
        ServerEventKind::ModelQueued {
            provider,
            model: queued,
        } => notice(
            tree,
            format!("model queued · {provider} · {queued}"),
            ui::Tone::Neutral,
        ),
        ServerEventKind::ModelChanged {
            provider,
            model: new_model,
        } => fold_model_changed(tree, model, provider, new_model),
        ServerEventKind::RetryScheduled {
            attempt, reason, ..
        } => {
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.push_retry(format!("retry {attempt} · {reason}"), ui::Tone::Warn);
            }
            true
        }
        ServerEventKind::RetryStarted { attempt, .. } => {
            if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
                stream.push_retry(format!("retry {attempt} running"), ui::Tone::Neutral);
            }
            true
        }
        ServerEventKind::ReasoningDowngraded { from, to, reason } => notice(
            tree,
            format!("reasoning {from} → {to} · {reason}"),
            ui::Tone::Warn,
        ),
        _ => false,
    }
}

/// Apply the host's confirmation that the session's model changed.
fn fold_model_changed(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    provider: &str,
    new_model: &str,
) -> bool {
    if let Some(active) = model.active.as_ref()
        && let Some(summary) = model
            .sessions
            .iter_mut()
            .find(|summary| summary.session_id == active.as_str())
    {
        summary.provider = Some(provider.to_owned());
        summary.model = Some(norte_code_wire::AgentRef {
            id: new_model.to_owned(),
            version: String::new(),
        });
    }
    model.active_provider = Some(provider.to_owned());
    model.active_model = Some(norte_code_wire::AgentRef {
        id: new_model.to_owned(),
        version: String::new(),
    });
    notice(
        tree,
        format!("model · {provider} · {new_model}"),
        ui::Tone::Neutral,
    )
}

/// Park the turn on a question, opening the richer review for plans.
fn fold_question_event(
    question: &str,
    prompt: &str,
    options: &[String],
    multiple: bool,
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
) {
    model.turn = TurnState::AwaitingAnswer;
    model.question = Some(crate::ui_model::PendingQuestion {
        question: question.to_owned(),
        prompt: prompt.to_owned(),
        options: options.to_vec(),
        multiple,
    });
    if let Some(content) = prompt.strip_prefix(ui::PLAN_REVIEW_PREFIX)
        && let Some(overlay) = tree.find_mut::<ui::PlanOverlay>("plan_overlay")
    {
        overlay.begin_review(question.to_owned(), content.to_owned());
    }
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.stop_thinking();
        let text = if prompt.starts_with(ui::PLAN_REVIEW_PREFIX) {
            "the model asks · plan review".to_owned()
        } else {
            format!("the model asks · {prompt}")
        };
        stream.push(ui::Item::Notice {
            text,
            tone: ui::Tone::Warn,
        });
    }
}

fn stop_with_notice(
    tree: &mut terminalui::Tree,
    model: &mut UiModel,
    text: String,
    tone: ui::Tone,
) -> bool {
    model.turn = TurnState::Idle;
    toast(tree, &text, tone);
    if let Some(stream) = tree.find_mut::<ui::Transcript>("stream") {
        stream.stop_thinking();
        stream.complete_running_tools();
        stream.push(ui::Item::Notice { text, tone });
    }
    true
}
