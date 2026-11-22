//! Operational limits of the RAEP service host.

use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::loading::invalid;

/// Default number of frames kept for replay.
pub const DEFAULT_REPLAY_CAPACITY: usize = 2_000;
/// Default bytes retained across replay frames.
pub const DEFAULT_REPLAY_BYTES: u64 = 8 * 1024 * 1024;
/// Default number of jobs that may run at once.
pub const DEFAULT_MAX_CONCURRENT_JOBS: usize = 8;
/// Default retention of finished job output, in seconds.
pub const DEFAULT_JOB_OUTPUT_RETENTION_S: u64 = 86_400;
/// Default number of remembered turn request fingerprints.
pub const DEFAULT_IDEMPOTENCY_CAPACITY: usize = 4_096;
/// Default number of inputs waiting behind a running turn.
pub const DEFAULT_MAX_QUEUED_INPUTS: usize = 8;
/// Default largest accepted frame, in bytes.
pub const DEFAULT_MAX_FRAME_BYTES: u64 = 1024 * 1024;
/// Default largest accepted artifact, in bytes.
pub const DEFAULT_MAX_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;
/// Default largest exported bundle, in bytes.
pub const DEFAULT_MAX_EXPORT_BYTES: u64 = 64 * 1024 * 1024;
/// Default lifetime of a generated export, in seconds.
pub const DEFAULT_EXPORT_TTL_S: u64 = 3_600;
/// Default number of concurrent WebSocket and SSE clients.
pub const DEFAULT_MAX_CONNECTIONS: usize = 64;
/// Default deadline for handling one command, in milliseconds.
pub const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 120_000;

/// Capacity and retention limits owned by the RAEP host.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RaepLimitsConfig {
    /// Frames kept for replay.
    pub replay_capacity: usize,
    /// Bytes retained across replay frames.
    pub replay_bytes: u64,
    /// Jobs that may run at once.
    pub max_concurrent_jobs: usize,
    /// Seconds a finished job's output is kept.
    pub job_output_retention_s: u64,
    /// Turn request fingerprints kept for idempotency.
    pub idempotency_capacity: usize,
    /// Inputs waiting behind a running turn.
    pub max_queued_inputs: usize,
    /// Largest accepted frame.
    pub max_frame_bytes: u64,
    /// Largest accepted uploaded artifact.
    pub max_artifact_bytes: u64,
    /// Largest exported bundle.
    pub max_export_bytes: u64,
    /// Seconds a generated bundle can be downloaded.
    pub export_ttl_s: u64,
    /// Concurrent WebSocket and SSE clients.
    pub max_connections: usize,
    /// Deadline for handling one command.
    pub command_timeout_ms: u64,
}

impl Default for RaepLimitsConfig {
    fn default() -> Self {
        Self {
            replay_capacity: DEFAULT_REPLAY_CAPACITY,
            replay_bytes: DEFAULT_REPLAY_BYTES,
            max_concurrent_jobs: DEFAULT_MAX_CONCURRENT_JOBS,
            job_output_retention_s: DEFAULT_JOB_OUTPUT_RETENTION_S,
            idempotency_capacity: DEFAULT_IDEMPOTENCY_CAPACITY,
            max_queued_inputs: DEFAULT_MAX_QUEUED_INPUTS,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_artifact_bytes: DEFAULT_MAX_ARTIFACT_BYTES,
            max_export_bytes: DEFAULT_MAX_EXPORT_BYTES,
            export_ttl_s: DEFAULT_EXPORT_TTL_S,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            command_timeout_ms: DEFAULT_COMMAND_TIMEOUT_MS,
        }
    }
}

impl RaepLimitsConfig {
    /// Validates the RAEP capacity and retention limits.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] naming the first offending field.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let counts = [
            ("replay_capacity", self.replay_capacity),
            ("max_concurrent_jobs", self.max_concurrent_jobs),
            ("idempotency_capacity", self.idempotency_capacity),
            ("max_queued_inputs", self.max_queued_inputs),
            ("max_connections", self.max_connections),
        ];
        for (field, value) in counts {
            if value == 0 {
                return Err(invalid(
                    format!("raep_limits.{field}"),
                    "must be greater than zero",
                ));
            }
        }
        let bytes = [
            ("replay_bytes", self.replay_bytes),
            ("max_frame_bytes", self.max_frame_bytes),
            ("max_artifact_bytes", self.max_artifact_bytes),
            ("max_export_bytes", self.max_export_bytes),
        ];
        for (field, value) in bytes {
            if value == 0 {
                return Err(invalid(
                    format!("raep_limits.{field}"),
                    "must be greater than zero",
                ));
            }
        }
        if self.job_output_retention_s == 0 || self.export_ttl_s == 0 {
            return Err(invalid(
                "raep_limits",
                "job_output_retention_s and export_ttl_s must be greater than zero",
            ));
        }
        if !(1..=600_000).contains(&self.command_timeout_ms) {
            return Err(invalid(
                "raep_limits.command_timeout_ms",
                "must be between 1 and 600000 milliseconds",
            ));
        }
        Ok(())
    }
}
