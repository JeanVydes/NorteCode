//! Transport policy for provider requests made by Norte adapters.

use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::loading::invalid;

/// Default number of attempts to establish a connection.
pub const DEFAULT_CONNECT_MAX_RETRIES: u32 = 3;
/// Default deadline for establishing one connection, in milliseconds.
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 10_000;
/// Default deadline for a whole provider request, in milliseconds.
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 120_000;
/// Default time a stream may stay silent, in milliseconds.
pub const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;

/// Transport bounds shared by every provider adapter.
///
/// This is not the turn-level `retry` block. Connection retries happen before
/// a request reaches the provider. Turn retries happen after a provider fails.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderHttpConfig {
    /// Attempts made while establishing a connection.
    pub connect_max_retries: u32,
    /// Deadline for one connection attempt.
    pub connect_timeout_ms: u64,
    /// Deadline for the whole request, including the streamed body.
    pub request_timeout_ms: u64,
    /// Time a stream may send no bytes before it is treated as stuck.
    pub stream_idle_timeout_ms: u64,
}

impl Default for ProviderHttpConfig {
    fn default() -> Self {
        Self {
            connect_max_retries: DEFAULT_CONNECT_MAX_RETRIES,
            connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS,
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
            stream_idle_timeout_ms: DEFAULT_STREAM_IDLE_TIMEOUT_MS,
        }
    }
}

impl ProviderHttpConfig {
    /// Validates the provider transport policy.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] naming the first offending field.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.connect_max_retries > 10 {
            return Err(invalid(
                "provider_http.connect_max_retries",
                "must be at most 10",
            ));
        }
        if !(1..=60_000).contains(&self.connect_timeout_ms) {
            return Err(invalid(
                "provider_http.connect_timeout_ms",
                "must be between 1 and 60000 milliseconds",
            ));
        }
        if !(1..=3_600_000).contains(&self.request_timeout_ms) {
            return Err(invalid(
                "provider_http.request_timeout_ms",
                "must be between 1 and 3600000 milliseconds",
            ));
        }
        if !(1..=3_600_000).contains(&self.stream_idle_timeout_ms) {
            return Err(invalid(
                "provider_http.stream_idle_timeout_ms",
                "must be between 1 and 3600000 milliseconds",
            ));
        }
        if self.request_timeout_ms < self.connect_timeout_ms {
            return Err(invalid(
                "provider_http.request_timeout_ms",
                "must not be shorter than connect_timeout_ms",
            ));
        }
        Ok(())
    }
}
