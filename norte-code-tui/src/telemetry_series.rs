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
    /// Prompt tokens reported by providers.
    pub(crate) input: f64,
    /// Answer tokens reported by providers.
    pub(crate) output: f64,
    /// Prompt tokens served from a cache.
    pub(crate) cache_read: f64,
    /// Prompt tokens the providers reported, the cache-rate denominator.
    pub(crate) prompt: f64,
    /// Sum of measured request durations.
    pub(crate) duration_total: u64,
    /// Count of request durations that were measured live.
    pub(crate) duration_samples: u64,
    /// Sum of first-output timings.
    pub(crate) first_output_total: u64,
    /// Count of first-output timings.
    pub(crate) first_output_samples: u64,
    /// Sum of first-visible-text timings.
    pub(crate) first_text_total: u64,
    /// Count of first-visible-text timings.
    pub(crate) first_text_samples: u64,
}

impl Bucket {
    fn merge(&mut self, other: &Self) {
        self.requests += other.requests;
        self.cost += other.cost;
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.prompt += other.prompt;
        self.duration_total = self.duration_total.saturating_add(other.duration_total);
        self.duration_samples = self.duration_samples.saturating_add(other.duration_samples);
        self.first_output_total = self
            .first_output_total
            .saturating_add(other.first_output_total);
        self.first_output_samples = self
            .first_output_samples
            .saturating_add(other.first_output_samples);
        self.first_text_total = self.first_text_total.saturating_add(other.first_text_total);
        self.first_text_samples = self
            .first_text_samples
            .saturating_add(other.first_text_samples);
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

/// Time series ready for the telemetry charts.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Series {
    /// Cost candles: open and close bracket the spend added in each bucket.
    pub(crate) cost: Vec<Candle>,
    /// Tokens per bucket.
    /// Prompt tokens per bucket.
    pub(crate) input: Vec<f64>,
    /// Answer tokens per bucket.
    pub(crate) output: Vec<f64>,
    /// Requests per bucket.
    pub(crate) requests: Vec<f64>,
    /// Average measured request latency. Missing buckets are chart gaps.
    pub(crate) latency: Vec<Option<f64>>,
    /// Average first-output time. Missing buckets are chart gaps.
    pub(crate) first_output: Vec<Option<f64>>,
    /// Average first-visible-text time. Missing buckets are chart gaps.
    pub(crate) first_text: Vec<Option<f64>>,
    /// Cache hit percentage per bucket, in `0.0..=100.0`.
    pub(crate) cache: Vec<Option<f64>>,
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
            if record
                .metadata
                .get("source")
                .and_then(serde_json::Value::as_str)
                == Some("history")
            {
                continue;
            }
            let Some(key) = key(record, interval) else {
                continue;
            };
            let bucket = buckets.entry(key).or_default();
            let input = record.input_tokens;
            let output = record.output_tokens.unwrap_or(0);
            bucket.requests += 1;
            bucket.cost += record.cost_usd.unwrap_or(0.0);
            if let Some(input) = input {
                bucket.input = (bucket.input + input as f64).round();
                bucket.prompt = (bucket.prompt + input as f64).round();
                bucket.cache_read =
                    (bucket.cache_read + record.cache_read_tokens.unwrap_or(0) as f64).round();
            }
            bucket.output = (bucket.output + output as f64).round();
            bucket.duration_total = bucket.duration_total.saturating_add(record.duration_ms);
            bucket.duration_samples = bucket.duration_samples.saturating_add(1);
            if let Some(first_output) = record.first_token_ms {
                bucket.first_output_total = bucket.first_output_total.saturating_add(first_output);
                bucket.first_output_samples = bucket.first_output_samples.saturating_add(1);
            }
            if let Some(first_text) = record.first_text_ms {
                bucket.first_text_total = bucket.first_text_total.saturating_add(first_text);
                bucket.first_text_samples = bucket.first_text_samples.saturating_add(1);
            }
        }
    }

    let buckets: Vec<(String, Bucket)> =
        downsample(buckets.into_iter().collect(), MAX_CHART_BUCKETS);
    let mut cost = Vec::with_capacity(buckets.len());
    let mut input = Vec::with_capacity(buckets.len());
    let mut output = Vec::with_capacity(buckets.len());
    let mut requests = Vec::with_capacity(buckets.len());
    let mut latency = Vec::with_capacity(buckets.len());
    let mut first_output = Vec::with_capacity(buckets.len());
    let mut first_text = Vec::with_capacity(buckets.len());
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
        input.push(bucket.input);
        output.push(bucket.output);
        requests.push(bucket.requests as f64);
        latency.push(
            (bucket.duration_samples > 0)
                .then(|| bucket.duration_total as f64 / bucket.duration_samples as f64),
        );
        first_output.push(
            (bucket.first_output_samples > 0)
                .then(|| bucket.first_output_total as f64 / bucket.first_output_samples as f64),
        );
        first_text.push(
            (bucket.first_text_samples > 0)
                .then(|| bucket.first_text_total as f64 / bucket.first_text_samples as f64),
        );
        cache.push(
            (bucket.prompt > 0.0)
                .then(|| (bucket.cache_read / bucket.prompt * 100.0).clamp(0.0, 100.0)),
        );
    }
    Series {
        cost,
        input,
        output,
        requests,
        latency,
        first_output,
        first_text,
        cache,
        buckets,
    }
}

/// Compute the visible time window of the stacked charts from the bucket
/// count, the candle chart's zoom and scroll position. Pure, so the
/// arithmetic can be checked without a rendered tree.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn chart_window(
    total: usize,
    x_zoom: f64,
    width: u16,
    scroll_back: usize,
) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let cell_width = (2.0 * x_zoom).round().clamp(1.0, 16.0) as u16;
    let plot_width = width.saturating_sub(11).max(1);
    let visible = usize::from(plot_width / cell_width).max(1);
    let end = total.saturating_sub(scroll_back).max(1);
    (end.saturating_sub(visible), end)
}

#[cfg(test)]
#[path = "telemetry_series_tests.rs"]
mod tests;
