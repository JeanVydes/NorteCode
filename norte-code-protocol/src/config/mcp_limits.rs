//! Runtime limits for MCP servers.

use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::loading::invalid;

/// Default deadline for one MCP request.
pub const DEFAULT_MCP_REQUEST_TIMEOUT_MS: u64 = 60_000;

/// Limits shared by every configured MCP server.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct McpLimitsConfig {
    /// Deadline for a handshake, tool listing or tool call.
    pub request_timeout_ms: u64,
}

impl Default for McpLimitsConfig {
    fn default() -> Self {
        Self {
            request_timeout_ms: DEFAULT_MCP_REQUEST_TIMEOUT_MS,
        }
    }
}

impl McpLimitsConfig {
    /// Validates the MCP request timeout.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when the timeout is out of range.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1_000..=600_000).contains(&self.request_timeout_ms) {
            return Err(invalid(
                "mcp_limits.request_timeout_ms",
                "must be between 1000 and 600000 milliseconds",
            ));
        }
        Ok(())
    }
}
