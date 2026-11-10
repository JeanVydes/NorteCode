use crate::api::{ApiClient, CostReport};
use crate::components::{MetricCard, PageHeader};
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn CostsPage() -> Element {
    let context = use_context::<AppContext>();
    let global_report = use_signal(|| None::<CostReport>);
    let session_report = use_signal(|| None::<CostReport>);
    let loading = use_signal(|| false);
    use_effect(move || refresh_costs(&context, global_report, session_report, loading));

    let global = global_report.read().clone();
    let session = session_report.read().clone();
    let selected = context.selected_session.read().clone();
    rsx! {
        PageHeader {
            eyebrow: "Usage",
            title: "Costs",
            description: "View provider-reported spend from the host's usage ledger. Missing provider cost is shown as unreported, never as zero."
        }
        div { class: "section-heading",
            h2 { "Cost ledger" }
            button {
                class: "button-secondary",
                disabled: *loading.read(),
                onclick: move |_| refresh_costs(&context, global_report, session_report, loading),
                if *loading.read() { "Loading…" } else { "Refresh costs" }
            }
        }
        div { class: "metric-grid",
            MetricCard {
                label: "All sessions",
                value: global.as_ref().map_or_else(
                    || "Not reported".to_owned(),
                    |report| format_cost(report.total_usd),
                ),
                note: "Total reported by the host ledger"
            }
            MetricCard {
                label: "Requests counted",
                value: global.as_ref().map_or_else(
                    || "-".to_owned(),
                    |report| report.requests.to_string(),
                ),
                note: "Provider requests in the ledger"
            }
            MetricCard {
                label: "Selected session",
                value: session.as_ref().map_or_else(
                    || if selected.is_some() { "Not reported".to_owned() } else { "No session".to_owned() },
                    |report| format_cost(report.total_usd),
                ),
                note: "Session-scoped provider cost"
            }
            MetricCard {
                label: "Session requests",
                value: session.as_ref().map_or_else(
                    || "-".to_owned(),
                    |report| report.requests.to_string(),
                ),
                note: "Requests attributed to this session"
            }
        }
        if global.is_none() {
            div { class: "notice notice-info", style: "margin-top: 1rem",
                p { "No global cost report is loaded. Refresh to query the host." }
            }
        }
        if let Some(session) = selected {
            p { class: "helper-text", style: "margin-top: 1rem",
                "Session: {session.title.as_deref().unwrap_or(\"Untitled session\")} · {session.session_id}"
            }
        }
    }
}

fn refresh_costs(
    context: &AppContext,
    mut global_report: Signal<Option<CostReport>>,
    mut session_report: Signal<Option<CostReport>>,
    mut loading: Signal<bool>,
) {
    loading.set(true);
    let context = *context;
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
        match api.cost_report(None).await {
            Ok(report) => global_report.set(Some(report)),
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
        let selected = context.selected_session.read().clone();
        if let Some(session) = selected {
            let agent_id = session.agent.map_or_else(String::new, |agent| agent.id);
            if agent_id.trim().is_empty() {
                context.set_page_error(Some(
                    "The selected session has no agent id for cost reporting.".to_owned(),
                ));
            } else {
                match api.attach(&session.session_id, &agent_id).await {
                    Ok(()) => {
                        let report = api.cost_report(Some(&session.session_id)).await;
                        let detach = api.detach(&session.session_id).await;
                        match (report, detach) {
                            (Ok(report), Ok(())) => session_report.set(Some(report)),
                            (Err(error), _) | (Ok(_), Err(error)) => {
                                context.set_page_error(Some(error.to_string()));
                            }
                        }
                    }
                    Err(error) => context.set_page_error(Some(error.to_string())),
                }
            }
        } else {
            session_report.set(None);
        }
        loading.set(false);
    });
}

fn format_cost(value: Option<f64>) -> String {
    value.map_or_else(
        || "Not reported".to_owned(),
        |amount| format!("${amount:.4}"),
    )
}
