//! The fields shown for a session, shared by the sessions and telemetry tabs.

use terminalui::components::badge::BadgeVariant;

use norte_code_wire::{SessionStatus, SessionSummary};

use super::detail_panel::Field;
use crate::context::human_count;
use crate::session_stats::SessionUsage;

fn duration(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn status_variant(status: SessionStatus) -> BadgeVariant {
    match status {
        SessionStatus::Active => BadgeVariant::Success,
        SessionStatus::Suspended => BadgeVariant::Warning,
        SessionStatus::Closed | SessionStatus::Unknown => BadgeVariant::Neutral,
    }
}

/// The panel content for one session and its rolled-up usage.
pub(crate) fn fields(session: &SessionSummary, usage: &SessionUsage) -> Vec<Field> {
    let title = session
        .title
        .clone()
        .unwrap_or_else(|| "untitled".to_owned());
    let model = session.model.as_ref().map_or_else(
        || "unknown".to_owned(),
        |model| {
            session.provider.as_ref().map_or_else(
                || model.id.clone(),
                |provider| format!("{provider}/{}", model.id),
            )
        },
    );
    let turns = session.turn_count.unwrap_or(usage.turns.len() as u64);
    let models = usage
        .top_models(2)
        .into_iter()
        .map(|(model, count)| format!("{model} ×{count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut fields = vec![
        Field::new("title", title),
        Field::new("id", terminalui::clip(&session.session_id, 36)),
        Field::new("model", terminalui::clip(&model, 36)),
        Field::badge(
            "status",
            session.status.label(),
            status_variant(session.status),
        ),
        Field::new(
            "agent",
            session
                .agent
                .as_ref()
                .map_or_else(|| "unknown".to_owned(), |agent| agent.id.clone()),
        ),
        Field::new("turns", turns.to_string()),
        Field::new("requests", usage.requests.to_string()),
        Field::new(
            "tokens",
            format!("{}:{}", human_count(usage.input), human_count(usage.output)),
        ),
        Field::new("cache", format!("{:.0}%", usage.cache_rate())),
        Field::new("tools", usage.tools.to_string()),
        Field::new(
            "models",
            if models.is_empty() {
                "none".to_owned()
            } else {
                models
            },
        ),
    ];
    if usage.errors > 0 || usage.cancelled > 0 {
        fields.push(Field::new(
            "failed",
            format!("{} err · {} cancelled", usage.errors, usage.cancelled),
        ));
    }
    if usage.duration_ms > 0 {
        fields.push(Field::new("time", duration(usage.duration_ms)));
    }
    if usage.has_cost {
        fields.push(Field::new("cost", format!("${:.4}", usage.cost)));
    }
    if let Some(parent) = &session.parent_session_id {
        fields.push(Field::new("parent", terminalui::clip(parent, 34)));
    }
    fields.push(Field::separator());
    fields.push(Field::badge(
        "pinned",
        session.pinned.to_string(),
        if session.pinned {
            BadgeVariant::Success
        } else {
            BadgeVariant::Neutral
        },
    ));
    fields.push(Field::badge(
        "archived",
        session.archived.to_string(),
        if session.archived {
            BadgeVariant::Warning
        } else {
            BadgeVariant::Neutral
        },
    ));
    fields
}
