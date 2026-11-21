//! Plan review and question overlays: committed answers become commands.

use crate::client::UiCommand;
use crate::ui;
use crate::ui_model::UiModel;

pub(in crate::app) fn drain_plan_requests(tree: &mut terminalui::Tree, model: &UiModel) {
    let Some(id) = tree
        .find_mut::<ui::Rail>("rail")
        .and_then(ui::Rail::take_plan_request)
    else {
        return;
    };
    if let Some(plan) = model.plans.iter().find(|plan| plan.id == id).cloned()
        && let Some(overlay) = tree.find_mut::<ui::PlanOverlay>("plan_overlay")
    {
        overlay.open(plan);
    }
}

/// Ship the plan decision the review overlay just committed, if any.
pub(in crate::app) fn drain_plan_review(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    let Some((question, action)) = tree
        .find_mut::<ui::PlanOverlay>("plan_overlay")
        .and_then(ui::PlanOverlay::take_action)
    else {
        return;
    };
    if let (Some(commands), Some(session)) = (commands.as_ref(), model.active.clone()) {
        let answer = match &action {
            ui::PlanReviewAction::Accept => crate::answer::QuestionAnswer::approve(),
            ui::PlanReviewAction::Decline => crate::answer::QuestionAnswer::reject(),
            ui::PlanReviewAction::Feedback(text) => text.clone().into_answer(),
        };
        let _ = commands.try_send(UiCommand::AnswerQuestion {
            session,
            question,
            answer,
        });
    }
    model.question = None;
    ui::sync_views(tree, model);
}

/// Ship the answer the ask-user overlay just committed, if any.
pub(in crate::app) fn drain_question_answer(
    tree: &mut terminalui::Tree,
    commands: &mut Option<tokio::sync::mpsc::Sender<UiCommand>>,
    model: &mut UiModel,
) {
    let Some(prompt) = tree.find_mut::<ui::QuestionPrompt>("question_prompt") else {
        return;
    };
    let Some((question_id, answer)) = prompt.take_pending_answer() else {
        return;
    };
    if let (Some(commands), Some(session)) = (commands.as_ref(), model.active.clone()) {
        let _ = commands.try_send(UiCommand::AnswerQuestion {
            session,
            question: question_id,
            answer,
        });
    }
    model.question = None;
    ui::sync_views(tree, model);
}
