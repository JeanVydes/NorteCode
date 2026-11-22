//! Per-plugin permissions stored in the agent configuration.

use std::collections::BTreeMap;

use norte_code_wire::PluginId;
use serde::{Deserialize, Serialize};

/// What one plugin is allowed to ask the host to do.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginPolicyConfig {
    /// Whether this plugin may request user-role messages.
    pub allow_user_messages: bool,
}

/// Plugin permissions, keyed by the name the plugin reports.
pub type PluginPolicies = BTreeMap<PluginId, PluginPolicyConfig>;

/// Whether a named plugin may request user-role messages.
#[must_use]
pub fn allows_user_messages(policies: &PluginPolicies, plugin: &PluginId) -> bool {
    policies
        .get(plugin)
        .is_some_and(|policy| policy.allow_user_messages)
}
