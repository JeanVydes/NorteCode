use crate::domain::{ConnectionState, Page};
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn PageHeader(
    eyebrow: &'static str,
    title: &'static str,
    description: &'static str,
) -> Element {
    rsx! {
        header { class: "page-header",
            p { class: "eyebrow", "{eyebrow}" }
            h1 { "{title}" }
            p { class: "page-description", "{description}" }
        }
    }
}

#[component]
pub fn MetricCard(label: &'static str, value: String, note: &'static str) -> Element {
    rsx! {
        article { class: "metric-card",
            p { class: "metric-label", "{label}" }
            p { class: "metric-value", "{value}" }
            p { class: "metric-note", "{note}" }
        }
    }
}

#[component]
pub fn EmptyState(title: &'static str, message: &'static str) -> Element {
    rsx! {
        div { class: "empty-state",
            span { class: "empty-mark", "—" }
            h3 { "{title}" }
            p { "{message}" }
        }
    }
}

#[component]
pub fn Sidebar() -> Element {
    let context = use_context::<AppContext>();
    let page = *context.page.read();
    let connection = *context.connection.read();
    let server_info = context.server_info.read().clone();
    let delegation_available = server_info.is_some_and(|info| {
        info.capabilities
            .contains(&crate::domain::Capability::Delegation)
    });
    let profile_name = context
        .active_profile()
        .map_or_else(|| "No server selected".to_owned(), |profile| profile.name);
    rsx! {
        aside { class: "sidebar",
            a { class: "brand", href: "#overview", onclick: move |_| context.set_page(Page::Overview),
                span { class: "brand-mark", "N" }
                span {
                    strong { "Norte Code" }
                    small { "RAEP workspace" }
                }
            }
            div { class: "sidebar-server",
                span { class: connection.css_class(), "{connection.label()}" }
                p { class: "server-name", "{profile_name}" }
            }
            nav { class: "primary-nav", "aria-label": "Main navigation",
                NavLink { page: Page::Overview, current: page, label: "Overview", marker: "OV" }
                NavLink { page: Page::Sessions, current: page, label: "Sessions", marker: "SE" }
                NavLink { page: Page::Chat, current: page, label: "Conversation", marker: "CH" }
                NavLink { page: Page::Jobs, current: page, label: "Jobs", marker: "JB" }
                NavLink {
                    page: Page::Subagents,
                    current: page,
                    label: "Subagents",
                    marker: "SA",
                    disabled: !delegation_available,
                }
                NavLink { page: Page::Telemetry, current: page, label: "Telemetry", marker: "TE" }
            }
            div { class: "sidebar-footer",
                NavLink { page: Page::Settings, current: page, label: "Settings", marker: "ST" }
                NavLink { page: Page::Developer, current: page, label: "Developer", marker: "DV" }
                p { "Capabilities negotiated from the selected server." }
            }
        }
    }
}

#[component]
fn NavLink(
    page: Page,
    current: Page,
    label: &'static str,
    marker: &'static str,
    #[props(default = false)] disabled: bool,
) -> Element {
    let context = use_context::<AppContext>();
    let current_class = if page == current {
        "nav-link nav-link-current"
    } else {
        "nav-link"
    };
    rsx! {
        button {
            class: current_class,
            disabled,
            onclick: move |_| context.set_page(page),
            span { class: "nav-marker", "{marker}" }
            span { "{label}" }
            if disabled {
                span { class: "nav-hint", "Unavailable" }
            }
        }
    }
}

#[component]
pub fn PageError() -> Element {
    let context = use_context::<AppContext>();
    let error = context.page_error.read().clone();
    if let Some(error) = error {
        rsx! {
            div { class: "notice notice-error", role: "alert",
                strong { "Request failed" }
                p { "{error}" }
            }
        }
    } else {
        rsx! {}
    }
}

#[component]
pub fn ConnectionBadge(state: ConnectionState) -> Element {
    rsx! {
        span { class: state.css_class(), "{state.label()}" }
    }
}
