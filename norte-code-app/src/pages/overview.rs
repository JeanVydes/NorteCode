use crate::components::{MetricCard, PageHeader};
use crate::domain::Page;
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn OverviewPage() -> Element {
    let context = use_context::<AppContext>();
    let session_count = context.sessions.read().len();
    let agent_count = context.agents.read().len();
    let job_count = context.jobs.read().len();
    let request_count = context.telemetry.read().as_ref().map_or_else(
        || "—".to_owned(),
        |report| report.summary.requests.to_string(),
    );
    let capabilities = context
        .server_info
        .read()
        .as_ref()
        .map_or_else(Vec::new, |info| info.capabilities.clone());
    rsx! {
        PageHeader {
            eyebrow: "Workspace",
            title: "Overview",
            description: "A capability-aware view of your RAEP server, sessions, jobs, and model activity."
        }
        div { class: "grid metric-grid",
            MetricCard { label: "Sessions", value: session_count.to_string(), note: "Visible to this client" }
            MetricCard { label: "Agents", value: agent_count.to_string(), note: "Advertised by the server" }
            MetricCard { label: "Loaded jobs", value: job_count.to_string(), note: "Refresh from a session in Jobs" }
            MetricCard { label: "Requests", value: request_count, note: "Load telemetry for exact totals" }
        }
        div { class: "grid panel-grid",
            section { class: "panel",
                div { class: "section-heading",
                    h2 { "Start here" }
                    button { class: "button-secondary", onclick: move |_| context.set_page(Page::Sessions), "Browse sessions" }
                }
                p { class: "helper-text", "Create a session with an agent served by this host, then use the shared conversation timeline for live work and history." }
                div { class: "button-row", style: "margin-top: 1rem",
                    button { class: "button", onclick: move |_| context.set_page(Page::Chat), "Open conversation" }
                    button { class: "button-secondary", onclick: move |_| context.set_page(Page::Telemetry), "View telemetry" }
                }
            }
            section { class: "panel",
                div { class: "section-heading", h2 { "Server capabilities" } }
                if capabilities.is_empty() {
                    p { class: "helper-text", "Connect to a server to discover its negotiated features." }
                } else {
                    div { class: "capability-list",
                        for capability in capabilities {
                            span { class: "capability-chip", key: "{capability:?}", "{capability.label()}" }
                        }
                    }
                }
            }
        }
    }
}
