//! Skill discovery and secret collection.

use std::path::Path;

use norte_code_protocol::catalog::Catalog;
use norte_code_protocol::config::Config;
use norte_code_protocol::skills::SkillRegistry;
use norte_code_wire::ServerEventKind;

use super::mapping::single;
use super::{EventStream, Service};

pub(super) fn discover_skills(config: &Config, workspace: &Path) -> SkillRegistry {
    if !config.skills.enabled {
        return SkillRegistry::discover(&[], 1);
    }
    let roots = norte_code_protocol::skills::discovery_roots(
        workspace,
        &config.skills.paths,
        config.skills.interop,
    );
    SkillRegistry::discover(&roots, config.skills.max_skills)
}

/// Every API key already known, for redaction.
///
/// Missing keys are not an error here: only the selected provider needs one, and
/// a provider that names no variable needs none at all. Every provider is
/// walked and not only the session's, because a command's output may quote any
/// of them and a key that reached a log is a key that leaked.
///
/// This reads env, the host's cache and the 0600 file, never the OS keyring: a
/// redaction pass at startup must not unlock a credential store once per
/// provider. A key resolved later is added by the switch that uses it.
pub(super) fn collect_secrets(config: &Config, catalog: &Catalog) -> Vec<String> {
    let mut secrets = Vec::new();
    for (id, provider) in catalog.providers() {
        if let Some(key) = config.known_api_key(id, &provider.environment_keys) {
            secrets.push(key.expose().to_owned());
        }
    }
    // An MCP server's `env` is where its token lives, and a command's output is
    // redacted against this list.
    secrets.extend(super::mcp::McpServers::secrets(&config.mcp_servers));
    secrets.retain(|secret| !secret.trim().is_empty());
    secrets.sort();
    secrets.dedup();
    secrets
}

impl Service {
    /// The skills the host found, with the warnings of the discovery.
    pub(super) fn skill_list_event(&self, request_id: u64) -> EventStream {
        let skills = self.skills();
        let mut warnings = Vec::new();
        let mut infos = Vec::new();
        for skill in skills.skills() {
            infos.push(norte_code_wire::SkillInfo {
                name: skill.name.clone(),
                description: skill.description.clone(),
                root: skill.root.display().to_string(),
                allowed_tools: skill.allowed_tools.clone(),
            });
        }
        for warning in skills.warnings() {
            warnings.push(format!("{}: {}", warning.path.display(), warning.message));
        }
        single(
            request_id,
            None,
            ServerEventKind::SkillList {
                skills: infos,
                warnings,
            },
        )
    }

    /// Looks for the skills again, after the workspace changed.
    pub(super) fn refresh_skills_event(&self, request_id: u64) -> EventStream {
        let count = self.refresh_skills();
        single(
            request_id,
            None,
            ServerEventKind::SkillsUpdated { skills: count },
        )
    }
}
