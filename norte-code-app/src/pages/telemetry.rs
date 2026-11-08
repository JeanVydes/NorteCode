use crate::api::{ApiClient, TelemetryRecord, TelemetryStatus};
use crate::components::{EmptyState, MetricCard, PageHeader};
use crate::state::AppContext;
use dioxus::prelude::*;

const RECORDS_PER_PAGE: usize = 25;

#[component]
pub fn TelemetryPage() -> Element {
    let context = use_context::<AppContext>();
    let report = context.telemetry.read().clone();
    let mut selected_only = use_signal(|| false);
    let loading = use_signal(|| false);
    let mut page = use_signal(|| 0usize);
    let selected_session = context.selected_session.read().clone();
    use_effect(move || {
        let only_selected = *selected_only.read();
        let has_selected_session = context.selected_session.read().is_some();
        if !only_selected || has_selected_session {
            load_telemetry(&context, only_selected, loading, page);
        }
    });
    let records = report
        .as_ref()
        .map_or_else(Vec::new, |value| value.records.clone());
    let max_duration = records
        .iter()
        .map(|record| record.duration_ms)
        .max()
        .unwrap_or_default();
    let record_count = records.len();
    let current_page = *page.read();
    let page_count = record_count.div_ceil(RECORDS_PER_PAGE).max(1);
    let visible_records = records
        .iter()
        .rev()
        .skip(current_page.saturating_mul(RECORDS_PER_PAGE))
        .take(RECORDS_PER_PAGE)
        .cloned()
        .collect::<Vec<_>>();
    let first_record = current_page
        .saturating_mul(RECORDS_PER_PAGE)
        .saturating_add(1);
    let last_record = first_record
        .saturating_add(visible_records.len())
        .saturating_sub(1);
    let empty_title: &'static str = if *loading.read() {
        "Loading telemetry"
    } else {
        "Telemetry unavailable"
    };
    let empty_message: &'static str = if *loading.read() {
        "The host is preparing the request report."
    } else {
        "Check the connection and refresh to load provider request activity."
    };
    rsx! {
        PageHeader {
            eyebrow: "Observability",
            title: "Telemetry",
            description: "Provider request activity reported by the RAEP host. Missing token and cost values are never estimated."
        }
        div { class: "section-heading",
            div {
                h2 { "Request activity" }
                p { class: "helper-text", "Metrics include every retained request; the list below is paginated locally." }
            }
            div { class: "telemetry-controls",
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
                    disabled: *loading.read()
                        || (*selected_only.read() && selected_session.is_none()),
                    onclick: move |_| load_telemetry(&context, *selected_only.read(), loading, page),
                    if *loading.read() { "Refreshing…" } else { "Refresh" }
                }
            }
        }
        if *loading.read() {
            div { class: "loading-state", role: "status",
                span { class: "loading-indicator" }
                "Loading provider activity…"
            }
        }
        if let Some(report) = report {
            p { class: "report-scope",
                span { class: "scope-indicator" }
                "Showing {report.session.as_deref().unwrap_or(\"all sessions\")}"
            }
            div { class: "grid metric-grid",
                MetricCard { label: "Requests", value: report.summary.requests.to_string(), note: "Provider attempts" }
                MetricCard { label: "Errors", value: report.summary.errors.to_string(), note: "Reported failures" }
                MetricCard { label: "Duration", value: format_duration(report.summary.duration_ms), note: "Sum of request durations" }
                MetricCard { label: "Input tokens", value: report.summary.input_tokens.to_string(), note: "Provider-reported total" }
            }
            div { class: "grid panel-grid telemetry-panels",
                section { class: "panel",
                    div { class: "section-heading",
                        div {
                            h2 { "Recent latency" }
                            p { class: "helper-text", "Newest 12 requests · duration per provider call" }
                        }
                    }
                    if records.is_empty() {
                        EmptyState {
                            title: "No request samples yet",
                            message: "Latency appears here after the host records provider requests."
                        }
                    } else {
                        div { class: "chart-bars",
                            for record in records.iter().rev().take(12) {
                                LatencyBar { record: record.clone(), maximum: max_duration }
                            }
                        }
                    }
                }
                section { class: "panel",
                    div { class: "section-heading",
                        div {
                            h2 { "Token and cache usage" }
                            p { class: "helper-text", "Provider-reported totals" }
                        }
                    }
                    MetricCard { label: "Output tokens", value: report.summary.output_tokens.to_string(), note: "Provider-reported total" }
                    div { class: "session-meta",
                        span { "Cache read: {report.summary.cache_read_tokens}" }
                        span { "Cache write: {report.summary.cache_write_tokens}" }
                        span { "Cancelled: {report.summary.cancelled}" }
                    }
                }
            }
            div { class: "section-heading telemetry-record-heading",
                div {
                    h2 { "Recent requests" }
                    p { class: "helper-text", "{record_count} retained · newest first" }
                }
                if record_count > 0 {
                    span { class: "page-indicator", "{first_record}–{last_record} of {record_count}" }
                }
            }
            if records.is_empty() {
                EmptyState { title: "No activity yet", message: "Provider requests will appear here after a session runs." }
            } else {
                div { class: "record-grid",
                    for record in visible_records {
                        TelemetryCard { record }
                    }
                }
                div { class: "telemetry-pagination",
                    button {
                        class: "button-secondary",
                        disabled: current_page == 0,
                        onclick: move |_| page.set(current_page.saturating_sub(1)),
                        "Previous"
                    }
                    span { class: "page-indicator", "Page {current_page + 1} of {page_count}" }
                    button {
                        class: "button-secondary",
                        disabled: current_page + 1 >= page_count,
                        onclick: move |_| page.set((current_page + 1).min(page_count - 1)),
                        "Next"
                    }
                }
            }
        } else {
            EmptyState {
                title: empty_title,
                message: empty_message,
            }
        }
    }
}

#[component]
fn LatencyBar(record: TelemetryRecord, maximum: u64) -> Element {
    let height = chart_height(record.duration_ms, maximum);
    rsx! {
        div { class: "chart-bar-wrap", key: "{record.id}",
            div { class: "chart-bar-track",
                div {
                    class: "chart-bar",
                    title: "{record.provider} · {format_duration(record.duration_ms)}",
                    style: "height: {height}%"
                }
            }
            span { class: "chart-bar-label", "{record.provider}" }
            span { class: "chart-bar-value", "{format_duration(record.duration_ms)}" }
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

fn load_telemetry(
    context: &AppContext,
    selected_only: bool,
    mut loading: Signal<bool>,
    mut page: Signal<usize>,
) {
    let context = *context;
    let session_id = if selected_only {
        let Some(session) = context.selected_session.read().clone() else {
            context.set_page_error(Some("Choose a session first.".to_owned()));
            return;
        };
        Some(session.session_id)
    } else {
        None
    };
    loading.set(true);
    spawn(async move {
        context.set_page_error(None);
        let Some(profile) = context.active_profile() else {
            context.set_page_error(Some("Select a server profile first.".to_owned()));
            loading.set(false);
            return;
        };
        let api = match ApiClient::new(&profile.base_url, &profile.bearer_token) {
            Ok(api) => api,
            Err(error) => {
                context.set_page_error(Some(error.to_string()));
                loading.set(false);
                return;
            }
        };
        match api.telemetry(session_id.as_deref()).await {
            Ok(report) => {
                context.set_telemetry(Some(report));
                page.set(0);
            }
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
        loading.set(false);
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
