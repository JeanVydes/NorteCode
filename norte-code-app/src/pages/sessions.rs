use crate::api::ApiClient;
use crate::components::{EmptyState, PageHeader};
use crate::domain::Page;
use crate::state::AppContext;
use dioxus::prelude::*;
use norte_code_wire::{ModelInfo as ModelSummary, SessionSummary};

#[component]
pub fn SessionsPage() -> Element {
    let context = use_context::<AppContext>();
    let mut title = use_signal(String::new);
    let mut agent_selection = use_signal(String::new);
    let mut model_selection = use_signal(String::new);
    let mut models = use_signal(Vec::<ModelSummary>::new);
    let creating = use_signal(|| false);
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
    let catalogue = models.read().clone();

    use_effect(move || {
        spawn(async move {
            let Some(client) = client(&context) else {
                return;
            };
            if let Ok(list) = client.models().await {
                models.set(list);
            }
        });
    });

    rsx! {
        PageHeader { eyebrow: "", title: "Sessions", description: "" }
        section { class: "panel",
            div { class: "form-row",
                label { class: "field",
                    span { "Title" }
                    input {
                        value: "{title}",
                        placeholder: "Optional",
                        oninput: move |event| title.set(event.value())
                    }
                }
                label { class: "field",
                    span { "Agent" }
                    select {
                        value: "{chosen_agent}",
                        onchange: move |event| agent_selection.set(event.value()),
                        if agents.is_empty() {
                            option { value: "", "No agents advertised" }
                        }
                        for agent in &agents {
                            option { value: "{agent.id}", "{agent_label(agent)}" }
                        }
                    }
                }
                label { class: "field",
                    span { "Model" }
                    select {
                        value: "{model_selection}",
                        onchange: move |event| model_selection.set(event.value()),
                        option { value: "", "Agent default" }
                        for model in &catalogue {
                            option {
                                value: "{model.provider}\u{1f}{model.model}",
                                "{model.provider} / {model.name}"
                            }
                        }
                    }
                }
            }
            div { class: "button-row", style: "margin-top: 1rem",
                button {
                    class: "button",
                    disabled: chosen_agent.is_empty() || *creating.read(),
                    onclick: {
                        let agent_id = chosen_agent.clone();
                        move |_| {
                            let title_value = title.read().trim().to_owned();
                            let agent_id = agent_id.clone();
                            let model = model_selection.read().clone();
                            create(context, agent_id, title_value, model, creating);
                        }
                    },
                    if *creating.read() { "Creating…" } else { "Create session" }
                }
                button {
                    class: "button-secondary",
                    onclick: move |_| {
                        spawn(async move { crate::app::refresh_server(&context).await; });
                    },
                    "Refresh"
                }
            }
        }
        if sessions.is_empty() {
            EmptyState { title: "No sessions", message: "" }
        } else {
            div { class: "session-grid", style: "margin-top: 1rem",
                for session in sessions {
                    SessionCard { session }
                }
            }
        }
    }
}

