//! One of the service's domains, as `impl Service` blocks.
//!
//! Child modules see the service's private fields, so a domain owns its
//! behaviour here while the state stays in one place.

use std::sync::Arc;

use norte_code_protocol::skills::SkillRegistry;

use skills::discover_skills;

#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    /// Re-scans the skill roots and returns how many valid skills remain.
    ///
    /// The next engine build registers the refreshed tools; a turn already in
    /// flight keeps the registry it started with.
    pub fn refresh_skills(&self) -> usize {
        let registry = Arc::new(discover_skills(&self.lock_config(), &self.workspace_root));
        let count = registry.len();
        for warning in registry.warnings() {
            tracing::warn!(
                path = %warning.path.display(),
                "skipping skill: {}",
                warning.message
            );
        }
        *self.lock_skills_write() = registry;
        count
    }
    /// The skills registry in use.
    #[must_use]
    pub fn skills(&self) -> Arc<SkillRegistry> {
        Arc::clone(&self.lock_skills())
    }
}
