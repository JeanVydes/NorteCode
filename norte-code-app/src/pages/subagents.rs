use crate::api::{ApiClient, ChildSessionState, ChildSessionSummary};
use crate::components::{EmptyState, PageHeader};
use crate::domain::Capability;
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn SubagentsPage() -> Element {
    let context = use_context::<AppContext>();
    let children = use_signal(|| None::<(String, Vec<ChildSessionSummary>)>);
    let supported = context
        .server_info
        .read()
        .as_ref()
        .is_some_and(|info| info.capabilities.contains(&Capability::DelegationBasic));
    let selected = context.selected_session.read().clone();
    let child_sessions = selected.as_ref().and_then(|session| {
        children
            .read()
            .as_ref()
            .filter(|(parent_id, _)| parent_id == &session.session_id)
            .map(|(_, children)| children.clone())
    });
    rsx! {
        PageHeader {
            eyebrow: "Delegation",
            title: "Subagents",
            description: "Inspect child sessions spawned by the selected parent session."
        }
        if !supported {
            EmptyState {
                title: "Delegation is unavailable",
                message: "The selected host does not advertise delegation, so child-session inspection is disabled."
            }
        } else if let Some(session) = selected {
            div { class: "section-heading",
                h2 { "{session.title.as_deref().unwrap_or(\"Untitled session\")}" }
                button {
                    class: "button-secondary",
                    onclick: move |_| refresh_children(&context, children, session.session_id.clone()),
                    "Refresh children"
                }
            }
            if child_sessions.as_ref().is_none_or(Vec::is_empty) {
                EmptyState {
                    title: "No child sessions loaded",
                    message: "Refresh to inspect the child sessions spawned by this parent."
                }
            } else {
                div { class: "session-grid",
                    for child in child_sessions.unwrap_or_default() {
                        ChildSessionCard { child }
                    }
                }
            }
        } else {
            EmptyState {
                title: "Choose a parent session",
                message: "Select a session first to inspect its child sessions."
            }
        }
    }
}

#[component]
fn ChildSessionCard(child: ChildSessionSummary) -> Element {
    let title = child
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Untitled child session");
    let state = match child.state {
        ChildSessionState::Idle => "Idle",
        ChildSessionState::Streaming => "Running",
        ChildSessionState::AwaitingTool => "Awaiting tool",
        ChildSessionState::Failed => "Failed",
        ChildSessionState::Stopped => "Stopped",
    };
    rsx! {
        article { class: "session-card",
            div { class: "session-card-head",
                div {
                    p { class: "session-title", "{title}" }
                    p { class: "session-id", "{child.session_id}" }
                }
                span { class: "status status-muted", "{state}" }
            }
        }
    }
}

fn refresh_children(
    context: &AppContext,
    mut children: Signal<Option<(String, Vec<ChildSessionSummary>)>>,
    parent_session_id: String,
) {
    let context = *context;
    children.set(None);
    spawn(async move {
        context.set_page_error(None);
        let Some(profile) = context.active_profile() else {
            context.set_page_error(Some("Select a server profile first.".to_owned()));
            return;
        };
        let api = match ApiClient::new(&profile.base_url, &profile.bearer_token) {
            Ok(api) => api,
            Err(error) => {
                context.set_page_error(Some(error.to_string()));
                return;
            }
        };
        match api.child_sessions(&parent_session_id).await {
            Ok(result) => children.set(Some((parent_session_id, result))),
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
    });
}
