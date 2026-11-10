use crate::api::{ApiClient, PluginChange, PluginSummary};
use crate::components::{EmptyState, PageHeader};
use crate::state::AppContext;
use dioxus::prelude::*;

#[derive(Clone)]
enum PluginAction {
    Install {
        url: String,
        package: Option<String>,
    },
    Update(String),
    Remove(String),
    SetEnabled {
        name: String,
        enabled: bool,
    },
}

#[component]
pub fn PluginsPage() -> Element {
    let context = use_context::<AppContext>();
    let plugins = use_signal(Vec::<PluginSummary>::new);
    let loading = use_signal(|| false);
    let busy = use_signal(|| false);
    let mut source_url = use_signal(String::new);
    let mut package = use_signal(String::new);
    let mut pending_remove = use_signal(|| None::<String>);
    let notice = use_signal(|| None::<String>);
    use_effect(move || refresh_plugins(&context, plugins, loading));

    let current_plugins = plugins.read().clone();
    rsx! {
        PageHeader {
            eyebrow: "Host extensions",
            title: "Plugins",
            description: "Inspect and manage plugins installed on the selected RAEP host."
        }
        section { class: "notice notice-info",
            strong { "Install only trusted repositories" }
            p {
                "Plugin installation clones and builds remote code on the host. Enabling or disabling a plugin changes its state for the next host start; there is no live reload operation in the current RAEP API."
            }
        }
        section { class: "panel", style: "margin-top: 1rem",
            div { class: "section-heading",
                h2 { "Install plugin" }
            }
            div { class: "form-row",
                label { class: "field",
                    span { "Git repository URL" }
                    input {
                        value: "{source_url}",
                        placeholder: "https://github.com/owner/plugin",
                        oninput: move |event| source_url.set(event.value())
                    }
                }
                label { class: "field",
                    span { "Workspace package (optional)" }
                    input {
                        value: "{package}",
                        placeholder: "plugin package name",
                        oninput: move |event| package.set(event.value())
                    }
                }
            }
            div { class: "button-row", style: "margin-top: 1rem",
                button {
                    class: "button",
                    disabled: *busy.read() || source_url.read().trim().is_empty(),
                    onclick: move |_| {
                        let url = source_url.read().trim().to_owned();
                        let package_name = package.read().trim().to_owned();
                        run_plugin_action(
                            &context,
                            plugins,
                            busy,
                            pending_remove,
                            notice,
                            PluginAction::Install {
                                url,
                                package: (!package_name.is_empty()).then_some(package_name),
                            },
                        );
                    },
                    if *busy.read() { "Working…" } else { "Install" }
                }
            }
        }
        if let Some(message) = notice.read().as_ref() {
            div { class: "notice notice-info", "{message}" }
        }
        div { class: "section-heading",
            h2 { "Installed plugins" }
            button {
                class: "button-secondary",
                disabled: *loading.read() || *busy.read(),
                onclick: move |_| refresh_plugins(&context, plugins, loading),
                if *loading.read() { "Refreshing…" } else { "Refresh" }
            }
        }
        if current_plugins.is_empty() && !*loading.read() {
            EmptyState {
                title: "No plugins reported",
                message: "The host returned no installed plugins, or has not loaded its plugin catalogue yet."
            }
        } else {
            div { class: "plugin-grid",
                for plugin in current_plugins {
                    PluginCard {
                        plugin: plugin.clone(),
                        busy: *busy.read(),
                        removal_pending: pending_remove.read().as_deref() == Some(plugin.name.as_str()),
                        on_action: move |action| run_plugin_action(
                            &context, plugins, busy, pending_remove, notice, action,
                        ),
                        on_remove_requested: move |name| pending_remove.set(Some(name)),
                        on_keep: move |()| pending_remove.set(None),
                    }
                }
            }
        }
    }
}

