use crate::domain::{ConnectionState, Page};
use crate::state::AppContext;
use dioxus::prelude::*;

mod markdown;
mod message;

pub(crate) use markdown::render_safe_markdown as render_markdown;
pub use message::{JobRow, MessageItem, TodoPanel};

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
            if !description.is_empty() {
                p { class: "page-description", "{description}" }
            }
        }
    }
}

#[component]
pub fn MetricCard(label: &'static str, value: String, note: &'static str) -> Element {
    rsx! {
        article { class: "metric-card",
            p { class: "metric-label", "{label}" }
            p { class: "metric-value", "{value}" }
            if !note.is_empty() {
                p { class: "metric-note", "{note}" }
            }
        }
    }
}

#[component]
pub fn EmptyState(title: &'static str, message: &'static str) -> Element {
    rsx! {
        div { class: "empty-state",
            h3 { "{title}" }
            if !message.is_empty() {
                p { "{message}" }
            }
        }
    }
}

#[component]
pub fn Sidebar() -> Element {
    let context = use_context::<AppContext>();
    let page = *context.page.read();
    let connection = *context.connection.read();
    let sessions = context.sessions.read().clone();
    // A subagent is not a session of its own in the sidebar: it belongs under
    // the session that spawned it, collapsed until asked for. Left at the top
    // level, a busy parent pushed a dozen indented children into the list and
    // the conversation people wanted scrolled away.
    let activity = |session: &norte_code_wire::SessionSummary| {
        session
            .last_activity_at
            .or(session.updated_at)
            .or(session.created_at)
            .unwrap_or(0)
    };
    let known: std::collections::HashSet<&str> = sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect();
    let is_child = |session: &norte_code_wire::SessionSummary| {
        session
            .parent_session_id
            .as_deref()
            .is_some_and(|parent| known.contains(parent))
    };
    let mut roots: Vec<&norte_code_wire::SessionSummary> = sessions
        .iter()
        .filter(|session| !is_child(session))
        .collect();
    roots.sort_by_key(|session| std::cmp::Reverse(activity(session)));
    let mut children: std::collections::BTreeMap<&str, Vec<&norte_code_wire::SessionSummary>> =
        std::collections::BTreeMap::new();
    for session in sessions.iter().filter(|session| is_child(session)) {
        if let Some(parent) = session.parent_session_id.as_deref() {
            children.entry(parent).or_default().push(session);
        }
    }
    for group in children.values_mut() {
        group.sort_by_key(|session| std::cmp::Reverse(activity(session)));
    }
    let loading_session = context.streaming_turn_id.read().clone();
    let selected_session_id = context
        .selected_session
        .read()
        .as_ref()
        .map(|session| session.session_id.clone());
    let profile_name = context
        .active_profile()
        .map_or_else(|| "No server".to_owned(), |profile| profile.name);
    let developer_mode = *context.developer_mode.read();
    rsx! {
        aside { class: "sidebar",
            button { class: "brand", onclick: move |_| context.set_page(Page::Overview),
                span { class: "brand-mark", "N" }
                span { class: "brand-text", "Norte Code" }
            }
            div { class: "sidebar-server",
                span { class: connection.dot_class() }
                span { class: "server-name", "{profile_name}" }
            }
            nav { class: "primary-nav", "aria-label": "Main navigation",
                NavLink { page: Page::Overview, current: page, label: "Overview" }
                NavLink { page: Page::Providers, current: page, label: "Providers" }
                NavLink { page: Page::Plugins, current: page, label: "Plugins" }
                NavLink { page: Page::HostCapabilities, current: page, label: "Host" }
            }
            section { class: "sidebar-sessions",
                div { class: "sidebar-section-heading",
                    h2 { "Sessions" }
                    button {
                        class: "sidebar-new-session",
                        title: "New session",
                        onclick: move |_| context.set_page(Page::Sessions),
                        "+"
                    }
                }
                if sessions.is_empty() {
                    p { class: "sidebar-empty", "No sessions yet." }
                } else {
                    div { class: "sidebar-session-list",
                        for session in roots {
                            SidebarSession {
                                session: session.clone(),
                                current: selected_session_id.as_deref()
                                    == Some(session.session_id.as_str()),
                                working: is_working(session, selected_session_id.as_ref(), loading_session.as_ref()),
                                switching_disabled: false,
                                subagents: children
                                    .get(session.session_id.as_str())
                                    .map(|group| group.iter().map(|child| (*child).clone()).collect())
                                    .unwrap_or_default(),
                                selected: selected_session_id.clone(),
                                loading_session: loading_session.clone(),
                            }
                        }
                    }
                }
            }
            div { class: "sidebar-footer",
                NavLink { page: Page::Costs, current: page, label: "Costs" }
                NavLink { page: Page::Telemetry, current: page, label: "Telemetry" }
                NavLink { page: Page::Settings, current: page, label: "Settings" }
                if developer_mode {
                    NavLink { page: Page::Developer, current: page, label: "Developer" }
                }
            }
        }
    }
}

