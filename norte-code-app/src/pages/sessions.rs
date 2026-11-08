use crate::api::{ApiClient, SessionSummary};
use crate::components::{EmptyState, PageHeader};
use crate::domain::Page;
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn SessionsPage() -> Element {
    let context = use_context::<AppContext>();
    let mut title = use_signal(String::new);
    let mut agent_selection = use_signal(String::new);
    let agents = context.agents.read().clone();
    let sessions = context.sessions.read().clone();
    let default_agent = context
        .server_info
        .read()
        .as_ref()
        .map_or_else(String::new, |info| info.default_agent.clone());
    let chosen_agent = if agent_selection.read().is_empty() {
        default_agent
    } else {
        agent_selection.read().clone()
    };
    rsx! {
        PageHeader {
            eyebrow: "Workspace",
            title: "Sessions",
            description: "Choose an advertised agent when creating a session. Activity fields are shown only when the server reports them."
        }
        section { class: "panel",
            div { class: "section-heading", h2 { "Create a session" } }
            div { class: "form-row",
                label { class: "field",
                    span { "Session title" }
                    input {
                        value: "{title}",
                        placeholder: "A focused task name",
                        oninput: move |event| title.set(event.value())
                    }
                }
                label { class: "field",
                    span { "Agent" }
                    select {
                        value: "{chosen_agent}",
                        onchange: move |event| agent_selection.set(event.value()),
                        if agents.is_empty() {
                            option { value: "", "Connect to discover agents" }
                        }
                        for agent in agents {
                            option { value: "{agent.id}", "{format_agent(&agent)}" }
                        }
                    }
                }
            }
            div { class: "button-row", style: "margin-top: 1rem",
                button {
                    class: "button",
                    disabled: chosen_agent.is_empty(),
                    onclick: move |_| {
                        let context = context;
                        let title_value = title.read().trim().to_owned();
                        let agent_id = chosen_agent.clone();
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
                            match api.create_session(&agent_id, (!title_value.is_empty()).then_some(title_value.as_str())).await {
                                Ok(session) => {
                                    context.update_sessions(|sessions| sessions.insert(0, session.clone()));
                                    context.select_session(session, Page::Chat);
                                    title.set(String::new());
                                }
                                Err(error) => context.set_page_error(Some(error.to_string())),
                            }
                        });
                    },
                    "Create session"
                }
                button {
                    class: "button-secondary",
                    onclick: move |_| {
                        spawn(async move { crate::app::refresh_server(&context).await; });
                    },
                    "Refresh list"
                }
            }
        }
        div { class: "section-heading",
            h2 { "Your sessions" }
            span { class: "helper-text", "{sessions.len()} shown" }
        }
        if sessions.is_empty() {
            EmptyState {
                title: "No sessions yet",
                message: "Connect to a server or create a session to start a conversation."
            }
        } else {
            div { class: "session-grid",
                for session in sessions {
                    SessionCard { session }
                }
            }
        }
    }
}

#[component]
fn SessionCard(session: SessionSummary) -> Element {
    let context = use_context::<AppContext>();
    let id = session.session_id.clone();
    let session_for_open = session.clone();
    let title = session
        .title
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Untitled session".to_owned());
    let agent = session
        .agent
        .as_ref()
        .map_or("Unknown".to_owned(), |agent| agent.id.clone());
    let is_current = context
        .selected_session
        .read()
        .as_ref()
        .is_some_and(|selected| selected.session_id == session.session_id);
    let switching_disabled = context.chat_state.read().is_busy() && !is_current;
    let created = session
        .created_at
        .filter(|value| *value > 0)
        .map_or_else(|| "Creation time unavailable".to_owned(), format_timestamp);
    let turn_count = session.turn_count.map_or_else(
        || "Turn count unavailable".to_owned(),
        |count| format!("{count} turns"),
    );
    rsx! {
        article { class: "session-card",
            div { class: "session-card-head",
                div {
                    p { class: "session-title", "{title}" }
                    p { class: "session-id", "{id}" }
                }
                span { class: session.status.css_class(), "{session.status.label()}" }
            }
            div { class: "session-meta",
                span { "Agent: {agent}" }
                span { "{created}" }
                span { "{turn_count}" }
            }
            div { class: "button-row session-card-actions",
                button {
                    class: "button-secondary",
                    disabled: switching_disabled,
                    onclick: move |_| {
                        context.select_session(session_for_open.clone(), Page::Chat);
                    },
                    "Open conversation"
                }
                button {
                    class: "button-secondary",
                    disabled: switching_disabled,
                    onclick: move |_| context.select_session(session.clone(), Page::Jobs),
                    "Jobs"
                }
            }
        }
    }
}

fn format_timestamp(timestamp: i64) -> String {
    let seconds = timestamp.div_euclid(1_000);
    format!("Created at Unix time {seconds}")
}

fn format_agent(agent: &crate::api::AgentSummary) -> String {
    let version = if agent.version.is_empty() {
        "version unknown".to_owned()
    } else {
        format!("v{}", agent.version)
    };
    let provider_model = match (&agent.provider, &agent.model) {
        (Some(provider), Some(model)) => format!(" · {} / {}", provider.id, model.id),
        (Some(provider), None) => format!(" · {}", provider.id),
        _ => String::new(),
    };
    format!("{} · {}{}", agent.id, version, provider_model)
}
