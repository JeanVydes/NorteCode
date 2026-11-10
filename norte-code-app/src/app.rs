use crate::api::{ApiClient, ApiError};
use crate::components::{ConnectionBadge, PageError, Sidebar};
use crate::domain::{ChatState, ConnectionState, Page, QueueMode, ServerProfile, ThemeMode};
use crate::pages::{
    ChatPage, CostsPage, DeveloperPage, HostCapabilitiesPage, OverviewPage, PluginsPage,
    ProvidersPage, SessionsPage, SettingsPage, TelemetryPage,
};
use crate::state::AppContext;
use dioxus::prelude::*;
use std::time::Duration;

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
        session_todos: use_signal(Vec::new),
        pinned_turns: use_signal(Vec::new),
        pending_questions: use_signal(Vec::new),
        chat_state: use_signal(|| ChatState::Idle),
        active_turn_id: use_signal(|| None),
        streaming_turn_id: use_signal(|| None),
        stream_segment: use_signal(|| 0),
        conversation_epoch: use_signal(|| 0u64),
        queue_mode: use_signal(|| QueueMode::Queue),
        children: use_signal(Vec::new),
        compacting: use_signal(|| false),
        pending_turns: use_signal(std::collections::VecDeque::new),
        deleted_sessions: use_signal(std::collections::HashSet::new),
        history_request: use_signal(|| 0u64),
        live_session_list: use_signal(|| true),
        jobs: use_signal(Vec::new),
        telemetry: use_signal(|| None),
        page_error: use_signal(|| None),
        theme: use_signal(|| ThemeMode::System),
        show_thinking: use_signal(|| true),
        show_timestamps: use_signal(|| true),
        show_tool_output: use_signal(|| true),
        developer_mode: use_signal(|| false),
    };
    use_context_provider(|| context);
    use_effect(move || {
        spawn(async move {
            refresh_server(&context).await;
            // A session survives a client restart; reopening the last one
            // puts the conversation back where the person left it instead of
            // asking them to pick again.
            if context.selected_session.read().is_none() {
                let last = context.sessions.read().first().cloned();
                if let Some(session) = last {
                    context.select_session(session, Page::Chat);
                }
            }
        });
    });
    // The sidebar is a recency list and says which sessions are working, and
    // both change without this page doing anything: another client, a child
    // session, a background job. There is no push channel for the list, so it
    // is polled, but only while the tab is visible and the setting is on, and
    // on a slow cadence: the stream already tells this page everything about
    // the session it has open.
    use_effect(move || {
        spawn(async move {
            loop {
                if *context.live_session_list.read() && !window_hidden() {
                    poll_sessions(&context).await;
                }
                sleep(Duration::from_secs(15)).await;
            }
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
                            p { class: "topbar-label", "{profile_name}" }
                            ConnectionBadge { state: connection }
                        }
                        PageError {}
                        // The conversation stays mounted while another page is
                        // shown. Unmounting it closes the session's event
                        // stream, and coming back reopened it and reloaded the
                        // history, which threw away whatever was streaming
                        // and made the page "start loading the conversation
                        // again" on every tab switch.
                        div { class: if page == Page::Chat { "page-body" } else { "page-body page-hidden" },
                            ChatPage {}
                        }
                        if page != Page::Chat {
                            match page {
                                Page::Overview => rsx! { OverviewPage {} },
                                Page::Sessions => rsx! { SessionsPage {} },
                                Page::Providers => rsx! { ProvidersPage {} },
                                Page::Plugins => rsx! { PluginsPage {} },
                                Page::HostCapabilities => rsx! { HostCapabilitiesPage {} },
                                Page::Costs => rsx! { CostsPage {} },
                                Page::Telemetry => rsx! { TelemetryPage {} },
                                Page::Settings => rsx! { SettingsPage {} },
                                Page::Developer => rsx! { DeveloperPage {} },
                                Page::Chat => rsx! {},
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Re-reads the session list in place, leaving the open conversation alone.
async fn poll_sessions(context: &AppContext) {
    if *context.connection.read() != ConnectionState::Connected {
        return;
    }
    let Some(profile) = context.active_profile() else {
        return;
    };
    let Ok(client) = ApiClient::new(&profile.base_url, &profile.bearer_token) else {
        return;
    };
    if let Ok(sessions) = client.sessions().await {
        // The list is the host's answer, and the open session is refreshed
        // from it: a session deleted elsewhere stops being shown, and one
        // whose title or model changed elsewhere stops being a stale copy.
        let selected = context.selected_session.read().clone();
        if let Some(selected) = selected {
            if let Some(fresh) = sessions
                .iter()
                .find(|session| session.session_id == selected.session_id)
            {
                context.set_selected_session(Some(fresh.clone()));
            } else {
                context.set_selected_session(None);
                context.reset_conversation();
            }
        }
        context.set_sessions(sessions);
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

/// Whether the browser tab is in the background.
fn window_hidden() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        return web_sys::window()
            .and_then(|window| window.document())
            .is_some_and(|document| document.hidden());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

/// Waits without pulling a runtime in: Dioxus's own timer on wasm, a blocking
/// thread elsewhere. One poll interval, not a hot loop.
#[cfg_attr(not(target_arch = "wasm32"), allow(clippy::unused_async))]
async fn sleep(duration: std::time::Duration) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast as _;
        if let Some(window) = web_sys::window() {
            let promise = js_sys::Promise::new(&mut |resolve, _reject| {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    &resolve,
                    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX),
                );
            });
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Native polls run off the UI thread already; a blocking sleep here
        // only delays this task, not the renderer.
        std::thread::sleep(duration);
    }
}