fn is_working(
    session: &norte_code_wire::SessionSummary,
    selected: Option<&String>,
    loading: Option<&String>,
) -> bool {
    // A turn is running if this page is driving one, or if the host says the
    // session has a pending turn (another client, a child, a queued message).
    session.has_pending.unwrap_or(false)
        || (loading.is_some() && selected.map(String::as_str) == Some(session.session_id.as_str()))
}

#[component]
fn SidebarSession(
    session: norte_code_wire::SessionSummary,
    current: bool,
    working: bool,
    switching_disabled: bool,
    subagents: Vec<norte_code_wire::SessionSummary>,
    selected: Option<String>,
    loading_session: Option<String>,
) -> Element {
    let context = use_context::<AppContext>();
    let title = session
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Untitled session");
    let class = if current {
        "sidebar-session sidebar-session-current"
    } else {
        "sidebar-session"
    };
    let session_for_chat = session.clone();
    // A parent with children can be opened to show them; it is open by default
    // only when the child currently on screen belongs to it, so the list does
    // not lose the place you are in.
    let has_children = !subagents.is_empty();
    let child_selected = subagents
        .iter()
        .any(|child| selected.as_deref() == Some(child.session_id.as_str()));
    let mut open = use_signal(|| child_selected);
    rsx! {
        div { class: "sidebar-group",
            div { class: "sidebar-session-row",
                if has_children {
                    button {
                        class: "sidebar-caret",
                        title: if *open.read() { "Hide subagents" } else { "Show subagents" },
                        onclick: move |_| open.toggle(),
                        if *open.read() { "▾" } else { "▸" }
                    }
                }
                button {
                    class,
                    disabled: switching_disabled,
                    title: "{title}",
                    onclick: move |_| context.select_session(session_for_chat.clone(), Page::Chat),
                    if working {
                        span { class: "sidebar-spinner", "aria-hidden": "true" }
                    }
                    span { class: "sidebar-session-title", "{title}" }
                    if has_children {
                        span { class: "sidebar-child-count", "{subagents.len()}" }
                    }
                }
            }
            if has_children && (*open.read() || child_selected) {
                div { class: "sidebar-children",
                    for child in subagents {
                        SidebarSession {
                            session: child.clone(),
                            current: selected.as_deref() == Some(child.session_id.as_str()),
                            working: is_working(&child, selected.as_ref(), loading_session.as_ref()),
                            switching_disabled: false,
                            subagents: Vec::new(),
                            selected: selected.clone(),
                            loading_session: loading_session.clone(),
                        }
                    }
                }
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
        }
    }
}

#[component]
pub fn PageError() -> Element {
    let context = use_context::<AppContext>();
    let error = context.page_error.read().clone();
    let displaced = *context.chat_state.read() == crate::domain::ChatState::Displaced;
    if let Some(error) = error {
        rsx! {
            div { class: "notice notice-error", role: "alert",
                p { "{error}" }
                if displaced {
                    button {
                        class: "button-secondary",
                        title: "Attach this tab and disconnect the other one",
                        onclick: move |_| {
                            // Bumping the epoch reopens the stream, which now
                            // takes over instead of being refused.
                            context.set_page_error(None);
                            context.set_chat_state(crate::domain::ChatState::LoadingHistory);
                            context.bump_conversation_epoch();
                        },
                        "Take over here"
                    }
                }
                button {
                    class: "link-button",
                    onclick: move |_| context.set_page_error(None),
                    "Dismiss"
                }
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
