//! The telemetry panel's data: what `/telemetry` reads and rolls up.
//!
//! The rows come from the `requests` table, one per provider attempt, and this
//! module is what turns them into the slices the panel draws: a range of time, a
//! provider and its models, a series over time, and the ranks a person switches
//! between. It holds no terminal and no state: a function of the rows, so the
//! panel's numbers can be checked without drawing a frame.
//!
//! # Models are not merged
//!
//! A model is keyed by `provider/model`, and two providers that publish the same
//! model name are two rows. `openai/gpt-5` and `openrouter/gpt-5` bill
//! differently, cache differently and are answered at different speeds, so
//! merging them would average away the only comparison worth making.
//!
//! # The pieces are built here and drawn elsewhere
//!
//! Nothing in this module knows about a terminal. It is a function of the rows,
//! so the panel's numbers — the ranks, the ranges, the error bars — are checked
//! against the rows and not against a screen.

#![allow(dead_code)] // the panel that draws this is built on top, frame by frame.
#![allow(clippy::cast_precision_loss)] // a token count is far under 2^52 in any real ledger.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use norte_code_store_sqlite::RequestRecord;

/// The window the panel totals over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Range {
    /// The last twenty-four hours.
    Today,
    /// The last seven days.
    Week,
    /// The last thirty days.
    Month,
    /// The last year.
    Year,
    /// Everything recorded.
    All,
}

impl Range {
    /// Every range, in the order the selector walks them.
    pub(crate) const ALL: [Self; 5] = [Self::Today, Self::Week, Self::Month, Self::Year, Self::All];

    /// The word the panel shows.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Week => "This Week",
            Self::Month => "This Month",
            Self::Year => "This Year",
            Self::All => "All Time",
        }
    }

    /// The next range, wrapping: what a key does when it walks the selector.
    #[must_use]
    pub(crate) fn next(self) -> Self {
        let at = Self::ALL
            .iter()
            .position(|range| *range == self)
            .unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// How far back the range reaches from `now`, or `None` for everything.
    #[must_use]
    fn since(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let span = match self {
            Self::Today => Duration::hours(24),
            Self::Week => Duration::days(7),
            Self::Month => Duration::days(30),
            Self::Year => Duration::days(365),
            Self::All => return None,
        };
        Some(now - span)
    }

    /// Whether a record falls inside the range at `now`.
    #[must_use]
    pub(crate) fn holds(self, record: &RequestRecord, now: DateTime<Utc>) -> bool {
        self.since(now)
            .is_none_or(|since| record.started_at >= since)
    }
}

/// A metric a rank can be ordered by.
///
/// It is what the `r` key walks: the same rows, ordered six ways, so a person
/// asking "who is expensive" and "who is slow" is one key apart on the same
/// list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Metric {
    /// Prompt and answer tokens together.
    Tokens,
    /// Answer tokens alone.
    Output,
    /// The share of the prompt served from the cache.
    CacheRate,
    /// The middle response time.
    Latency,
    /// The time to the first token.
    FirstToken,
    /// The money the provider reported.
    Cost,
    /// How many requests failed.
    Errors,
}

impl Metric {
    /// Every metric, in the order the key walks them.
    pub(crate) const ALL: [Self; 7] = [
        Self::Tokens,
        Self::Output,
        Self::CacheRate,
        Self::Latency,
        Self::FirstToken,
        Self::Cost,
        Self::Errors,
    ];

    /// The word the panel shows.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Tokens => "total tokens",
            Self::Output => "output tokens",
            Self::CacheRate => "cache rate",
            Self::Latency => "latency p50",
            Self::FirstToken => "time to first token",
            Self::Cost => "cost",
            Self::Errors => "errors",
        }
    }

    /// The next metric, wrapping.
    #[must_use]
    pub(crate) fn next(self) -> Self {
        let at = Self::ALL
            .iter()
            .position(|metric| *metric == self)
            .unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// The value a summary is ranked by, larger first.
    #[must_use]
    fn value(self, summary: &Slice) -> f64 {
        match self {
            Self::Tokens => summary.tokens() as f64,
            Self::Output => summary.output_tokens as f64,
            Self::CacheRate => summary.cache_rate().unwrap_or(0) as f64,
            Self::Latency => summary.p50_ms() as f64,
            Self::FirstToken => summary.p50_ttft_ms() as f64,
            Self::Cost => summary.cost_usd,
            Self::Errors => summary.errors as f64,
        }
    }
}

/// One request's error, kept whole so the bars can subdivide by kind.
#[derive(Clone, Debug, Default)]
pub(crate) struct ErrorTally {
    /// Failures by their typed kind, or `other` when none was reported.
    pub(crate) by_kind: BTreeMap<String, u64>,
}

