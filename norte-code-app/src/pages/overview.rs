use crate::components::{MetricCard, PageHeader};
use crate::domain::Page;
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn OverviewPage() -> Element {
    let context = use_context::<AppContext>();
    let session_count = context.sessions.read().len();
    let agent_count = context.agents.read().len();
    let request_count = context.telemetry.read().as_ref().map_or_else(
        || "—".to_owned(),
        |report| report.summary.requests.to_string(),
    );
    rsx! {
        PageHeader { eyebrow: "", title: "Overview", description: "" }
        div { class: "grid metric-grid",
            MetricCard { label: "Sessions", value: session_count.to_string(), note: "" }
            MetricCard { label: "Agents", value: agent_count.to_string(), note: "" }
            MetricCard { label: "Requests", value: request_count, note: "" }
        }
        div { class: "button-row", style: "margin-top: 1.5rem",
            button {
                class: "button",
                onclick: move |_| context.set_page(Page::Sessions),
                "Sessions"
            }
            button {
                class: "button-secondary",
                onclick: move |_| context.set_page(Page::Telemetry),
                "Telemetry"
            }
        }
    }
}
