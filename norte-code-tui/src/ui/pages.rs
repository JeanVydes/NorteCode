//! The catalogue pages: one builder per tab, fed from the model.
//!
//! Each page shows the same shape — a muted caption and a list of rows —
//! rendered by the shared [`Inspector`] view. The rows come straight from
//! the live data the client holds; nothing here invents mock content.

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
