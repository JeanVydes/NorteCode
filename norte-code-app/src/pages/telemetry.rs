use crate::api::{ApiClient, TelemetryRecord, TelemetryStatus};
use crate::components::{EmptyState, MetricCard, PageHeader};
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn TelemetryPage() -> Element {
    let context = use_context::<AppContext>();
    let report = context.telemetry.read().clone();
    let mut selected_only = use_signal(|| false);
    let selected_session = context.selected_session.read().clone();
    let records = report
        .as_ref()
        .map_or_else(Vec::new, |value| value.records.clone());
    let max_duration = records
        .iter()
        .map(|record| record.duration_ms)
        .max()
        .unwrap_or_default();
    let record_count = records.len();
    rsx! {
        PageHeader {
            eyebrow: "Observability",
            title: "Telemetry",
            description: "Request-level provider metrics reported by RAEP. Missing token or cost values remain unreported rather than being estimated."
        }
        div { class: "section-heading",
            div {
                h2 { "Request activity" }
                p { class: "helper-text", "The host currently returns retained records without server-side date filters or pagination." }
            }
            div { class: "button-row",
                label { class: "field",
                    span { "Scope" }
                    select {
                        value: if *selected_only.read() { "session" } else { "all" },
                        onchange: move |event| selected_only.set(event.value() == "session"),
                        option { value: "all", "All sessions" }
                        option {
                            value: "session",
                            disabled: selected_session.is_none(),
                            "Selected session"
                        }
                    }
                }
                button {
                    class: "button",
                    onclick: move |_| load_telemetry(&context, *selected_only.read()),
                    "Load telemetry"
                }
            }
        }
        if let Some(report) = report {
            p { class: "helper-text", "Report scope: {report.session.as_deref().unwrap_or(\"All sessions\")}" }
            div { class: "grid metric-grid",
                MetricCard { label: "Requests", value: report.summary.requests.to_string(), note: "Provider attempts" }
                MetricCard { label: "Errors", value: report.summary.errors.to_string(), note: "Reported failures" }
                MetricCard { label: "Duration", value: format_duration(report.summary.duration_ms), note: "Sum of request durations" }
                MetricCard { label: "Input tokens", value: report.summary.input_tokens.to_string(), note: "Provider-reported total" }
            }
            div { class: "grid panel-grid", style: "margin-top: 1rem",
                section { class: "panel",
                    div { class: "section-heading", h2 { "Latency by request" } }
                    if records.is_empty() {
                        p { class: "helper-text", "No telemetry records were returned." }
                    } else {
                        div { class: "chart-bars",
                            for record in records.iter().rev().take(12) {
                                LatencyBar { record: record.clone(), maximum: max_duration }
                            }
                        }
                    }
                }
                section { class: "panel",
                    div { class: "section-heading", h2 { "Token and cache usage" } }
                    MetricCard { label: "Output tokens", value: report.summary.output_tokens.to_string(), note: "Provider-reported total" }
                    div { class: "session-meta",
                        span { "Cache read: {report.summary.cache_read_tokens}" }
                        span { "Cache write: {report.summary.cache_write_tokens}" }
                        span { "Cancelled: {report.summary.cancelled}" }
                    }
                }
            }
            div { class: "section-heading", h2 { "Requests ({record_count})" } }
            if records.is_empty() {
                EmptyState { title: "No activity", message: "There are no request records in this report." }
            } else {
                div { class: "record-grid",
                    for record in records {
                        TelemetryCard { record }
                    }
                }
            }
        } else {
            EmptyState {
                title: "Telemetry not loaded",
                message: "Load the server report to view request metrics and charts."
            }
        }
    }
}

#[component]
fn LatencyBar(record: TelemetryRecord, maximum: u64) -> Element {
    let height = chart_height(record.duration_ms, maximum);
    rsx! {
        div { class: "chart-bar-wrap", key: "{record.id}",
            div { class: "chart-bar", style: "height: {height}%" }
            span { class: "chart-bar-label", "{record.provider}" }
        }
    }
}

#[component]
fn TelemetryCard(record: TelemetryRecord) -> Element {
    let status_class = match record.status {
        TelemetryStatus::Ok => "status status-success",
        TelemetryStatus::Error => "status status-error",
        TelemetryStatus::Cancelled => "status status-muted",
    };
    let status_label = match record.status {
        TelemetryStatus::Ok => "OK",
        TelemetryStatus::Error => "Error",
        TelemetryStatus::Cancelled => "Cancelled",
    };
    let cost = record
        .cost_usd
        .map_or_else(|| "Not reported".to_owned(), |value| format!("${value:.4}"));
    let input = record
        .input_tokens
        .map_or_else(|| "Not reported".to_owned(), |value| value.to_string());
    let output = record
        .output_tokens
        .map_or_else(|| "Not reported".to_owned(), |value| value.to_string());
    let first_token = record
        .first_token_ms
        .map_or_else(|| "Not reported".to_owned(), |value| format!("{value} ms"));
    rsx! {
        article { class: "record-card",
            div { class: "record-head",
                div {
                    h3 { "{record.provider} / {record.model}" }
                    p { class: "mono", "{record.id}" }
                }
                span { class: status_class, "{status_label}" }
            }
            div { class: "record-meta",
                span { "Duration: {format_duration(record.duration_ms)}" }
                span { "First token: {first_token}" }
                span { "Tokens: {input} in / {output} out" }
                span { "Cost: {cost}" }
                span { "Tools: {record.tool_calls.len()}" }
            }
            if let Some(error) = record.error {
                p { class: "helper-text", "{error}" }
            }
        }
    }
}

fn load_telemetry(context: &AppContext, selected_only: bool) {
    let context = *context;
    spawn(async move {
        context.set_page_error(None);
        let Some(profile) = context.active_profile() else {
            context.set_page_error(Some("Select a server profile first.".to_owned()));
            return;
        };
        let selected_session = context.selected_session.read().clone();
        let session_id = if selected_only {
            let Some(session) = selected_session else {
                context.set_page_error(Some("Choose a session first.".to_owned()));
                return;
            };
            Some(session.session_id)
        } else {
            None
        };
        match ApiClient::new(&profile.base_url, &profile.bearer_token) {
            Ok(api) => match api.telemetry(session_id.as_deref()).await {
                Ok(report) => context.set_telemetry(Some(report)),
                Err(error) => context.set_page_error(Some(error.to_string())),
            },
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
    });
}

fn chart_height(value: u64, maximum: u64) -> u64 {
    if maximum == 0 {
        return 5;
    }
    (value.saturating_mul(100) / maximum).max(5)
}

fn format_duration(milliseconds: u64) -> String {
    if milliseconds >= 1_000 {
        format!(
            "{}.{:02} s",
            milliseconds / 1_000,
            (milliseconds % 1_000) / 10
        )
    } else {
        format!("{milliseconds} ms")
    }
}