impl ErrorTally {
    /// Records one failure.
    fn add(&mut self, record: &RequestRecord) {
        let kind = record
            .error_kind
            .clone()
            .filter(|kind| !kind.is_empty())
            .unwrap_or_else(|| "other".to_owned());
        *self.by_kind.entry(kind).or_insert(0) += 1;
    }

    /// The total failures of every kind.
    #[must_use]
    pub(crate) fn total(&self) -> u64 {
        self.by_kind.values().sum()
    }

    /// The kinds, most frequent first.
    #[must_use]
    pub(crate) fn ranked(&self) -> Vec<(&str, u64)> {
        let mut kinds: Vec<(&str, u64)> = self
            .by_kind
            .iter()
            .map(|(kind, count)| (kind.as_str(), *count))
            .collect();
        kinds.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
        kinds
    }
}

/// The rolled-up figures of one slice: a provider, a model or the lot.
#[derive(Clone, Debug, Default)]
pub(crate) struct Slice {
    /// How many requests are in the slice.
    pub(crate) requests: u64,
    /// Prompt tokens the provider reported.
    pub(crate) input_tokens: u64,
    /// Answer tokens the provider reported.
    pub(crate) output_tokens: u64,
    /// Reasoning tokens the provider reported.
    pub(crate) reasoning_tokens: u64,
    /// Prompt tokens served from the cache.
    pub(crate) cache_read_tokens: u64,
    /// Prompt tokens written to the cache.
    pub(crate) cache_write_tokens: u64,
    /// How many requests hit the cache at all.
    pub(crate) cache_hits: u64,
    /// How many tool calls were made.
    pub(crate) tool_calls: u64,
    /// The money the provider reported, summed.
    pub(crate) cost_usd: f64,
    /// How many requests carried no reported cost.
    pub(crate) unpriced: u64,
    /// How many requests failed.
    pub(crate) errors: u64,
    /// How many requests were cancelled.
    pub(crate) cancelled: u64,
    /// Every request's duration, for the percentiles.
    durations: Vec<u64>,
    /// Every request's time to first token, for the percentiles.
    ttfts: Vec<u64>,
    /// The failures, by kind, for the error bars.
    pub(crate) error_tally: ErrorTally,
}

impl Slice {
    /// Rolls one record into the slice.
    pub(crate) fn add(&mut self, record: &RequestRecord, now: DateTime<Utc>) {
        self.add_within(record, now);
    }

    fn add_within(&mut self, record: &RequestRecord, _now: DateTime<Utc>) {
        self.requests += 1;
        self.input_tokens += record.input_tokens.unwrap_or(0);
        self.output_tokens += record.output_tokens.unwrap_or(0);
        self.reasoning_tokens += record.reasoning_tokens.unwrap_or(0);
        self.cache_read_tokens += record.cache_read_tokens.unwrap_or(0);
        self.cache_write_tokens += record.cache_write_tokens.unwrap_or(0);
        self.cache_hits += u64::from(record.cache_hit);
        self.tool_calls += u64::try_from(record.tool_calls.len()).unwrap_or(u64::MAX);
        if let Some(cost) = record.cost_usd {
            self.cost_usd += cost;
        } else {
            self.unpriced += 1;
        }
        match record.status.as_str() {
            "ok" => {}
            "cancelled" => self.cancelled += 1,
            _ => {
                self.errors += 1;
                self.error_tally.add(record);
            }
        }
        self.durations.push(record.duration_ms);
        if let Some(first_token) = record.first_token_ms {
            self.ttfts.push(first_token);
        }
    }

    /// Prompt and answer tokens together.
    #[must_use]
    pub(crate) fn tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// The share of prompt tokens served from the cache, weighted by tokens.
    #[must_use]
    pub(crate) fn cache_rate(&self) -> Option<u64> {
        if self.input_tokens == 0 {
            return None;
        }
        Some(
            self.cache_read_tokens
                .saturating_mul(100)
                .checked_div(self.input_tokens)
                .unwrap_or(0)
                .min(100),
        )
    }

    /// The middle response time.
    #[must_use]
    pub(crate) fn p50_ms(&self) -> u64 {
        percentile(&self.durations, 50)
    }

    /// The response time three requests in four are at or under.
    #[must_use]
    pub(crate) fn p95_ms(&self) -> u64 {
        percentile(&self.durations, 95)
    }

    /// The middle wait for a first token.
    #[must_use]
    pub(crate) fn p50_ttft_ms(&self) -> u64 {
        percentile(&self.ttfts, 50)
    }

    /// The wait for a first token three requests in four are at or under.
    #[must_use]
    pub(crate) fn p95_ttft_ms(&self) -> u64 {
        percentile(&self.ttfts, 95)
    }

