//! Formatting helpers for safe agent profile details.

use super::detail_panel::Field;

pub(super) fn ref_id(reference: Option<&norte_code_wire::AgentRef>) -> String {
    reference.map_or_else(|| "not configured".to_owned(), |item| item.id.clone())
}

pub(super) fn enabled(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

pub(super) fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items.join(", ")
    }
}

pub(super) fn modality_label(modality: norte_code_wire::OutputModality) -> &'static str {
    match modality {
        norte_code_wire::OutputModality::Text => "text",
        norte_code_wire::OutputModality::Image => "image",
        norte_code_wire::OutputModality::Audio => "audio",
        norte_code_wire::OutputModality::Video => "video",
        norte_code_wire::OutputModality::Document => "document",
    }
}

pub(super) fn modality_list(modalities: &[norte_code_wire::OutputModality]) -> String {
    if modalities.is_empty() {
        "none reported".to_owned()
    } else {
        modalities
            .iter()
            .map(|modality| modality_label(*modality))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(super) fn reasoning_label(reasoning: &norte_code_wire::AgentReasoningSummary) -> String {
    match reasoning {
        norte_code_wire::AgentReasoningSummary::Automatic => "automatic".to_owned(),
        norte_code_wire::AgentReasoningSummary::Disabled => "off".to_owned(),
        norte_code_wire::AgentReasoningSummary::Effort(effort) => effort.clone(),
        norte_code_wire::AgentReasoningSummary::Budget(tokens) => format!("{tokens} tokens"),
        norte_code_wire::AgentReasoningSummary::Custom => "custom value".to_owned(),
    }
}

pub(super) fn budget_fields(
    fields: &mut Vec<Field>,
    profile: &norte_code_wire::AgentProfileSummary,
) {
    for (label, budget) in [
        ("message budget", profile.budgets.message),
        ("session budget", profile.budgets.session),
        ("daily budget", profile.budgets.global_daily),
    ] {
        if let Some(budget) = budget {
            let limit = match budget.unit {
                norte_code_wire::BudgetUnit::Usd => format!("${:.2}", budget.limit),
                norte_code_wire::BudgetUnit::Tokens => format!("{:.0} tokens", budget.limit),
                norte_code_wire::BudgetUnit::Requests => format!("{:.0} requests", budget.limit),
            };
            let action = match budget.action {
                norte_code_wire::BudgetAction::Warn => "warn",
                norte_code_wire::BudgetAction::Ask => "ask",
                norte_code_wire::BudgetAction::Stop => "stop",
            };
            fields.push(Field::new(label, format!("{limit} · {action}")));
        }
    }
}
