use crate::api::{ApiClient, McpCatalog, SkillCatalog, ToolRule};
use crate::components::{EmptyState, PageHeader};
use crate::state::AppContext;
use dioxus::prelude::*;
use serde_json::Value;

#[component]
pub fn HostCapabilitiesPage() -> Element {
    let context = use_context::<AppContext>();
    let skills = use_signal(|| None::<SkillCatalog>);
    let mcp = use_signal(|| None::<McpCatalog>);
    let tools = use_signal(|| None::<Vec<ToolRule>>);
    let loading_skills = use_signal(|| false);
    let loading_mcp = use_signal(|| false);
    let loading_tools = use_signal(|| false);
    let busy = use_signal(|| false);
    let mut errors = use_signal(Vec::<String>::new);
    use_effect(move || {
        errors.set(Vec::new());
        refresh_skills(&context, skills, loading_skills, errors, false);
        refresh_mcp(&context, mcp, loading_mcp, errors, false);
        refresh_tools(&context, tools, loading_tools, errors);
    });

    let skill_catalog = skills.read().clone();
    let mcp_catalog = mcp.read().clone();
    let tool_rules = tools.read().clone();
    let capability_errors = errors.read().clone();

    let profile_name = context
        .active_profile()
        .map_or_else(|| "No server".to_owned(), |profile| profile.name);
    let connection = *context.connection.read();
    let server_version = context
        .server_info
        .read()
        .as_ref()
        .map(|info| info.server_version.clone());
    rsx! {
        PageHeader { eyebrow: "", title: "Host", description: "" }
        section { class: "panel host-status",
            div { class: "host-status-line",
                span { class: "host-name", "{profile_name}" }
                span { class: connection.css_class(), "{connection.label()}" }
                if let Some(version) = server_version {
                    span { class: "helper-text", "server {version}" }
                }
            }
            div { class: "button-row",
                button {
                    class: "button",
                    onclick: move |_| {
                        spawn(async move { crate::app::refresh_server(&context).await; });
                    },
                    "Connect / refresh"
                }
            }
        }
        for error in capability_errors {
            div { class: "notice notice-error", role: "alert", "{error}" }
        }
        section { class: "panel",
            div { class: "section-heading",
                h2 { "Agent Skills" }
                button {
                    class: "button-secondary",
                    disabled: *loading_skills.read() || *busy.read(),
                    onclick: move |_| refresh_skills(&context, skills, loading_skills, errors, true),
                    if *loading_skills.read() { "Refreshing…" } else { "Rescan skills" }
                }
            }
            if let Some(catalog) = skill_catalog {
                if catalog.warnings.is_empty() {
                    p { class: "helper-text", "The host reported no skill scan errors." }
                } else {
                    for warning in catalog.warnings {
                        div { class: "notice notice-error", role: "alert",
                            strong { "Skill scan warning" }
                            p { "{warning}" }
                        }
                    }
                }
                if catalog.skills.is_empty() {
                    EmptyState {
                        title: "No skills found",
                        message: "The host found no installed skills. Rescan to read the configured skill roots again."
                    }
                } else {
                    div { class: "catalog-provider-grid",
                        for skill in catalog.skills {
                            SkillCard { skill }
                        }
                    }
                }
            } else {
                p { class: "helper-text", "Connect to load installed skills." }
            }
        }
        section { class: "panel", style: "margin-top: 1rem",
            div { class: "section-heading",
                h2 { "MCP servers" }
                button {
                    class: "button-secondary",
                    disabled: *loading_mcp.read() || *busy.read(),
                    onclick: move |_| refresh_mcp(&context, mcp, loading_mcp, errors, true),
                    if *loading_mcp.read() { "Refreshing…" } else { "Reconnect and refresh" }
                }
            }
            if let Some(catalog) = mcp_catalog {
                if !catalog.failures.is_empty() {
                    for failure in catalog.failures {
                        div { class: "notice notice-error", role: "alert",
                            strong { "MCP server failed" }
                            pre { class: "output-box", "{pretty_json(&failure)}" }
                        }
                    }
                }
                if catalog.servers.is_empty() {
                    EmptyState {
                        title: "No MCP servers connected",
                        message: "The host returned no active MCP servers."
                    }
                } else {
                    div { class: "session-grid",
                        for server in catalog.servers {
                            McpCard {
                                server,
                                busy: *busy.read(),
                                on_toggle: move |(name, active)| {
                                    set_mcp_active(&context, mcp, loading_mcp, busy, errors, name, active);
                                }
                            }
                        }
                    }
                }
            } else {
                p { class: "helper-text", "Connect to load MCP server state." }
            }
        }
        section { class: "panel", style: "margin-top: 1rem",
            div { class: "section-heading",
                h2 { "Tool approvals" }
                button {
                    class: "button-secondary",
                    disabled: *loading_tools.read() || *busy.read(),
                    onclick: move |_| refresh_tools(&context, tools, loading_tools, errors),
                    if *loading_tools.read() { "Refreshing…" } else { "Refresh rules" }
                }
            }
            p { class: "helper-text",
                "Allow and deny actions below create global rules that affect every session on this host. Forget removes the explicit rule and restores the host's default."
            }
            if let Some(tool_rules) = tool_rules {
                if tool_rules.is_empty() {
                    EmptyState {
                        title: "No tools reported",
                        message: "The host did not report any tool approval state."
                    }
                } else {
                    div { class: "job-grid",
                        for tool in tool_rules {
                            ToolRuleCard {
                                tool,
                                busy: *busy.read(),
                                on_change: move |(name, action)| {
                                    set_tool_rule(&context, tools, loading_tools, busy, errors, name, action);
                                }
                            }
                        }
                    }
                }
            } else {
                p { class: "helper-text", "Connect to load tool approval rules." }
            }
        }
    }
}

