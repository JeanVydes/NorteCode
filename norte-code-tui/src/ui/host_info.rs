//! The fields shown on the host tab: server, health, quotas and agents.

use super::detail_panel::Field;
use crate::ui_model::UiModel;

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1_024;
    const MIB: u64 = 1_024 * KIB;
    if bytes >= MIB {
        format!("{} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn millis(ms: u64) -> String {
    if ms >= 1_000 {
        format!("{} s", ms / 1_000)
    } else {
        format!("{ms} ms")
    }
}

fn epoch(value: u64) -> String {
    if value == 0 {
        return "–".to_owned();
    }
    // RFC 3339 needs a time library; a compact epoch is honest and dependency free.
    format!("{value}")
}

/// Everything the client knows about the server it is attached to.
pub(crate) fn fields(model: &UiModel) -> Vec<Field> {
    let mut fields = Vec::new();
    fields.push(Field::new(
        "address",
        if model.service_url.is_empty() {
            "–"
        } else {
            &model.service_url
        },
    ));

    if let Some(info) = &model.server_info {
        fields.push(Field::new("version", &info.server_version));
        if let Some(protocol) = &info.protocol {
            fields.push(Field::new(
                "protocol",
                format!("{} {}", protocol.name, protocol.version),
            ));
        }
        fields.push(Field::new("default agent", &info.default_agent));
        if !info.capabilities.is_empty() {
            fields.push(Field::new("capabilities", info.capabilities.join(", ")));
        }
        if !info.extensions.is_empty() {
            fields.push(Field::new("extensions", info.extensions.join(", ")));
        }
        let limits = &info.limits;
        if let Some(bytes) = limits.frame_bytes {
            fields.push(Field::new("max frame", human_bytes(bytes)));
        }
        if let Some(count) = limits.concurrent_jobs {
            fields.push(Field::new("max jobs", count.to_string()));
        }
        if let Some(bytes) = limits.artifact_bytes {
            fields.push(Field::new("max artifact", human_bytes(bytes)));
        }
        if let Some(bytes) = limits.prompt_bytes {
            fields.push(Field::new("max prompt", human_bytes(bytes)));
        }
    }

    if let Some(live) = &model.liveness {
        fields.push(Field::new("liveness", format!("{:?}", live.status)));
    }
    if let Some(ready) = &model.readiness {
        fields.push(Field::new("readiness", format!("{:?}", ready.status)));
        fields.push(Field::new("agents", ready.agents.to_string()));
        fields.push(Field::new("replay", ready.replay.to_string()));
        fields.push(Field::new("artifacts", ready.artifacts.to_string()));
        fields.push(Field::new(
            "connections",
            ready.limits.max_connections.to_string(),
        ));
        fields.push(Field::new(
            "timeout",
            millis(ready.limits.command_timeout_ms),
        ));
    }

    if let Some(quota) = &model.quota {
        fields.push(Field::new("tenant", &quota.tenant));
        fields.push(Field::new(
            "quota period",
            format!(
                "{} .. {}",
                epoch(quota.period.start),
                epoch(quota.period.end)
            ),
        ));
        for dimension in &quota.dimensions {
            fields.push(Field::new(
                dimension.name.label(),
                format!(
                    "{}/{} ({} left)",
                    dimension.used, dimension.limit, dimension.remaining
                ),
            ));
        }
    }

    for agent in &model.agents {
        let version = if agent.version.is_empty() {
            String::new()
        } else {
            format!(" v{}", agent.version)
        };
        fields.push(Field::new("agent", format!("{}{}", agent.id, version)));
        if let Some(provider) = &agent.provider {
            fields.push(Field::new("  provider", &provider.id));
        }
        if let Some(agent_model) = &agent.model {
            fields.push(Field::new("  model", &agent_model.id));
        }
    }

    fields
}
