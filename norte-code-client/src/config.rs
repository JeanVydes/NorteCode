//! Client-side settings read from `cli.json`.
//!
//! These limits describe how a client behaves. They do not belong to the
//! server configuration: changing them never changes a model request.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default number of Tokio worker threads for the local service.
pub const DEFAULT_TOKIO_WORKERS: usize = 4;
/// Default time to wait for a local service to become ready.
pub const DEFAULT_STARTUP_WAIT_MS: u64 = 20_000;
/// Default interval between readiness checks while starting.
pub const DEFAULT_STARTUP_POLL_MS: u64 = 150;
/// Default time to wait for a graceful shutdown.
pub const DEFAULT_STOP_WAIT_MS: u64 = 10_000;
/// Default interval between process checks while stopping.
pub const DEFAULT_STOP_POLL_MS: u64 = 100;
/// Default grace after sending a kill signal.
pub const DEFAULT_KILL_SETTLE_MS: u64 = 200;

/// Default delay before the first reconnect.
pub const DEFAULT_RECONNECT_BASE_MS: u64 = 400;
/// Default ceiling for a reconnect delay.
pub const DEFAULT_RECONNECT_MAX_MS: u64 = 5_000;
/// Default number of failed attempts before the client reports giving up.
pub const DEFAULT_GIVE_UP_ATTEMPTS: u32 = 3;
/// Default time a healthy stream must stay open before resets count.
pub const DEFAULT_HEALTHY_STREAM_S: u64 = 10;

/// Default number of transcript rows held in memory.
pub const DEFAULT_TRANSCRIPT_MAX_ITEMS: usize = 1_000;
/// Default number of transcript rows kept after a trim.
pub const DEFAULT_TRANSCRIPT_KEEP_ITEMS: usize = 800;
/// Default number of console lines held in memory.
pub const DEFAULT_CONSOLE_MAX_LINES: usize = 500;
/// Default number of console lines kept after a trim.
pub const DEFAULT_CONSOLE_KEEP_LINES: usize = 400;

/// A client runtime setting is outside the range the client can honour.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("invalid cli config: {field}: {message}")]
pub struct CliConfigError {
    /// Dotted path of the offending field.
    pub field: String,
    /// Why the value was refused.
    pub message: String,
}

fn invalid(field: impl Into<String>, message: impl Into<String>) -> CliConfigError {
    CliConfigError {
        field: field.into(),
        message: message.into(),
    }
}

/// Runtime behavior owned by client applications rather than the service.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CliRuntimeConfig {
    /// Tokio workers and local process lifecycle.
    pub service_lifecycle: ServiceLifecycleConfig,
    /// Reconnect backoff.
    pub reconnect: ReconnectConfig,
    /// Display history retention.
    pub retention: RetentionConfig,
}

impl CliRuntimeConfig {
    /// Validates every client runtime limit.
    ///
    /// # Errors
    ///
    /// Returns [`CliConfigError`] naming the first offending field.
    pub fn validate(&self) -> Result<(), CliConfigError> {
        self.service_lifecycle.validate()?;
        self.reconnect.validate()?;
        self.retention.validate()?;
        Ok(())
    }
}

/// Tokio workers and process lifecycle for a service started by this CLI.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceLifecycleConfig {
    /// Number of Tokio worker threads. The CLI clamps this to the CPUs available.
    pub tokio_workers: usize,
    /// How long to wait for the service to report ready.
    pub startup_wait_ms: u64,
    /// Interval between readiness checks.
    pub startup_poll_ms: u64,
    /// How long to wait for a graceful stop.
    pub stop_wait_ms: u64,
    /// Interval between process checks while stopping.
    pub stop_poll_ms: u64,
    /// Grace allowed after sending a kill signal.
    pub kill_settle_ms: u64,
}

impl Default for ServiceLifecycleConfig {
    fn default() -> Self {
        Self {
            tokio_workers: DEFAULT_TOKIO_WORKERS,
            startup_wait_ms: DEFAULT_STARTUP_WAIT_MS,
            startup_poll_ms: DEFAULT_STARTUP_POLL_MS,
            stop_wait_ms: DEFAULT_STOP_WAIT_MS,
            stop_poll_ms: DEFAULT_STOP_POLL_MS,
            kill_settle_ms: DEFAULT_KILL_SETTLE_MS,
        }
    }
}

