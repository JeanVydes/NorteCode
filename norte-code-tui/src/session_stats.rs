//! Telemetry rolled up per session, for the sessions page.
//!
//! The fleet telemetry report carries the session id on every record, so the
//! page can show each session's usage without a request per row.

use std::collections::{BTreeMap, BTreeSet};

use norte_code_wire::{RequestStatus, TelemetryRecord};

/// What one session spent across its reported requests.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SessionUsage {
    /// Requests reported for the session.
    pub(crate) requests: u64,
    /// Prompt tokens.
    pub(crate) input: u64,
    /// Answer tokens.
    pub(crate) output: u64,
    /// Prompt tokens served from a cache.
    pub(crate) cache_read: u64,
    /// Tool calls made by the model.
    pub(crate) tools: u64,
    /// Distinct turns the requests belong to.
    pub(crate) turns: BTreeSet<u64>,
    /// Requests per model id, most used first.
    pub(crate) models: BTreeMap<String, u64>,
    /// Failed requests.
    pub(crate) errors: u64,
    /// Cancelled requests.
    pub(crate) cancelled: u64,
    /// Total request time in milliseconds.
    pub(crate) duration_ms: u64,
    /// Provider-reported spend in USD, when any was reported.
    pub(crate) cost: f64,
    /// Whether any record reported a cost.
    pub(crate) has_cost: bool,
}

impl SessionUsage {
    /// Prompt tokens served from cache, as a percentage of prompt tokens.
    pub(crate) fn cache_rate(&self) -> f64 {
        if self.input == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let rate = self.cache_read as f64 / self.input as f64 * 100.0;
        rate.clamp(0.0, 100.0)
    }

    /// The most used models, up to `limit`.
    pub(crate) fn top_models(&self, limit: usize) -> Vec<(&str, u64)> {
        let mut models: Vec<(&str, u64)> = self
            .models
            .iter()
            .map(|(model, count)| (model.as_str(), *count))
            .collect();
        models.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
        models.truncate(limit);
        models
    }
}

fn add(usage: &mut SessionUsage, record: &TelemetryRecord) {
    usage.requests += 1;
    usage.input += record.input_tokens.unwrap_or(0);
    usage.output += record.output_tokens.unwrap_or(0);
    usage.cache_read += record.cache_read_tokens.unwrap_or(0);
    usage.tools += record.tool_calls.len() as u64;
    usage.turns.insert(record.turn);
    *usage.models.entry(record.model.clone()).or_default() += 1;
    match record.status {
        RequestStatus::Error => usage.errors += 1,
        RequestStatus::Cancelled => usage.cancelled += 1,
        RequestStatus::Ok => {}
    }
    usage.duration_ms += record.duration_ms;
    if let Some(cost) = record.cost_usd {
        usage.cost += cost;
        usage.has_cost = true;
    }
}

/// Group every record by session in one pass; the sessions page uses this to
/// enrich its rows without rescanning the report for each session.
pub(crate) fn by_session(records: &[TelemetryRecord]) -> BTreeMap<String, SessionUsage> {
    let mut grouped = BTreeMap::new();
    for record in records {
        add(grouped.entry(record.session.clone()).or_default(), record);
    }
    grouped
}

#[cfg(test)]
#[path = "session_stats_tests.rs"]
mod tests;
