//! Cost display.

use serde::{Deserialize, Serialize};

/// Cost display settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CostConfig {
    /// Whether to show cost in the UI and the CLI.
    pub show: bool,
}

impl Default for CostConfig {
    fn default() -> Self {
        Self { show: true }
    }
}
