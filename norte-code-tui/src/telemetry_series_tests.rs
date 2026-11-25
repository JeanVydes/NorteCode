//! Bucketing telemetry records into chart series.

use norte_code_wire::{TelemetryRecord, TelemetryReport};

use super::{ChartInterval, MAX_CHART_BUCKETS, chart_window, series};

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
fn token_directions_and_cache_rate_are_separate_series() {
    let data = report(vec![
        record("2026-11-19T10:00:01Z", 0.0, 100, 20, 50),
        record("2026-11-19T10:01:01Z", 0.0, 100, 20, 0),
    ]);
    let result = series(Some(&data), ChartInterval::Minute);
    assert_eq!(result.input, vec![100.0, 100.0]);
    assert_eq!(result.output, vec![20.0, 20.0]);
    assert_eq!(result.cache[0], Some(50.0));
    assert_eq!(result.cache[1], Some(0.0));
}

#[test]
fn charts_skip_reconstructed_history_with_unknown_timestamps() {
    let mut measured = record("2026-11-19T10:00:01Z", 0.0, 10, 2, 0);
    measured.duration_ms = 300;
    measured.first_token_ms = Some(80);
    measured.first_text_ms = Some(120);
    let mut historical = record("2026-11-19T10:01:01Z", 0.0, 10, 2, 0);
    historical.duration_ms = 0;
    historical.metadata = serde_json::json!({"source": "history"});
    let result = series(
        Some(&report(vec![measured, historical])),
        ChartInterval::Minute,
    );

    assert_eq!(result.buckets.len(), 1);
    assert_eq!(result.input, vec![10.0]);
    assert_eq!(result.requests, vec![1.0]);
    assert_eq!(result.latency, vec![Some(300.0)]);
    assert_eq!(result.first_output, vec![Some(80.0)]);
    assert_eq!(result.first_text, vec![Some(120.0)]);
}

#[test]
fn an_unparseable_stamp_is_skipped() {
    let data = report(vec![record("short", 1.0, 1, 1, 0)]);
    let result = series(Some(&data), ChartInterval::Minute);
    assert_eq!(result.buckets.len(), 0);
    assert_eq!(result.cost.len(), 0);
}

#[test]
fn a_stamp_cut_inside_a_character_is_skipped() {
    let data = report(vec![record("2026-11-19T10:0\u{e9}Z", 1.0, 1, 1, 0)]);

    let result = series(Some(&data), ChartInterval::Minute);

    assert_eq!(result.buckets, Vec::new());
}

#[test]
fn the_series_downsamples_the_full_history_to_the_chart_limit() {
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
    let requests: u64 = result
        .buckets
        .iter()
        .map(|(_, bucket)| bucket.requests)
        .sum();

    assert_eq!(result.buckets.len(), MAX_CHART_BUCKETS);
    assert_eq!(result.cost.len(), MAX_CHART_BUCKETS);
    assert_eq!(result.buckets[0].0, "2026-11-19T00:00");
    assert_eq!(result.buckets.last().unwrap().0, "2026-11-19T04:09");
    assert_eq!(
        usize::try_from(requests).expect("request count fits"),
        MAX_CHART_BUCKETS + 10
    );
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

#[test]
fn chart_window_clamps_and_scrolls() {
    assert_eq!(chart_window(0, 1.0, 40, 0), (0, 0));
    let (start, end) = chart_window(10, 1.0, 40, 0);
    assert!(end >= start);
    assert!(end <= 10);
    let (start2, end2) = chart_window(10, 1.0, 40, 3);
    assert!(end2 <= 7);
    assert!(start2 <= end2);
}
