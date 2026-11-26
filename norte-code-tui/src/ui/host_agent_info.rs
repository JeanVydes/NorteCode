//! The safe, detailed agent profile shown in the host overlay.

use super::detail_panel::Field;
use super::host_agent_format::{
    budget_fields, enabled, list_or_none, modality_label, modality_list, reasoning_label, ref_id,
};
use super::host_info::{human_bytes, millis};
use crate::ui_model::UiModel;

pub(super) fn append(fields: &mut Vec<Field>, model: &UiModel) {
    fields.push(Field::separator());
    fields.push(Field::new(
        "configured agents",
        model.agents.len().to_string(),
    ));
    if model.agents.is_empty() {
        fields.push(Field::new("agents", "not reported by host"));
        return;
    }

    let default_agent = model
        .server_info
        .as_ref()
        .map(|info| info.default_agent.as_str());
    for agent in &model.agents {
        append_agent(fields, agent, default_agent == Some(agent.id.as_str()));
    }
}

fn append_agent(fields: &mut Vec<Field>, agent: &norte_code_wire::AgentSummary, is_default: bool) {
    fields.push(Field::separator());
    fields.push(Field::new("agent", agent_label(agent, is_default)));
    fields.push(Field::new("version", version_label(agent)));
    fields.push(Field::new("provider", ref_id(agent.provider.as_ref())));
    fields.push(Field::new("model", ref_id(agent.model.as_ref())));
    if let Some(capabilities) = &agent.capabilities {
        append_capabilities(fields, capabilities);
    }
    let Some(profile) = &agent.profile else {
        fields.push(Field::new("profile", "details unavailable"));
        return;
    };
    append_profile(fields, profile);
}

fn append_profile(fields: &mut Vec<Field>, profile: &norte_code_wire::AgentProfileSummary) {
    append_identity(fields, profile);
    append_runtime(fields, profile);
    append_tool_policy(fields, profile);
    budget_fields(fields, profile);
    append_integrations(fields, profile);
}

fn append_identity(fields: &mut Vec<Field>, profile: &norte_code_wire::AgentProfileSummary) {
    if let Some(description) = &profile.description {
        fields.push(Field::new("description", description));
    }
    if let Some(author) = &profile.author {
        fields.push(Field::new("author", author));
    }
    if let Some(origin) = &profile.origin_url {
        fields.push(Field::new("definition", origin));
    }
    fields.push(Field::new("reasoning", reasoning_label(&profile.reasoning)));
    fields.push(Field::new("delegates", list_or_none(&profile.delegates)));
    if let Some(target) = &profile.default_subagent_delegation {
        fields.push(Field::new("default delegate", target));
    }
    fields.push(Field::new(
        "child limits",
        format!(
            "depth {} · {} concurrent · {} held messages",
            profile.delegation.max_depth,
            profile.delegation.max_concurrent,
            profile.delegation.max_held_messages
        ),
    ));
}

fn append_runtime(fields: &mut Vec<Field>, profile: &norte_code_wire::AgentProfileSummary) {
    fields.push(Field::new(
        "compaction",
        format!(
            "{} · {:.0}% · {:?} · keep {}",
            enabled(profile.compaction.enabled),
            profile.compaction.threshold * 100.0,
            profile.compaction.trigger,
            profile.compaction.keep_recent
        ),
    ));
    fields.push(Field::new(
        "retries",
        format!(
            "{} attempts · {} initial / {} max",
            profile.retry.max_attempts,
            millis(profile.retry.backoff_ms),
            millis(profile.retry.max_delay_ms)
        ),
    ));
    fields.push(Field::new(
        "turn limit",
        profile.max_iterations.map_or_else(
            || "unlimited".to_owned(),
            |limit| format!("{limit} iterations"),
        ),
    ));
}

