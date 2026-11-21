//! Bucketing telemetry records into chart series.

use norte_code_wire::{TelemetryRecord, TelemetryReport};

use super::{ChartInterval, MAX_CHART_BUCKETS, series};

fn record(stamp: &str, cost: f64, input: u64, output: u64, read: u64) -> TelemetryRecord {
    TelemetryRecord {
        started_at: stamp.to_owned(),
        cost_usd: Some(cost),
        input_tokens: Some(input),
        output_tokens: Some(output),
        cache_read_tokens: Some(read),
        ..TelemetryRecord::default()
    }
}

fn report(records: Vec<TelemetryRecord>) -> TelemetryReport {
    TelemetryReport {
        records,
        ..TelemetryReport::default()
    }
}

#[test]
fn records_bucket_by_minute_in_order() {
    let data = report(vec![
        record("2026-11-19T10:00:01Z", 1.0, 10, 5, 0),
        record("2026-11-19T10:01:01Z", 2.0, 20, 5, 0),
        record("2026-11-19T10:00:30Z", 3.0, 30, 5, 0),
    ]);
    let result = series(Some(&data), ChartInterval::Minute);
    assert_eq!(result.buckets.len(), 2);
    assert_eq!(result.buckets[0].0, "2026-11-19T10:00");
    assert_eq!(result.buckets[0].1.requests, 2);
    assert_eq!(result.buckets[1].0, "2026-11-19T10:01");
}

#[test]
fn cost_candles_track_cumulative_spend() {
    let data = report(vec![
        record("2026-11-19T10:00:01Z", 1.0, 0, 0, 0),
        record("2026-11-19T10:01:01Z", 2.5, 0, 0, 0),
    ]);
    let result = series(Some(&data), ChartInterval::Minute);
    assert_eq!(result.cost.len(), 2);
    assert_eq!((result.cost[0].open, result.cost[0].close), (0.0, 1.0));
    assert_eq!((result.cost[1].open, result.cost[1].close), (1.0, 3.5));
    assert_eq!(result.cost[1].volume, 1.0);
}

#[test]
fn tokens_sum_and_cache_rate_is_a_percentage() {
    let data = report(vec![
        record("2026-11-19T10:00:01Z", 0.0, 100, 20, 50),
        record("2026-11-19T10:01:01Z", 0.0, 100, 20, 0),
    ]);
    let result = series(Some(&data), ChartInterval::Minute);
    assert_eq!(result.tokens, vec![120.0, 120.0]);
    assert_eq!(result.cache[0], 50.0);
    assert_eq!(result.cache[1], 0.0);
}

#[test]
fn an_unparseable_stamp_is_skipped() {
    let data = report(vec![record("short", 1.0, 1, 1, 0)]);
    let result = series(Some(&data), ChartInterval::Minute);
    assert_eq!(result.buckets.len(), 0);
    assert_eq!(result.cost.len(), 0);
}

#[test]
fn the_series_keeps_only_the_buckets_the_chart_can_draw() {
    let records = (0..MAX_CHART_BUCKETS + 10)
        .map(|minute| {
            record(
                &format!("2026-11-19T{:02}:{:02}:01Z", minute / 60, minute % 60),
                1.0,
                1,
                1,
                0,
            )
        })
        .collect();
    let result = series(Some(&report(records)), ChartInterval::Minute);

    assert_eq!(result.buckets.len(), MAX_CHART_BUCKETS);
    assert_eq!(result.cost.len(), MAX_CHART_BUCKETS);
    assert_eq!(result.buckets[0].0, "2026-11-19T00:10");
}

#[test]
fn hour_and_day_group_finer_stamps() {
    let data = report(vec![
        record("2026-11-19T10:00:01Z", 1.0, 1, 1, 0),
        record("2026-11-19T10:30:01Z", 1.0, 1, 1, 0),
        record("2026-11-19T11:00:01Z", 1.0, 1, 1, 0),
    ]);
    assert_eq!(series(Some(&data), ChartInterval::Hour).buckets.len(), 2);
    assert_eq!(series(Some(&data), ChartInterval::Day).buckets.len(), 1);
}