    /// The mean response time.
    #[must_use]
    pub(crate) fn average_ms(&self) -> u64 {
        if self.durations.is_empty() {
            return 0;
        }
        let count = u64::try_from(self.durations.len()).unwrap_or(u64::MAX);
        self.durations.iter().sum::<u64>() / count.max(1)
    }
}

/// The value at a percentile of a set, zero when it is empty.
///
/// It sorts a copy and not the set: a summary is asked for several percentiles
/// and a caller that sorted in place would reorder the caller's own data.
#[must_use]
fn percentile(values: &[u64], which: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = (sorted.len() * which / 100).min(sorted.len() - 1);
    sorted[index]
}

/// One point of a series over time.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Point {
    /// The bucket's name, as `bucket_label` writes it.
    pub(crate) label: String,
    /// Prompt and answer tokens in the bucket.
    pub(crate) tokens: u64,
    /// The money the provider reported in the bucket.
    pub(crate) cost_usd: f64,
}

/// A model's line in the time chart: its own points, not merged with another
/// provider's model of the same name.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Series {
    /// The `provider/model` the line is for.
    pub(crate) label: String,
    /// The points, oldest bucket first.
    pub(crate) points: Vec<u64>,
}

/// Everything the panel draws, rolled up once from the rows.
#[derive(Clone, Debug, Default)]
pub(crate) struct Report {
    /// The whole set, the total line.
    pub(crate) total: Slice,
    /// Per provider.
    pub(crate) providers: BTreeMap<String, Slice>,
    /// Per `provider/model`.
    pub(crate) models: BTreeMap<String, Slice>,
    /// Per bucket of the range's own granularity.
    pub(crate) buckets: BTreeMap<String, Slice>,
}

impl Report {
    /// Rolls every record into the report.
    #[must_use]
    pub(crate) fn of(records: &[RequestRecord]) -> Self {
        let now = Utc::now();
        let mut report = Self::default();
        for record in records {
            report.total.add_within(record, now);
            report
                .providers
                .entry(record.provider.clone())
                .or_default()
                .add_within(record, now);
            report
                .models
                .entry(format!("{}/{}", record.provider, record.model))
                .or_default()
                .add_within(record, now);
            report
                .buckets
                .entry(bucket_of(record.started_at))
                .or_default()
                .add_within(record, now);
        }
        report
    }

    /// The providers, most of `metric` first.
    #[must_use]
    pub(crate) fn providers_by(&self, metric: Metric) -> Vec<(&str, &Slice)> {
        rank(&self.providers, metric)
    }

    /// The models, most of `metric` first.
    #[must_use]
    pub(crate) fn models_by(&self, metric: Metric) -> Vec<(&str, &Slice)> {
        rank(&self.models, metric)
    }

    /// The models of one provider, most of `metric` first.
    #[must_use]
    pub(crate) fn models_of(&self, provider: &str, metric: Metric) -> Vec<(&str, &Slice)> {
        let prefix = format!("{provider}/");
        let mut rows: Vec<(&str, &Slice)> = self
            .models
            .iter()
            .filter(|(label, _)| label.starts_with(&prefix))
            .map(|(label, slice)| (label.as_str(), slice))
            .collect();
        sort_by(&mut rows, metric);
        rows
    }
}

/// Ranks a map's entries, keeping the keys so the caller can show them.
fn rank(map: &BTreeMap<String, Slice>, metric: Metric) -> Vec<(&str, &Slice)> {
    let mut rows: Vec<(&str, &Slice)> = map
        .iter()
        .map(|(label, slice)| (label.as_str(), slice))
        .collect();
    sort_by(&mut rows, metric);
    rows
}

/// Sorts rank rows by a metric, larger first, ties broken by name.
fn sort_by(rows: &mut [(&str, &Slice)], metric: Metric) {
    rows.sort_by(|left, right| {
        metric
            .value(right.1)
            .partial_cmp(&metric.value(left.1))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(right.0))
    });
}