impl ServiceLifecycleConfig {
    /// Validates the local service lifecycle.
    ///
    /// # Errors
    ///
    /// Returns [`CliConfigError`] naming the first offending field.
    pub fn validate(&self) -> Result<(), CliConfigError> {
        if self.tokio_workers == 0 || self.tokio_workers > 1024 {
            return Err(invalid(
                "service_lifecycle.tokio_workers",
                "must be between 1 and 1024",
            ));
        }
        if self.startup_wait_ms == 0 {
            return Err(invalid(
                "service_lifecycle.startup_wait_ms",
                "must be greater than zero",
            ));
        }
        if self.startup_poll_ms == 0 || self.startup_poll_ms > self.startup_wait_ms {
            return Err(invalid(
                "service_lifecycle.startup_poll_ms",
                "must be positive and not exceed startup_wait_ms",
            ));
        }
        if self.stop_wait_ms == 0 {
            return Err(invalid(
                "service_lifecycle.stop_wait_ms",
                "must be greater than zero",
            ));
        }
        if self.stop_poll_ms == 0 || self.stop_poll_ms > self.stop_wait_ms {
            return Err(invalid(
                "service_lifecycle.stop_poll_ms",
                "must be positive and not exceed stop_wait_ms",
            ));
        }
        if self.kill_settle_ms > 10_000 {
            return Err(invalid(
                "service_lifecycle.kill_settle_ms",
                "must be at most 10000",
            ));
        }
        Ok(())
    }
}

/// Backoff used by a client when the event stream drops.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReconnectConfig {
    /// Delay before the first reconnect.
    pub retry_base_ms: u64,
    /// Largest delay between reconnects.
    pub retry_max_ms: u64,
    /// Failed attempts before the client reports giving up.
    pub give_up_attempts: u32,
    /// Time a healthy stream must stay open before resets are cleared.
    pub healthy_stream_s: u64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            retry_base_ms: DEFAULT_RECONNECT_BASE_MS,
            retry_max_ms: DEFAULT_RECONNECT_MAX_MS,
            give_up_attempts: DEFAULT_GIVE_UP_ATTEMPTS,
            healthy_stream_s: DEFAULT_HEALTHY_STREAM_S,
        }
    }
}

impl ReconnectConfig {
    /// Validates the reconnect backoff.
    ///
    /// # Errors
    ///
    /// Returns [`CliConfigError`] naming the first offending field.
    pub fn validate(&self) -> Result<(), CliConfigError> {
        if self.retry_base_ms == 0 || self.retry_base_ms > 60_000 {
            return Err(invalid(
                "reconnect.retry_base_ms",
                "must be between 1 and 60000",
            ));
        }
        if self.retry_max_ms < self.retry_base_ms || self.retry_max_ms > 3_600_000 {
            return Err(invalid(
                "reconnect.retry_max_ms",
                "must be at least retry_base_ms and at most 3600000",
            ));
        }
        if self.give_up_attempts == 0 || self.give_up_attempts > 20 {
            return Err(invalid(
                "reconnect.give_up_attempts",
                "must be between 1 and 20",
            ));
        }
        if self.healthy_stream_s == 0 || self.healthy_stream_s > 3_600 {
            return Err(invalid(
                "reconnect.healthy_stream_s",
                "must be between 1 and 3600",
            ));
        }
        Ok(())
    }
}

/// How much conversation and console history a client keeps.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionConfig {
    /// Largest number of transcript rows held in memory.
    pub transcript_max_items: usize,
    /// Number of transcript rows kept after a trim.
    pub transcript_keep_items: usize,
    /// Largest number of console lines held in memory.
    pub console_max_lines: usize,
    /// Number of console lines kept after a trim.
    pub console_keep_lines: usize,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            transcript_max_items: DEFAULT_TRANSCRIPT_MAX_ITEMS,
            transcript_keep_items: DEFAULT_TRANSCRIPT_KEEP_ITEMS,
            console_max_lines: DEFAULT_CONSOLE_MAX_LINES,
            console_keep_lines: DEFAULT_CONSOLE_KEEP_LINES,
        }
    }
}

impl RetentionConfig {
    /// Validates the transcript and console retention.
    ///
    /// # Errors
    ///
    /// Returns [`CliConfigError`] naming the first offending field.
    pub fn validate(&self) -> Result<(), CliConfigError> {
        if self.transcript_max_items == 0
            || self.transcript_keep_items == 0
            || self.transcript_keep_items > self.transcript_max_items
        {
            return Err(invalid(
                "retention",
                "transcript_keep_items must be positive and not exceed transcript_max_items",
            ));
        }
        if self.console_max_lines == 0
            || self.console_keep_lines == 0
            || self.console_keep_lines > self.console_max_lines
        {
            return Err(invalid(
                "retention",
                "console_keep_lines must be positive and not exceed console_max_lines",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
