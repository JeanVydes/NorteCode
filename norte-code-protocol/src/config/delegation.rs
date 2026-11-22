//! Bounds on agent delegation.

use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::loading::invalid;

/// Default maximum nesting of child sessions.
pub const DEFAULT_MAX_DELEGATION_DEPTH: usize = 3;
/// Default number of child sessions that may run at once.
pub const DEFAULT_MAX_CONCURRENT_DELEGATES: usize = 3;
/// Default number of parent messages held for a running child.
pub const DEFAULT_MAX_HELD_DELEGATOR_MESSAGES: usize = 20;

/// How much delegation one turn may start and keep.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DelegationConfig {
    /// How deep a chain of child sessions may go.
    pub max_depth: usize,
    /// How many child sessions may run at once.
    pub max_concurrent: usize,
    /// How many parent messages are replayed when a child finishes.
    pub max_held_messages: usize,
}

impl Default for DelegationConfig {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DELEGATION_DEPTH,
            max_concurrent: DEFAULT_MAX_CONCURRENT_DELEGATES,
            max_held_messages: DEFAULT_MAX_HELD_DELEGATOR_MESSAGES,
        }
    }
}

impl DelegationConfig {
    /// Validates the delegation bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] naming the first offending field.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=8).contains(&self.max_depth) {
            return Err(invalid("delegation.max_depth", "must be between 1 and 8"));
        }
        if !(1..=64).contains(&self.max_concurrent) {
            return Err(invalid(
                "delegation.max_concurrent",
                "must be between 1 and 64",
            ));
        }
        if !(1..=1_000).contains(&self.max_held_messages) {
            return Err(invalid(
                "delegation.max_held_messages",
                "must be between 1 and 1000",
            ));
        }
        Ok(())
    }
}
