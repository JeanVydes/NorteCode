//! Time buckets drawn by the telemetry page.
//!
//! The host sends request records oldest first with their RFC 3339 start time.
//! A bucket key is the timestamp prefix for the chosen interval, so no date
//! library is needed and the lexical order is also chronological order.

use std::collections::BTreeMap;

use terminalui::components::candlechart::Candle;

use crate::telemetry::ChartInterval;

/// The charts draw at most this many buckets, matching the chart component.
pub(crate) const MAX_CHART_BUCKETS: usize = 240;

/// One time bucket of request measures.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Bucket {
    /// Requests in the bucket.
    pub(crate) requests: u64,
    /// Provider-reported spend, in USD.
    pub(crate) cost: f64,
    /// Prompt plus answer tokens.
    pub(crate) tokens: f64,
    /// Prompt tokens served from a cache.
    pub(crate) cache_read: f64,
    /// Prompt tokens the providers reported, the cache-rate denominator.
    pub(crate) prompt: f64,
}

impl Bucket {
    fn merge(&mut self, other: &Self) {
        self.requests += other.requests;
        self.cost += other.cost;
        self.tokens += other.tokens;
        self.cache_read += other.cache_read;
        self.prompt += other.prompt;
    }
}

/// Merge adjacent buckets until at most `limit` remain.
fn downsample(buckets: Vec<(String, Bucket)>, limit: usize) -> Vec<(String, Bucket)> {
    if limit == 0 || buckets.len() <= limit {
        return buckets;
    }
    let base = buckets.len() / limit;
    let extra = buckets.len() % limit;
    let mut merged = Vec::with_capacity(limit);
    let mut source = buckets.into_iter();
    for group in 0..limit {
        let size = base + usize::from(group < extra);
        let Some(mut current) = source.next() else {
            break;
        };
        for _ in 1..size {
            if let Some((_, bucket)) = source.next() {
                current.1.merge(&bucket);
            }
        }
        merged.push(current);
    }
    merged
}

/// Series ready for the three charts.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Series {
    /// Cost candles: open and close bracket the spend added in each bucket.
    pub(crate) cost: Vec<Candle>,
    /// Tokens per bucket.
    pub(crate) tokens: Vec<f64>,
    /// Cache hit percentage per bucket, in `0.0..=100.0`.
    pub(crate) cache: Vec<f64>,
    /// The buckets, for captions and labels.
    pub(crate) buckets: Vec<(String, Bucket)>,
}

fn key(record: &norte_code_wire::TelemetryRecord, interval: ChartInterval) -> Option<String> {
    let width = match interval {
        ChartInterval::Minute => 16,
        ChartInterval::Hour => 13,
        ChartInterval::Day => 10,
    };
    let stamp = record.started_at.as_str();
    stamp.get(..width).map(str::to_owned)
}

/// Fold the report's records into buckets for the chosen interval.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn series(
    report: Option<&norte_code_wire::TelemetryReport>,
    interval: ChartInterval,
) -> Series {
    let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
    if let Some(report) = report {
        for record in &report.records {
            let Some(key) = key(record, interval) else {
                continue;
            };
            let bucket = buckets.entry(key).or_default();
            let input = record.input_tokens.unwrap_or(0);
            let output = record.output_tokens.unwrap_or(0);
            let read = record.cache_read_tokens.unwrap_or(0);
            bucket.requests += 1;
            bucket.cost += record.cost_usd.unwrap_or(0.0);
            bucket.tokens = (bucket.tokens + input as f64 + output as f64).round();
            bucket.cache_read = (bucket.cache_read + read as f64).round();
            bucket.prompt = (bucket.prompt + input as f64).round();
        }
    }

    let buckets: Vec<(String, Bucket)> =
        downsample(buckets.into_iter().collect(), MAX_CHART_BUCKETS);
    let mut cost = Vec::with_capacity(buckets.len());
    let mut tokens = Vec::with_capacity(buckets.len());
    let mut cache = Vec::with_capacity(buckets.len());
    let mut running = 0.0f64;
    for (_, bucket) in &buckets {
        let open = running;
        running += bucket.cost;
        let close = running;
        cost.push(Candle {
            open,
            high: close.max(open),
            low: close.min(open),
            close,
            volume: bucket.requests as f64,
        });
        tokens.push(bucket.tokens);
        cache.push(if bucket.prompt > 0.0 {
            (bucket.cache_read / bucket.prompt * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        });
    }
    Series {
        cost,
        tokens,
        cache,
        buckets,
    }
}

#[cfg(test)]
#[path = "telemetry_series_tests.rs"]
mod tests;
