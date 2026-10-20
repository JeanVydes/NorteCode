//! Skill discovery and secret collection.

use std::path::Path;

use norte_code_catalog::Catalog;
use norte_code_config::Config;
use norte_code_skills::SkillRegistry;

pub(super) fn discover_skills(config: &Config, workspace: &Path) -> SkillRegistry {
    if !config.skills.enabled {
        return SkillRegistry::discover(&[], 1);
    }
    let roots =
        norte_code_skills::discovery_roots(workspace, &config.skills.paths, config.skills.interop);
    SkillRegistry::discover(&roots, config.skills.max_skills)
}

/// Every API key the config can resolve, for redaction in command output.
///
/// Missing keys are not an error here: only the selected provider needs one.
pub(super) fn collect_secrets(config: &Config, catalog: &Catalog) -> Vec<String> {
    let mut secrets = Vec::new();
    for (id, provider) in &catalog.providers {
        if let Ok(key) = config.resolve_api_key(id, &provider.env) {
            secrets.push(key.expose().to_owned());
        }
    }
    for id in config.providers.keys() {
        let env = catalog
            .provider(id)
            .map_or_else(Vec::new, |provider| provider.env.clone());
        if let Ok(key) = config.resolve_api_key(id, &env) {
            secrets.push(key.expose().to_owned());
        }
    }
    secrets.retain(|secret| !secret.trim().is_empty());
    secrets.sort();
    secrets.dedup();
    secrets
}
