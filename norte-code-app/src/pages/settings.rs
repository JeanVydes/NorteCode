use crate::components::PageHeader;
use crate::domain::{ConnectionState, ServerProfile, ThemeMode};
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn SettingsPage() -> Element {
    let context = use_context::<AppContext>();
    let profile = context.active_profile();
    let theme = *context.theme.read();
    let selected_id = context.active_profile_id.read().clone();
    let profile_count = context.profiles.read().len();
    rsx! {
        PageHeader {
            eyebrow: "Preferences",
            title: "Settings",
            description: "Manage local and remote RAEP profiles, connection credentials, theme, and conversation display preferences."
        }
        section { class: "panel",
            div { class: "section-heading",
                h2 { "Server profiles" }
                button {
                    class: "button-secondary",
                    onclick: move |_| add_profile(&context),
                    "Add server"
                }
            }
            div { class: "form-grid",
                label { class: "field",
                    span { "Active server" }
                    select {
                        value: "{selected_id}",
                        onchange: move |event| {
                            context.set_active_profile_id(event.value());
                            context.set_connection(ConnectionState::Disconnected);
                            context.set_server_info(None);
                            context.set_page_error(None);
                        },
                        for item in context.profiles.read().iter() {
                            option { value: "{item.id}", "{item.name} · {item.base_url}" }
                        }
                    }
                }
                if let Some(profile) = profile {
                    label { class: "field",
                        span { "Profile name" }
                        input {
                            value: "{profile.name}",
                            oninput: move |event| update_profile(&context, |current| current.name = event.value())
                        }
                    }
                    label { class: "field",
                        span { "Server URL" }
                        input {
                            value: "{profile.base_url}",
                            placeholder: "https://server.example",
                            oninput: move |event| update_profile(&context, |current| current.base_url = event.value())
                        }
                    }
                    label { class: "field",
                        span { "Bearer token" }
                        input {
                            r#type: "password",
                            value: "{profile.bearer_token}",
                            autocomplete: "off",
                            oninput: move |event| update_profile(&context, |current| current.bearer_token = event.value())
                        }
                    }
                    div { class: "button-row",
                        button {
                            class: "button",
                            onclick: move |_| {
                                spawn(async move { crate::app::refresh_server(&context).await; });
                            },
                            "Test and connect"
                        }
                        if profile_count > 1 {
                            button {
                                class: "button-danger",
                                onclick: move |_| remove_active_profile(&context),
                                "Remove profile"
                            }
                        }
                    }
                }
            }
            p { class: "footer-note", "Credentials are kept in application memory only in this build. Web deployment also requires the RAEP server to allow this app's Origin through its CORS configuration." }
        }
        section { class: "panel", style: "margin-top: 1rem",
            div { class: "section-heading", h2 { "Appearance" } }
            label { class: "field",
                span { "Theme" }
                select {
                    value: theme_value(theme),
                    onchange: move |event| context.set_theme(parse_theme(&event.value())),
                    option { value: "system", "{ThemeMode::System.label()}" }
                    option { value: "light", "{ThemeMode::Light.label()}" }
                    option { value: "dark", "{ThemeMode::Dark.label()}" }
                }
            }
            ToggleRow {
                label: "Show thinking",
                description: "Render reasoning history items when the server includes them.",
                checked: *context.show_thinking.read(),
                onchange: move |value| context.set_show_thinking(value),
            }
            ToggleRow {
                label: "Show timestamps",
                description: "Show the timestamps carried by history items.",
                checked: *context.show_timestamps.read(),
                onchange: move |value| context.set_show_timestamps(value),
            }
            ToggleRow {
                label: "Show tool output",
                description: "Include tool calls and results in the conversation timeline.",
                checked: *context.show_tool_output.read(),
                onchange: move |value| context.set_show_tool_output(value),
            }
            ToggleRow {
                label: "Live session list",
                description: "Refresh the sidebar from the host every 15 seconds while this tab is visible, so another client's turns and new sessions show up.",
                checked: *context.live_session_list.read(),
                onchange: move |value| context.set_live_session_list(value),
            }
            ToggleRow {
                label: "Developer mode",
                description: "Expose negotiated capabilities and transport diagnostics on a separate page.",
                checked: *context.developer_mode.read(),
                onchange: move |value| context.set_developer_mode(value),
            }
        }
    }
}

#[component]
fn ToggleRow(
    label: &'static str,
    description: &'static str,
    checked: bool,
    onchange: EventHandler<bool>,
) -> Element {
    rsx! {
        div { class: "toggle-row",
            div {
                strong { "{label}" }
                p { class: "helper-text", "{description}" }
            }
            input {
                r#type: "checkbox",
                checked,
                onchange: move |event| onchange.call(event.checked())
            }
        }
    }
}

fn add_profile(context: &AppContext) {
    let profile = ServerProfile {
        id: uuid::Uuid::new_v4().to_string(),
        name: "New server".to_owned(),
        base_url: "https://".to_owned(),
        bearer_token: String::new(),
    };
    context.set_active_profile_id(profile.id.clone());
    context.update_profiles(|profiles| profiles.push(profile));
    context.set_connection(ConnectionState::Disconnected);
    context.set_server_info(None);
}

fn update_profile(context: &AppContext, update: impl FnOnce(&mut ServerProfile)) {
    let id = context.active_profile_id.read().clone();
    context.update_profiles(|profiles| {
        if let Some(profile) = profiles.iter_mut().find(|profile| profile.id == id) {
            update(profile);
        }
    });
    context.set_connection(ConnectionState::Disconnected);
    context.set_server_info(None);
}

fn remove_active_profile(context: &AppContext) {
    let id = context.active_profile_id.read().clone();
    let next_id = context
        .profiles
        .read()
        .iter()
        .find(|profile| profile.id != id)
        .map(|profile| profile.id.clone());
    if let Some(next_id) = next_id {
        context.update_profiles(|profiles| profiles.retain(|profile| profile.id != id));
        context.set_active_profile_id(next_id);
        context.set_connection(ConnectionState::Disconnected);
        context.set_server_info(None);
    }
}

fn theme_value(theme: ThemeMode) -> &'static str {
    match theme {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

fn parse_theme(value: &str) -> ThemeMode {
    match value {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        _ => ThemeMode::System,
    }
}
