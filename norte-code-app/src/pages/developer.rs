use crate::api::{ApiClient, Liveness, Readiness, ServerInfoExt};
use crate::components::{EmptyState, MetricCard, PageHeader};
use crate::state::AppContext;
use dioxus::prelude::*;
use norte_code_wire::QuotaReport;

#[component]
pub fn DeveloperPage() -> Element {
    let context = use_context::<AppContext>();
    let liveness = use_signal(|| None::<Liveness>);
    let readiness = use_signal(|| None::<Readiness>);
    let quota = use_signal(|| None::<QuotaReport>);
    let loading = use_signal(|| false);
    let enabled = *context.developer_mode.read();
    let server_info = context.server_info.read().clone();
    let profile = context.active_profile();
    let profile_name = profile
        .as_ref()
        .map_or_else(|| "Unknown".to_owned(), |item| item.name.clone());
    let profile_url = profile
        .as_ref()
        .map_or_else(|| "Unknown".to_owned(), |item| item.base_url.clone());
    use_effect(move || {
        if *context.developer_mode.read() {
            refresh_diagnostics(&context, liveness, readiness, quota, loading);
        }
    });
    rsx! {
        PageHeader {
            eyebrow: "Diagnostics",
            title: "Developer",
            description: "A separate diagnostics view for server capabilities and limits. It never displays authentication material."
        }
        if !enabled {
            EmptyState {
                title: "Developer mode is off",
                message: "Enable Developer mode in Settings to expose diagnostics."
            }
        } else {
            section { class: "panel",
                div { class: "section-heading",
                    h2 { "Host health and quota" }
                    button {
                        class: "button-secondary",
                        disabled: *loading.read(),
                        onclick: move |_| refresh_diagnostics(&context, liveness, readiness, quota, loading),
                        if *loading.read() { "Refreshing…" } else { "Refresh diagnostics" }
                    }
                }
                if let Some(live) = liveness.read().as_ref() {
                    p { class: "helper-text", "Liveness: {health_label(live.status)}" }
                }
                if let Some(ready) = readiness.read().as_ref() {
                    p { class: "helper-text",
                        "Readiness: {health_label(ready.status)} · agent {ready.agent_id} · {ready.agents} configured agent(s)"
                    }
                    p { class: "helper-text",
                        "Replay store: {enabled_label(ready.replay)} · Artifact store: {enabled_label(ready.artifacts)} · maximum connections: {ready.limits.max_connections} · command timeout: {ready.limits.command_timeout_ms} ms"
                    }
                }
                if let Some(quota) = quota.read().as_ref() {
                    p { class: "helper-text",
                        "Principal: {quota.tenant}. These are host-local counters and configured limits, not a billing or tenant quota service."
                    }
                    div { class: "metric-grid",
                        for dimension in &quota.dimensions {
                            MetricCard {
                                label: dimension.name.label(),
                                value: format!(
                                    "{} / {} · {} remaining",
                                    dimension.used, dimension.limit, dimension.remaining
                                ),
                                note: "Host-reported usage against its configured limit"
                            }
                        }
                    }
                } else {
                    p { class: "helper-text",
                        "Quota counters are not loaded. This endpoint requires any configured server token."
                    }
                }
            }
            if let Some(info) = server_info {
            div { class: "grid panel-grid",
                section { class: "panel",
                    div { class: "section-heading", h2 { "Connection" } }
                    dl { class: "record-meta",
                        dt { "Profile" }
                        dd { "{profile_name}" }
                        dt { "Server URL" }
                        dd { class: "mono", "{profile_url}" }
                        dt { "Server version" }
                        dd { "{info.server_version}" }
                        dt { "Protocol" }
                        dd { "{protocol_label(info.protocol.as_ref())}" }
                    }
                }
                section { class: "panel",
                    div { class: "section-heading", h2 { "Effective limits" } }
                    dl { class: "record-meta",
                        dt { "Maximum frame bytes" }
                        dd { "{format_option(info.limits.frame_bytes)}" }
                        dt { "Concurrent jobs" }
                        dd { "{format_option(info.limits.concurrent_jobs)}" }
                        dt { "Maximum artifact bytes" }
                        dd { "{format_option(info.limits.artifact_bytes)}" }
                        dt { "Maximum prompt bytes" }
                        dd { "{format_option(info.limits.prompt_bytes)}" }
                    }
                }
            }
            section { class: "panel", style: "margin-top: 1rem",
                div { class: "section-heading", h2 { "Negotiated capabilities" } }
                div { class: "capability-list",
                    for capability in info.decoded_capabilities() {
                        span { class: "capability-chip", key: "{capability:?}", "{capability.label()}" }
                    }
                }
                if !info.extensions.is_empty() {
                    div { class: "section-heading", h2 { "Extensions" } }
                    p { class: "mono", "{info.extensions.join(\", \")}" }
                }
            }
            } else {
                EmptyState {
                    title: "No server diagnostics",
                    message: "Connect to a RAEP host to inspect its advertised information."
                }
            }
        }
    }
}

fn refresh_diagnostics(
    context: &AppContext,
    mut liveness: Signal<Option<Liveness>>,
    mut readiness: Signal<Option<Readiness>>,
    mut quota: Signal<Option<QuotaReport>>,
    mut loading: Signal<bool>,
) {
    let context = *context;
    loading.set(true);
    spawn(async move {
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

        let mut errors = Vec::new();
        match api.health_live().await {
            Ok(value) => liveness.set(Some(value)),
            Err(error) => errors.push(format!("Liveness: {error}")),
        }
        match api.health_ready().await {
            Ok(value) => readiness.set(Some(value)),
            Err(error) => errors.push(format!("Readiness: {error}")),
        }
        match api.quota().await {
            Ok(value) => quota.set(Some(value)),
            Err(error) => errors.push(format!("Quota: {error}")),
        }
        context.set_page_error((!errors.is_empty()).then(|| errors.join("\n")));
        loading.set(false);
    });
}

fn health_label(status: crate::api::HealthStatus) -> &'static str {
    match status {
        crate::api::HealthStatus::Live => "Live",
        crate::api::HealthStatus::Ready => "Ready",
    }
}

fn enabled_label(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
}

fn format_option(value: Option<u64>) -> String {
    value.map_or_else(|| "Not reported".to_owned(), |value| value.to_string())
}

fn protocol_label(protocol: Option<&crate::api::ProtocolInfo>) -> String {
    protocol.map_or_else(
        || "Unknown".to_owned(),
        |value| format!("{} {}", value.name, value.version),
    )
}