fn append_tool_policy(fields: &mut Vec<Field>, profile: &norte_code_wire::AgentProfileSummary) {
    fields.push(Field::new(
        "tool access",
        format!(
            "web {} · job auto-turn {} · workspace {}",
            enabled(profile.tools.web_fetch),
            enabled(profile.tools.auto_turn_on_job_end),
            if profile.tools.workspace_only {
                "only"
            } else {
                "unrestricted"
            }
        ),
    ));
    fields.push(Field::new(
        "shell sandbox",
        format!(
            "{} · network {} · timeout {}–{}",
            enabled(profile.tools.sandbox_enabled),
            enabled(profile.tools.sandbox_network),
            millis(profile.tools.shell_default_timeout_ms),
            millis(profile.tools.shell_max_timeout_ms)
        ),
    ));
    fields.push(Field::new(
        "shell indefinite",
        enabled(profile.tools.allow_indefinite_shell),
    ));
    fields.push(Field::new(
        "disabled tools",
        list_or_none(&profile.tools.disabled),
    ));
    fields.push(Field::new(
        "always allowed",
        list_or_none(&profile.approvals.allow),
    ));
    fields.push(Field::new(
        "always denied",
        list_or_none(&profile.approvals.deny),
    ));
}

fn append_integrations(fields: &mut Vec<Field>, profile: &norte_code_wire::AgentProfileSummary) {
    fields.push(Field::new(
        "skills",
        format!(
            "{} · interop {} · scripts {} · {} max · {} extra roots",
            enabled(profile.skills.enabled),
            enabled(profile.skills.interop),
            enabled(profile.skills.allow_scripts),
            profile.skills.max_skills,
            profile.skills.extra_path_count
        ),
    ));
    fields.push(Field::new(
        "skill approvals",
        enabled(profile.skills.honor_allowed_tools),
    ));
    fields.push(Field::new("plugins", plugin_list(profile)));
    fields.push(Field::new("MCP servers", mcp_list(profile)));
    fields.push(Field::new(
        "system prompt",
        if profile.custom_system_prompt {
            "custom prompt configured; contents hidden"
        } else {
            "built-in"
        },
    ));
}

fn agent_label(agent: &norte_code_wire::AgentSummary, is_default: bool) -> String {
    if is_default {
        format!("{} · default", agent.id)
    } else {
        agent.id.clone()
    }
}

fn version_label(agent: &norte_code_wire::AgentSummary) -> String {
    if agent.version.is_empty() {
        "not reported".to_owned()
    } else {
        agent.version.clone()
    }
}

fn append_capabilities(fields: &mut Vec<Field>, capabilities: &norte_code_wire::AgentCapabilities) {
    fields.push(Field::new("accepts", modality_list(&capabilities.input)));
    fields.push(Field::new("produces", modality_list(&capabilities.output)));
    for (modality, limits) in &capabilities.limits {
        let mut details = Vec::new();
        if !limits.mime_types.is_empty() {
            details.push(limits.mime_types.join(", "));
        }
        if let Some(bytes) = limits.max_bytes {
            details.push(format!("{} max", human_bytes(bytes)));
        }
        if let Some(items) = limits.max_items {
            details.push(format!("{items} parts"));
        }
        if let Some(duration) = limits.max_duration_ms {
            details.push(format!("{} max duration", millis(duration)));
        }
        if !details.is_empty() {
            fields.push(Field::new(
                format!("{} limits", modality_label(*modality)),
                details.join(" · "),
            ));
        }
    }
}

fn plugin_list(profile: &norte_code_wire::AgentProfileSummary) -> String {
    if profile.plugins.is_empty() {
        return "none configured".to_owned();
    }
    profile
        .plugins
        .iter()
        .map(|plugin| {
            format!(
                "{}{}",
                plugin.id,
                if plugin.allow_user_messages {
                    " (messages)"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn mcp_list(profile: &norte_code_wire::AgentProfileSummary) -> String {
    if profile.mcp_servers.is_empty() {
        return "none configured".to_owned();
    }
    profile
        .mcp_servers
        .iter()
        .map(|server| {
            format!(
                "{} · {} {} · allow {} / deny {}",
                server.name,
                if server.remote { "remote" } else { "local" },
                if server.enabled { "on" } else { "off" },
                if server.allow_count == 0 {
                    "all".to_owned()
                } else {
                    server.allow_count.to_string()
                },
                server.deny_count
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}