#[allow(clippy::large_types_passed_by_value, clippy::needless_pass_by_value)]
fn create(
    context: AppContext,
    agent_id: String,
    title: String,
    model: String,
    mut creating: Signal<bool>,
) {
    let Some(client) = client(&context) else {
        return;
    };
    let override_pair = model
        .split_once('\u{1f}')
        .map(|(provider, model)| (provider.to_owned(), model.to_owned()));
    creating.set(true);
    spawn(async move {
        context.set_page_error(None);
        let created = match override_pair {
            Some((provider, model)) => {
                client
                    .create_session_with_model(
                        &agent_id,
                        (!title.is_empty()).then_some(title.as_str()),
                        Some((provider.as_str(), model.as_str())),
                    )
                    .await
            }
            None => {
                client
                    .create_session(&agent_id, (!title.is_empty()).then_some(title.as_str()))
                    .await
            }
        };
        match created {
            Ok(session) => {
                context.update_sessions(|sessions| sessions.insert(0, session.clone()));
                context.select_session(session, Page::Chat);
            }
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
        creating.set(false);
    });
}

fn client(context: &AppContext) -> Option<ApiClient> {
    let profile = context.active_profile()?;
    match ApiClient::new(&profile.base_url, &profile.bearer_token) {
        Ok(client) => Some(client),
        Err(error) => {
            context.set_page_error(Some(error.to_string()));
            None
        }
    }
}

#[component]
fn SessionCard(session: SessionSummary) -> Element {
    let context = use_context::<AppContext>();
    let mut title = use_signal(|| session.title.clone().unwrap_or_default());
    let mut renaming = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let session_id = session.session_id.clone();
    let session_for_open = session.clone();
    let shown_title = session
        .title
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Untitled".to_owned());
    rsx! {
        article { class: "session-card",
            div { class: "session-card-head",
                div {
                    p { class: "session-title", "{shown_title}" }
                    p { class: "session-id", "{session_id}" }
                }
                if session.status.is_suspended() {
                    span { class: "status status-muted", "{session.status.label()}" }
                }
            }
            div { class: "button-row session-card-actions",
                button {
                    class: "button-secondary",
                    onclick: move |_| context.select_session(session_for_open.clone(), Page::Chat),
                    "Open"
                }
                button {
                    class: "link-button",
                    onclick: move |_| {
                        title.set(session.title.clone().unwrap_or_default());
                        renaming.set(true);
                        confirm_delete.set(false);
                    },
                    "Rename"
                }
                if *confirm_delete.read() {
                    button {
                        class: "button-danger",
                        onclick: {
                            let session_id = session_id.clone();
                            move |_| delete(context, session_id.clone())
                        },
                        "Confirm delete"
                    }
                    button { class: "link-button", onclick: move |_| confirm_delete.set(false), "Cancel" }
                } else {
                    button {
                        class: "link-button",
                        onclick: move |_| {
                            confirm_delete.set(true);
                            renaming.set(false);
                        },
                        "Delete"
                    }
                }
            }
            if *renaming.read() {
                div { class: "rename-row",
                    input {
                        value: "{title}",
                        oninput: move |event| title.set(event.value()),
                        onkeydown: {
                            let session_id = session_id.clone();
                            move |event| {
                                if event.key() == Key::Enter {
                                    rename(context, session_id.clone(), title.read().trim().to_owned());
                                    renaming.set(false);
                                }
                            }
                        }
                    }
                    button {
                        class: "button",
                        disabled: title.read().trim().is_empty(),
                        onclick: move |_| {
                            rename(context, session_id.clone(), title.read().trim().to_owned());
                            renaming.set(false);
                        },
                        "Save"
                    }
                }
            }
        }
    }
}

#[allow(clippy::large_types_passed_by_value)]
fn rename(context: AppContext, session_id: String, title: String) {
    let Some(client) = client(&context) else {
        return;
    };
    spawn(async move {
        match client.rename_session(&session_id, &title).await {
            Ok(()) => {
                context.update_sessions(|sessions| {
                    if let Some(session) = sessions
                        .iter_mut()
                        .find(|session| session.session_id == session_id)
                    {
                        session.title = Some(title.clone());
                    }
                });
                if let Some(mut selected) = context.selected_session.read().clone()
                    && selected.session_id == session_id
                {
                    selected.title = Some(title);
                    context.set_selected_session(Some(selected));
                }
            }
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
    });
}

#[allow(clippy::large_types_passed_by_value)]
fn delete(context: AppContext, session_id: String) {
    let Some(client) = client(&context) else {
        return;
    };
    spawn(async move {
        match client.delete_session(&session_id).await {
            Ok(()) => {
                context.mark_session_deleted(&session_id);
                context.update_sessions(|sessions| {
                    sessions.retain(|session| session.session_id != session_id);
                });
                if context
                    .selected_session
                    .read()
                    .as_ref()
                    .is_some_and(|session| session.session_id == session_id)
                {
                    context.set_selected_session(None);
                    context.reset_conversation();
                }
            }
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
    });
}

fn agent_label(agent: &norte_code_wire::AgentSummary) -> String {
    let provider_model = match (&agent.provider, &agent.model) {
        (Some(provider), Some(model)) => format!(", {} / {}", provider.id, model.id),
        (Some(provider), None) => format!(", {}", provider.id),
        _ => String::new(),
    };
    format!("{}{provider_model}", agent.id)
}