#[component]
fn SkillCard(skill: Value) -> Element {
    let name = string_field(&skill, "name")
        .or_else(|| string_field(&skill, "id"))
        .unwrap_or_else(|| "Unnamed skill".to_owned());
    let description = string_field(&skill, "description");
    rsx! {
        article { class: "panel catalog-provider",
            h3 { "{name}" }
            if let Some(description) = description {
                p { class: "helper-text", "{description}" }
            }
        }
    }
}

#[component]
fn McpCard(server: Value, busy: bool, on_toggle: EventHandler<(String, bool)>) -> Element {
    let name = string_field(&server, "name");
    let active = server
        .get("active")
        .and_then(Value::as_bool)
        .or_else(|| server.get("enabled").and_then(Value::as_bool));
    let tool_count = server
        .get("tools")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    rsx! {
        article { class: "session-card",
            div { class: "session-card-head",
                div {
                    h3 { "{name.as_deref().unwrap_or(\"Unknown MCP server\")}" }
                    p { class: "helper-text", "{tool_count} tools reported" }
                }
                if let Some(active) = active {
                    span {
                        class: if active { "status status-active" } else { "status status-muted" },
                        if active { "Active" } else { "Inactive" }
                    }
                } else {
                    span { class: "status status-unknown", "State unavailable" }
                }
            }
            if let Some(name) = name {
                if let Some(active) = active {
                    button {
                        class: "button-secondary",
                        disabled: busy,
                        style: "margin-top: 1rem",
                        onclick: move |_| on_toggle.call((name.clone(), !active)),
                        if active { "Disable" } else { "Enable" }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ToolRuleAction {
    Allow,
    Deny,
    Forget,
}

#[component]
fn ToolRuleCard(
    tool: ToolRule,
    busy: bool,
    on_change: EventHandler<(String, ToolRuleAction)>,
) -> Element {
    let name = tool.name.clone();
    rsx! {
        article { class: "job-card",
            div { class: "job-card-head",
                div {
                    h3 { "{tool.name}" }
                    p { class: "helper-text", "Rule scope: {tool.scope}" }
                }
                span {
                    class: if tool.allowed { "status status-active" } else { "status status-muted" },
                    if tool.allowed { "Allowed" } else { "Denied" }
                }
            }
            div { class: "button-row", style: "margin-top: 1rem",
                button {
                    class: "button-secondary",
                    disabled: busy || tool.allowed,
                    onclick: {
                        let name = name.clone();
                        move |_| on_change.call((name.clone(), ToolRuleAction::Allow))
                    },
                    "Allow globally"
                }
                button {
                    class: "button-secondary",
                    disabled: busy || !tool.allowed,
                    onclick: {
                        let name = name.clone();
                        move |_| on_change.call((name.clone(), ToolRuleAction::Deny))
                    },
                    "Deny globally"
                }
                button {
                    class: "button-secondary",
                    disabled: busy || tool.scope == "default",
                    onclick: move |_| on_change.call((name.clone(), ToolRuleAction::Forget)),
                    "Forget rule"
                }
            }
        }
    }
}

fn refresh_skills(
    context: &AppContext,
    mut skills: Signal<Option<SkillCatalog>>,
    mut loading: Signal<bool>,
    mut errors: Signal<Vec<String>>,
    rescan: bool,
) {
    loading.set(true);
    errors.set(Vec::new());
    let context = *context;
    spawn(async move {
        match active_api(&context) {
            Ok(api) => match if rescan {
                api.refresh_skills().await
            } else {
                api.skills().await
            } {
                Ok(catalog) => skills.set(Some(catalog)),
                Err(error) => push_error(errors, error.to_string()),
            },
            Err(error) => push_error(errors, error),
        }
        loading.set(false);
    });
}

fn refresh_mcp(
    context: &AppContext,
    mut mcp: Signal<Option<McpCatalog>>,
    mut loading: Signal<bool>,
    mut errors: Signal<Vec<String>>,
    reconnect: bool,
) {
    loading.set(true);
    errors.set(Vec::new());
    let context = *context;
    spawn(async move {
        match active_api(&context) {
            Ok(api) => match if reconnect {
                api.refresh_mcp().await
            } else {
                api.mcp_catalog().await
            } {
                Ok(catalog) => mcp.set(Some(catalog)),
                Err(error) => push_error(errors, error.to_string()),
            },
            Err(error) => push_error(errors, error),
        }
        loading.set(false);
    });
}

fn refresh_tools(
    context: &AppContext,
    mut tools: Signal<Option<Vec<ToolRule>>>,
    mut loading: Signal<bool>,
    mut errors: Signal<Vec<String>>,
) {
    loading.set(true);
    errors.set(Vec::new());
    let context = *context;
    spawn(async move {
        match active_api(&context) {
            Ok(api) => match api.tool_rules().await {
                Ok(rules) => tools.set(Some(rules)),
                Err(error) => push_error(errors, error.to_string()),
            },
            Err(error) => push_error(errors, error),
        }
        loading.set(false);
    });
}

fn set_mcp_active(
    context: &AppContext,
    mcp: Signal<Option<McpCatalog>>,
    loading: Signal<bool>,
    mut busy: Signal<bool>,
    errors: Signal<Vec<String>>,
    name: String,
    active: bool,
) {
    let api = match active_api(context) {
        Ok(api) => api,
        Err(error) => {
            push_error(errors, error);
            return;
        }
    };
    busy.set(true);
    let context = *context;
    spawn(async move {
        match api.set_mcp_active(&name, active).await {
            Ok(()) => refresh_mcp(&context, mcp, loading, errors, false),
            Err(error) => push_error(errors, error.to_string()),
        }
        busy.set(false);
    });
}

fn set_tool_rule(
    context: &AppContext,
    tools: Signal<Option<Vec<ToolRule>>>,
    loading: Signal<bool>,
    mut busy: Signal<bool>,
    errors: Signal<Vec<String>>,
    name: String,
    action: ToolRuleAction,
) {
    let api = match active_api(context) {
        Ok(api) => api,
        Err(error) => {
            push_error(errors, error);
            return;
        }
    };
    busy.set(true);
    let context = *context;
    spawn(async move {
        let result = match action {
            ToolRuleAction::Allow => api.set_tool_allowed(&name, true).await,
            ToolRuleAction::Deny => api.set_tool_allowed(&name, false).await,
            ToolRuleAction::Forget => api.forget_tool_rule(&name).await,
        };
        match result {
            Ok(()) => refresh_tools(&context, tools, loading, errors),
            Err(error) => push_error(errors, error.to_string()),
        }
        busy.set(false);
    });
}

fn active_api(context: &AppContext) -> Result<ApiClient, String> {
    let profile = context
        .active_profile()
        .ok_or_else(|| "Select a server profile first.".to_owned())?;
    ApiClient::new(&profile.base_url, &profile.bearer_token).map_err(|error| error.to_string())
}

fn push_error(mut errors: Signal<Vec<String>>, error: String) {
    errors.with_mut(|current| {
        if !current.contains(&error) {
            current.push(error);
        }
    });
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}
