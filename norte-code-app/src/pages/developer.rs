use crate::components::{EmptyState, PageHeader};
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn DeveloperPage() -> Element {
    let context = use_context::<AppContext>();
    let enabled = *context.developer_mode.read();
    let server_info = context.server_info.read().clone();
    let profile = context.active_profile();
    let profile_name = profile
        .as_ref()
        .map_or_else(|| "Unknown".to_owned(), |item| item.name.clone());
    let profile_url = profile
        .as_ref()
        .map_or_else(|| "Unknown".to_owned(), |item| item.base_url.clone());
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
        } else if let Some(info) = server_info {
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
                    for capability in info.capabilities {
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

fn format_option(value: Option<u64>) -> String {
    value.map_or_else(|| "Not reported".to_owned(), |value| value.to_string())
}

fn protocol_label(protocol: Option<&crate::api::ProtocolInfo>) -> String {
    protocol.map_or_else(
        || "Unknown".to_owned(),
        |value| format!("{} {}", value.name, value.version),
    )
}