#[component]
fn PluginCard(
    plugin: PluginSummary,
    busy: bool,
    removal_pending: bool,
    on_action: EventHandler<PluginAction>,
    on_remove_requested: EventHandler<String>,
    on_keep: EventHandler<()>,
) -> Element {
    let name = plugin.name.clone();
    let enabled = plugin.enabled;
    rsx! {
        article { class: "job-card plugin-card",
            div { class: "job-card-head",
                div {
                    h3 { "{plugin.name}" }
                    p { class: "helper-text",
                        if plugin.loaded {
                            "Loaded by this host"
                        } else if plugin.enabled {
                            "Enabled; takes effect after host restart"
                        } else {
                            "Disabled"
                        }
                    }
                }
                span {
                    class: if plugin.enabled { "status status-active" } else { "status status-muted" },
                    if plugin.enabled { "Enabled" } else { "Disabled" }
                }
            }
            div { class: "button-row", style: "margin-top: 1rem",
                button {
                    class: "button-secondary",
                    disabled: busy,
                    onclick: move |_| on_action.call(PluginAction::SetEnabled {
                        name: name.clone(),
                        enabled: !enabled,
                    }),
                    if plugin.enabled { "Disable" } else { "Enable" }
                }
                button {
                    class: "button-secondary",
                    disabled: busy,
                    onclick: {
                        let name = plugin.name.clone();
                        move |_| on_action.call(PluginAction::Update(name.clone()))
                    },
                    "Update"
                }
                if removal_pending {
                    button {
                        class: "button-danger",
                        disabled: busy,
                        onclick: {
                            let name = plugin.name.clone();
                            move |_| on_action.call(PluginAction::Remove(name.clone()))
                        },
                        "Confirm remove"
                    }
                    button {
                        class: "button-secondary",
                        onclick: move |_| on_keep.call(()),
                        "Keep"
                    }
                } else {
                    button {
                        class: "button-danger",
                        disabled: busy,
                        onclick: {
                            let name = plugin.name.clone();
                            move |_| on_remove_requested.call(name.clone())
                        },
                        "Remove"
                    }
                }
            }
        }
    }
}

fn refresh_plugins(
    context: &AppContext,
    mut plugins: Signal<Vec<PluginSummary>>,
    mut loading: Signal<bool>,
) {
    loading.set(true);
    let context = *context;
    spawn(async move {
        context.set_page_error(None);
        let Some(profile) = context.active_profile() else {
            context.set_page_error(Some("Select a server profile first.".to_owned()));
            loading.set(false);
            return;
        };
        let api = match ApiClient::new(&profile.base_url, &profile.bearer_token) {
            Ok(api) => api,
            Err(error) => {
                context.set_page_error(Some(error.to_string()));
                loading.set(false);
                return;
            }
        };
        match api.plugins().await {
            Ok(result) => plugins.set(result),
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
        loading.set(false);
    });
}

fn run_plugin_action(
    context: &AppContext,
    mut plugins: Signal<Vec<PluginSummary>>,
    mut busy: Signal<bool>,
    mut pending_remove: Signal<Option<String>>,
    mut notice: Signal<Option<String>>,
    action: PluginAction,
) {
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
    busy.set(true);
    notice.set(None);
    pending_remove.set(None);
    let context = *context;
    spawn(async move {
        context.set_page_error(None);
        let result = match action {
            PluginAction::Install { url, package } => {
                api.install_plugin(&url, package.as_deref(), false).await
            }
            PluginAction::Update(name) => api.update_plugin(Some(&name)).await,
            PluginAction::Remove(name) => api.remove_plugin(&name).await,
            PluginAction::SetEnabled { name, enabled } => {
                api.set_plugin_enabled(&name, enabled).await
            }
        };
        match result {
            Ok(change) => {
                notice.set(Some(format_change(&change)));
                match api.plugins().await {
                    Ok(result) => plugins.set(result),
                    Err(error) => context.set_page_error(Some(error.to_string())),
                }
            }
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
        busy.set(false);
    });
}

fn format_change(change: &PluginChange) -> String {
    if change.name.is_empty() {
        format!("Plugin operation {} completed.", change.operation)
    } else {
        format!("Plugin {}: {}.", change.name, change.operation)
    }
}
