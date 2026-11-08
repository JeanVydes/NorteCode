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
    let chat_busy = context.chat_state.read().is_busy();
    let sessions = context.sessions.read().clone();
    let selected_session_id = context
        .selected_session
        .read()
        .as_ref()
        .map(|session| session.session_id.clone());
    let server_info = context.server_info.read().clone();
    let delegation_available = server_info.is_some_and(|info| {
        info.capabilities
            .contains(&crate::domain::Capability::DelegationBasic)
    });
    let profile_name = context
        .active_profile()
        .map_or_else(|| "No server selected".to_owned(), |profile| profile.name);
    let developer_mode = *context.developer_mode.read();
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
                div { class: "sidebar-server-head",
                    span { class: "sidebar-server-dot" }
                    span { class: connection.css_class(), "{connection.label()}" }
                }
                p { class: "server-name", "{profile_name}" }
            }
            nav { class: "primary-nav", "aria-label": "Main navigation",
                NavLink { page: Page::Overview, current: page, label: "Overview" }
                NavLink {
                    page: Page::Subagents,
                    current: page,
                    label: "Subagents",
                    disabled: !delegation_available,
                }
            }
            section { class: "sidebar-sessions",
                div { class: "sidebar-section-heading",
                    h2 { "Sessions" }
                    button {
                        class: "sidebar-new-session",
                        disabled: chat_busy,
                        title: "Create a session",
                        onclick: move |_| context.set_page(Page::Sessions),
                        span { "+" }
                        "New"
                    }
                }
                if sessions.is_empty() {
                    p { class: "sidebar-empty", "Connect to load your sessions." }
                } else {
                    div { class: "sidebar-session-list",
                        for session in sessions {
                            SidebarSession {
                                session: session.clone(),
                                current: selected_session_id.as_deref()
                                    == Some(session.session_id.as_str()),
                                switching_disabled: chat_busy
                                    && selected_session_id.as_deref()
                                        != Some(session.session_id.as_str()),
                            }
                        }
                    }
                }
            }
            div { class: "sidebar-footer",
                NavLink { page: Page::Telemetry, current: page, label: "Telemetry" }
                NavLink { page: Page::Settings, current: page, label: "Settings" }
                if developer_mode {
                    NavLink { page: Page::Developer, current: page, label: "Developer" }
                }
            }
        }
    }
}

#[component]
fn SidebarSession(
    session: crate::api::SessionSummary,
    current: bool,
    switching_disabled: bool,
) -> Element {
    let context = use_context::<AppContext>();
    let title = session
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Untitled session");
    let selected_page = if current && *context.page.read() == Page::Chat {
        "sidebar-session-open sidebar-session-current"
    } else {
        "sidebar-session-open"
    };
    let session_for_chat = session.clone();
    let session_for_jobs = session.clone();
    rsx! {
        article { class: "sidebar-session",
            button {
                class: selected_page,
                disabled: switching_disabled,
                title: "{title}",
                onclick: move |_| context.select_session(session_for_chat.clone(), Page::Chat),
                span { class: "sidebar-session-title", "{title}" }
                span { class: session.status.css_class(), "{session.status.label()}" }
            }
            button {
                class: "sidebar-session-jobs",
                disabled: switching_disabled,
                onclick: move |_| context.select_session(session_for_jobs.clone(), Page::Jobs),
                "Jobs"
            }
        }
    }
}

#[component]
fn NavLink(
    page: Page,
    current: Page,
    label: &'static str,
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