/// The bucket a record falls in, by the day.
///
/// The panel's series is per day: a provider's use over a year is read by day,
/// and an hour bucket would be hundreds of points that say nothing.
#[must_use]
fn bucket_of(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(provider: &str, model: &str, status: &str) -> RequestRecord {
        RequestRecord {
            id: format!("{provider}-{model}-{status}"),
            session: "s1".to_owned(),
            north_session: "n1".to_owned(),
            turn: 1,
            provider: provider.to_owned(),
            model: model.to_owned(),
            endpoint: None,
            started_at: Utc::now(),
            duration_ms: 1_000,
            first_token_ms: Some(200),
            last_token_ms: Some(900),
            input_tokens: Some(100),
            output_tokens: Some(50),
            reasoning_tokens: Some(10),
            cache_read_tokens: Some(40),
            cache_write_tokens: Some(0),
            estimated_input_tokens: None,
            cache_hit: true,
            tool_calls: vec!["read".to_owned()],
            reasoning: None,
            sandbox: false,
            sandbox_network: false,
            params: json!({}),
            extensions: json!({}),
            metadata: json!({}),
            cost_usd: Some(0.01),
            finish_reason: None,
            status: status.to_owned(),
            error: None,
            error_kind: None,
            retry_after_ms: None,
        }
    }

    #[test]
    fn two_providers_with_the_same_model_name_are_two_rows() {
        // The whole point of keying a model by `provider/model`: the same model
        // name served by two providers bills and caches differently, and merging
        // them would average away the comparison.
        let records = vec![
            record("openai", "gpt-5", "ok"),
            record("openrouter", "gpt-5", "ok"),
        ];
        let report = Report::of(&records);
        assert_eq!(report.models.len(), 2, "{:?}", report.models.keys());
        assert!(report.models.contains_key("openai/gpt-5"));
        assert!(report.models.contains_key("openrouter/gpt-5"));
    }

    #[test]
    fn a_provider_expands_into_its_own_models_only() {
        let records = vec![
            record("openai", "gpt-5", "ok"),
            record("openai", "gpt-4", "ok"),
            record("openrouter", "gpt-5", "ok"),
        ];
        let report = Report::of(&records);
        let models = report.models_of("openai", Metric::Tokens);
        assert_eq!(models.len(), 2, "only openai's own: {models:?}");
        assert!(models.iter().all(|(label, _)| label.starts_with("openai/")));
    }

    #[test]
    fn errors_are_subdivided_by_their_typed_kind() {
        // A bar that says "three errors" is not enough: which three decides what
        // a person does about it, and the kind is in the row.
        let mut rate_limited = record("openai", "gpt-5", "error");
        rate_limited.error_kind = Some("rate_limited".to_owned());
        let mut other = record("openai", "gpt-5", "error");
        other.error_kind = None;
        let records = vec![rate_limited, other];
        let report = Report::of(&records);
        let slice = report.providers.get("openai").expect("provider");
        assert_eq!(slice.errors, 2);
        let kinds = slice.error_tally.ranked();
        assert_eq!(kinds, [("other", 1), ("rate_limited", 1)]);
    }

    #[test]
    fn the_rank_changes_with_the_metric() {
        // Same rows, two questions: the busiest provider and the one that costs
        // most, and the answer must be the row that is first for each.
        let mut cheap = record("cheap", "m", "ok");
        cheap.input_tokens = Some(0);
        cheap.output_tokens = Some(10);
        cheap.cost_usd = Some(0.001);
        let mut busy = record("busy", "m", "ok");
        busy.input_tokens = Some(1_000);
        busy.output_tokens = Some(1_000);
        busy.cost_usd = Some(0.5);
        let records = vec![cheap, busy];
        let report = Report::of(&records);
        // By cost, the expensive one leads.
        let by_cost = report.providers_by(Metric::Cost);
        assert_eq!(by_cost[0].0, "busy");
        // By output tokens alone, only the answer counts: still busy here, so use
        // a case where they differ.
        let mut talker = record("talker", "m", "ok");
        talker.input_tokens = Some(10);
        talker.output_tokens = Some(5_000);
        talker.cost_usd = Some(0.2);
        let report = Report::of(&[record("reader", "m", "ok"), talker]);
        let by_output = report.providers_by(Metric::Output);
        assert_eq!(by_output[0].0, "talker", "the one that answered most");
        let by_tokens = report.providers_by(Metric::Tokens);
        assert_eq!(by_tokens[0].0, "talker");
    }

    #[test]
    fn the_range_holds_what_falls_inside_it() {
        let now = Utc::now();
        let mut old = record("openai", "gpt-5", "ok");
        old.started_at = now - Duration::days(40);
        let fresh = record("openai", "gpt-5", "ok");
        assert!(!Range::Today.holds(&old, now));
        assert!(Range::Today.holds(&fresh, now));
        // A month reaches back thirty days, so the forty-day-old row is out.
        assert!(!Range::Month.holds(&old, now));
        // Everything holds everything.
        assert!(Range::All.holds(&old, now));
    }

    #[test]
    fn the_metric_and_range_walk_in_a_cycle() {
        assert_eq!(Range::All.next(), Range::Today, "it wraps");
        assert_eq!(Metric::Errors.next(), Metric::Tokens, "it wraps");
        assert_eq!(Range::ALL.len(), 5);
        assert_eq!(Metric::ALL.len(), 7);
    }
}
