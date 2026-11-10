//! The engine's own settings: compaction, retries and reasoning.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// When an automatic compaction may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    /// Only at an idle turn boundary (and before the next turn on resume or
    /// switch to a smaller model).
    #[default]
    Idle,
    /// At the first safe model-request boundary inside a turn.
    Eager,
}

/// Compaction settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    /// Whether compaction may run.
    pub enabled: bool,
    /// The used-context fraction that triggers it.
    pub threshold: f64,
    /// When an automatic compaction may run: `idle` (the default) only at an
    /// idle turn boundary, or `eager` at the first safe model-request boundary
    /// inside a turn. Eager still never splits a tool call from its results.
    pub trigger: CompactionTrigger,
    /// How many recent messages to keep verbatim.
    pub keep_recent: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.90,
            trigger: CompactionTrigger::default(),
            keep_recent: 6,
        }
    }
}

/// Automatic retries for transient provider failures.
///
/// The host retries by resuming the turn on the same session, so the user's
/// message is never sent twice. Only failures that can clear on their own are
/// retried: rate limits, overload, server errors and transport errors. A quota
/// or authentication failure is permanent and fails the turn immediately.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// How many times to retry after the first failure. `0` disables retries.
    pub max_attempts: u32,
    /// The longest wait the host will accept, in milliseconds.
    pub max_delay_ms: u64,
    /// The first fallback wait when the provider gives no `retry_after`, in
    /// milliseconds. It doubles on each attempt, up to `max_delay_ms`.
    pub backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            max_delay_ms: 60_000,
            backoff_ms: 5_000,
        }
    }
}

/// The default reasoning setting.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReasoningConfig {
    /// `auto`, `off`, an effort name, or a `{budget}` object.
    pub default: Value,
}

impl Default for ReasoningConfig {
    fn default() -> Self {
        Self {
            default: Value::String("auto".to_owned()),
        }
    }
}
