use crate::api::{ApiClient, ApiError};
use crate::components::{ConnectionBadge, PageError, Sidebar};
use crate::domain::{ChatState, ConnectionState, Page, ServerProfile, ThemeMode};
use crate::pages::{
    ChatPage, DeveloperPage, JobsPage, OverviewPage, SessionsPage, SettingsPage, SubagentsPage,
    TelemetryPage,
};
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn App() -> Element {
    let profile = ServerProfile {
        id: "local".to_owned(),
        name: "Local RAEP".to_owned(),
        base_url: "http://127.0.0.1:8787".to_owned(),
        bearer_token: String::new(),
    };
    let context = AppContext {
        page: use_signal(|| Page::Overview),
        profiles: use_signal(|| vec![profile]),
        active_profile_id: use_signal(|| "local".to_owned()),
        connection: use_signal(|| ConnectionState::Disconnected),
        connection_error: use_signal(|| None),
        server_info: use_signal(|| None),
        agents: use_signal(Vec::new),
        sessions: use_signal(Vec::new),
        selected_session: use_signal(|| None),
        chat_items: use_signal(Vec::new),
        chat_state: use_signal(|| ChatState::Idle),
        active_turn_id: use_signal(|| None),
        jobs: use_signal(Vec::new),
        telemetry: use_signal(|| None),
        page_error: use_signal(|| None),
        theme: use_signal(|| ThemeMode::System),
        show_thinking: use_signal(|| false),
        show_timestamps: use_signal(|| true),
        show_tool_output: use_signal(|| true),
        developer_mode: use_signal(|| false),
    };
    use_context_provider(|| context);
    use_effect(move || {
        spawn(async move {
            refresh_server(&context).await;
        });
    });

    let page = *context.page.read();
    let theme_class = context.theme.read().css_class();
    let connection = *context.connection.read();
    let profile_name = context
        .active_profile()
        .map_or_else(|| "No server selected".to_owned(), |value| value.name);
    rsx! {
        {crate::theme::stylesheet()}
        div { class: "app-root {theme_class}",
            div { class: "app-layout",
                Sidebar {}
                main { class: "main-area",
                    div { class: "content-column",
                        div { class: "topbar",
                            p { class: "topbar-label", "Connected target · {profile_name}" }
                            div { class: "button-row",
                                ConnectionBadge { state: connection }
                                button {
                                    class: "button-secondary",
                                    onclick: move |_| {
                                        spawn(async move { refresh_server(&context).await; });
                                    },
                                    "Connect / refresh"
                                }
                            }
                        }
                        PageError {}
                        match page {
                            Page::Overview => rsx! { OverviewPage {} },
                            Page::Sessions => rsx! { SessionsPage {} },
                            Page::Chat => rsx! { ChatPage {} },
                            Page::Jobs => rsx! { JobsPage {} },
                            Page::Subagents => rsx! { SubagentsPage {} },
                            Page::Telemetry => rsx! { TelemetryPage {} },
                            Page::Settings => rsx! { SettingsPage {} },
                            Page::Developer => rsx! { DeveloperPage {} },
                        }
                    }
                }
            }
        }
    }
}

pub(crate) async fn refresh_server(context: &AppContext) {
    context.set_connection(ConnectionState::Connecting);
    context.set_page_error(None);
    let Some(profile) = context.active_profile() else {
        context.set_connection(ConnectionState::Disconnected);
        context.set_page_error(Some("Select a server profile first.".to_owned()));
        return;
    };
    let client = match ApiClient::new(&profile.base_url, &profile.bearer_token) {
        Ok(client) => client,
        Err(error) => {
            set_connection_error(context, &error);
            return;
        }
    };
    let result = async {
        let info = client.info().await?;
        let agents = client.agents().await?;
        let sessions = client.sessions().await?;
        Ok::<_, ApiError>((info, agents, sessions))
    }
    .await;
    match result {
        Ok((info, agents, sessions)) => {
            context.set_server_info(Some(info));
            context.set_agents(agents);
            context.set_sessions(sessions);
            context.set_connection_error(None);
            context.set_connection(ConnectionState::Connected);
        }
        Err(error) => set_connection_error(context, &error),
    }
}

fn set_connection_error(context: &AppContext, error: &ApiError) {
    let message = error.to_string();
    context.set_connection_error(Some(message.clone()));
    context.set_page_error(Some(message));
    context.set_connection(ConnectionState::Failed);
}
