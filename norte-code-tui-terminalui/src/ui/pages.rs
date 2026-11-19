//! The catalogue pages: one builder per tab, fed from the model.
//!
//! Each page shows the same shape — a muted caption and a list of rows —
//! rendered by the shared [`Inspector`] view. The rows come straight from
//! the live data the client holds; nothing here invents mock content.

use std::fmt::Write;

use crate::ui_model::UiModel;

use super::inspector::{Inspector, Row, Selection};

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn human_duration(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

pub(crate) fn session(model: &UiModel) -> Inspector {
    let mut inspector = Inspector::new("sessions · newest first", Selection::Activate);
    let active = model.active.as_ref().map(|key| key.as_str().to_string());
    let mut rows: Vec<Row> = model
        .sessions
        .iter()
        .map(|session| {
            let is_active = active.as_deref() == Some(session.session_id.as_str());
            let marker = if is_active {
                "▸ "
            } else if session.pinned {
                "◆ "
            } else {
                "  "
            };
            let title = session.title.as_deref().unwrap_or(&session.session_id);
            let primary = format!("{marker}{}", clip(title, 44));
            let provider = session.provider.as_deref().unwrap_or("unknown");
            Row::new(primary).with_secondary(clip(provider, 22))
        })
        .collect();
    if rows.is_empty() {
        rows.push(Row::new("no sessions yet").with_secondary("detached"));
    }
    inspector.set_rows(rows);
    inspector
}

pub(crate) fn plans(model: &UiModel) -> Inspector {
    let mut inspector = Inspector::new("plans · newest first", Selection::Browse);
    let mut rows: Vec<Row> = model
        .plans
        .iter()
        .map(|plan| {
            let title = plan.title.clone().unwrap_or_else(|| clip(&plan.id, 48));
            let revision = if plan.revision > 0 {
                format!("r{} ", plan.revision)
            } else {
                String::new()
            };
            let primary = format!("{revision}{}", clip(&title, 44));
            Row::new(primary).with_secondary(plan.status.label())
        })
        .collect();
    if rows.is_empty() {
        rows.push(Row::new("no plans").with_secondary("nothing in flight"));
    }
    inspector.set_rows(rows);
    inspector
}

pub(crate) fn jobs(model: &UiModel) -> Inspector {
    let mut inspector = Inspector::new("jobs · background work", Selection::Browse);
    let mut rows: Vec<Row> = model
        .jobs
        .iter()
        .map(|job| {
            let duration = job
                .duration_ms
                .map_or_else(|| "running".to_string(), human_duration);
            let primary = clip(&job.command, 46);
            Row::new(primary).with_secondary(format!("{} · {duration}", job.status.label()))
        })
        .collect();
    if rows.is_empty() {
        rows.push(Row::new("no jobs").with_secondary("nothing running"));
    }
    inspector.set_rows(rows);
    inspector
}

fn request_status_label(status: norte_code_wire::RequestStatus) -> &'static str {
    match status {
        norte_code_wire::RequestStatus::Ok => "ok",
        norte_code_wire::RequestStatus::Error => "failed",
        norte_code_wire::RequestStatus::Cancelled => "cancelled",
    }
}

pub(crate) fn telemetry(model: &UiModel) -> Inspector {
    let mut inspector = Inspector::new("telemetry · session requests", Selection::Browse);
    let mut rows = Vec::new();
    if let Some(report) = &model.telemetry {
        let summary = &report.summary;
        let mut totals = format!("{} requests", summary.requests);
        if summary.errors > 0 {
            let _ = write!(&mut totals, " · {} errors", summary.errors);
        }
        if summary.cancelled > 0 {
            let _ = write!(&mut totals, " · {} cancelled", summary.cancelled);
        }
        rows.push(Row::new(totals).with_secondary("totals"));
        rows.push(
            Row::new(format!(
                "{} in / {} out",
                summary.input_tokens, summary.output_tokens
            ))
            .with_secondary("tokens"),
        );
        rows.push(
            Row::new(format!("{} cached reads", summary.cache_read_tokens)).with_secondary("cache"),
        );
        rows.push(Row::new(format!("{} ms total", summary.duration_ms)).with_secondary("duration"));

        for record in report.records.iter().rev().take(10) {
            let duration = human_duration(record.duration_ms);
            let model_name = record.model.clone();
            let status = request_status_label(record.status);
            rows.push(Row::new(model_name).with_secondary(format!("{status} · {duration}")));
        }
    }
    if rows.is_empty() {
        rows.push(Row::new("telemetry not loaded yet").with_secondary("waiting for attach"));
    }
    inspector.set_rows(rows);
    inspector
}

pub(crate) fn host(model: &UiModel) -> Inspector {
    let mut inspector = Inspector::new("host · catalog", Selection::Browse);
    let mut rows = Vec::new();
    for agent in &model.agents {
        let version = if agent.version.is_empty() {
            String::new()
        } else {
            format!(" v{}", agent.version)
        };
        rows.push(Row::new(format!("agent {}{}", agent.id, version)).with_secondary("agent"));
    }
    if let Some(catalog) = &model.catalog {
        for provider in &catalog.providers {
            let from_file = if provider.from_file { "file" } else { "plugin" };
            let key_tag = if provider.environment_keys.is_empty() {
                "no key needed"
            } else {
                "key required"
            };
            rows.push(
                Row::new(format!(
                    "{} · {} model(s)",
                    provider.provider_id, provider.model_count
                ))
                .with_secondary(format!("{from_file} · provider"))
                .with_tag(key_tag),
            );
        }
        rows.push(
            Row::new(format!("{} models in catalog", catalog.models.len())).with_secondary("total"),
        );
    }
    if rows.is_empty() {
        rows.push(Row::new("host data not loaded yet").with_secondary("waiting for attach"));
    }
    inspector.set_rows(rows);
    inspector
}

pub(crate) fn costs(model: &UiModel) -> Inspector {
    let mut inspector = Inspector::new("costs · provider-reported spend", Selection::Browse);
    let mut rows = Vec::new();
    if let Some(cost) = &model.cost {
        match cost.total_usd {
            Some(usd) => rows.push(Row::new(format!("${usd:.2}")).with_secondary("total usd")),
            None => rows.push(Row::new("no provider cost reported").with_secondary("usd")),
        }
        rows.push(Row::new(format!("{} requests", cost.requests)).with_secondary("requests"));
    }
    if let Some(report) = &model.telemetry {
        let total = report.summary.input_tokens + report.summary.output_tokens;
        rows.push(Row::new(format!("{total} tokens")).with_secondary("in+out"));
        rows.push(
            Row::new(format!("{} ms", report.summary.duration_ms)).with_secondary("duration"),
        );
    }
    if rows.is_empty() {
        rows.push(Row::new("cost data not loaded yet").with_secondary("waiting for attach"));
    }
    inspector.set_rows(rows);
    inspector
}
