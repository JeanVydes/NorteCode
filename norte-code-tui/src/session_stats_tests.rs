//! Rolling telemetry records up into session usage.

use norte_code_wire::{RequestStatus, TelemetryRecord};

use super::{SessionUsage, by_session};

fn record(session: &str, model: &str, turn: u64, input: u64, read: u64) -> TelemetryRecord {
    TelemetryRecord {
        session: session.to_owned(),
        model: model.to_owned(),
        turn,
        input_tokens: Some(input),
        cache_read_tokens: Some(read),
        ..TelemetryRecord::default()
    }
}

#[test]
fn only_the_sessions_records_are_counted() {
    let records = vec![
        record("a", "m1", 1, 100, 50),
        record("b", "m1", 1, 100, 50),
        record("a", "m2", 2, 200, 0),
    ];
    let usage = by_session(&records).remove("a").expect("session a");
    assert_eq!(usage.requests, 2);
    assert_eq!(usage.input, 300);
    assert_eq!(usage.turns.len(), 2);
}

#[test]
fn counters_saturate_when_reports_overflow() {
    let records = vec![
        record("a", "m", 1, u64::MAX, u64::MAX),
        record("a", "m", 2, u64::MAX, u64::MAX),
    ];

    let usage = by_session(&records).remove("a").expect("session a");

    assert_eq!(usage.input, u64::MAX);
    assert_eq!(usage.cache_read, u64::MAX);
}

#[test]
fn cache_rate_is_the_share_of_prompt_served_from_cache() {
    let usage = SessionUsage {
        input: 200,
        cache_read: 50,
        ..SessionUsage::default()
    };
    assert!((usage.cache_rate() - 25.0).abs() < f64::EPSILON);
    assert_eq!(SessionUsage::default().cache_rate(), 0.0);
}

#[test]
fn models_are_ranked_by_use() {
    let records = vec![
        record("a", "small", 1, 10, 0),
        record("a", "big", 2, 10, 0),
        record("a", "big", 3, 10, 0),
    ];
    let usage = by_session(&records).remove("a").expect("session a");
    assert_eq!(usage.top_models(2), vec![("big", 2), ("small", 1)]);
}

#[test]
fn tools_errors_and_cost_are_counted() {
    let mut failed = record("a", "m", 1, 10, 0);
    failed.tool_calls = vec!["read".to_owned(), "shell".to_owned()];
    failed.status = RequestStatus::Error;
    failed.cost_usd = Some(0.25);
    let usage = by_session(&[failed]).remove("a").expect("session a");
    assert_eq!(usage.tools, 2);
    assert_eq!(usage.errors, 1);
    assert_eq!(usage.cost, 0.25);
    assert!(usage.has_cost);
}
