//! Rankings reflect the shared report and the selected tab.

use norte_code_wire::TelemetryRecord;

use super::{Leaderboard, LeaderboardGroup, LeaderboardSubject};

fn record(
    session: &str,
    turn: u64,
    provider: &str,
    model: &str,
    input: Option<u64>,
    output: Option<u64>,
    cost: Option<f64>,
) -> TelemetryRecord {
    TelemetryRecord {
        session: session.to_owned(),
        turn,
        provider: provider.to_owned(),
        model: model.to_owned(),
        input_tokens: input,
        output_tokens: output,
        cost_usd: cost,
        ..TelemetryRecord::default()
    }
}

#[test]
fn model_usage_ranks_by_requests() {
    let records = vec![
        record("a", 1, "p", "small", Some(10), Some(2), Some(0.1)),
        record("a", 2, "p", "large", Some(100), Some(50), Some(1.0)),
        record("b", 1, "p", "small", Some(10), Some(2), Some(0.1)),
    ];

    let board = Leaderboard::build(&records, LeaderboardGroup::Usage, LeaderboardSubject::Model);

    assert_eq!(board.rows[0].name, "small");
    assert_eq!(board.rows[0].requests, 2);
    assert_eq!(board.rows[0].turns, 2);
}

#[test]
fn token_totals_saturate_when_reports_overflow() {
    let records = vec![
        record("a", 1, "p", "m", Some(u64::MAX), Some(u64::MAX), None),
        record("a", 2, "p", "m", Some(u64::MAX), Some(u64::MAX), None),
    ];

    let board = Leaderboard::build(&records, LeaderboardGroup::Usage, LeaderboardSubject::Model);

    assert_eq!(board.rows[0].input_tokens, u64::MAX);
    assert_eq!(board.rows[0].output_tokens, u64::MAX);
}

#[test]
fn session_usage_combines_input_and_output_tokens() {
    let records = vec![
        record("few", 1, "p", "m", Some(10), Some(10), Some(0.1)),
        record("many", 1, "p", "m", Some(100), Some(1), None),
        record("many", 1, "p", "m", Some(100), Some(1), None),
    ];

    let board = Leaderboard::build(
        &records,
        LeaderboardGroup::Usage,
        LeaderboardSubject::Session,
    );

    assert_eq!(board.rows[0].name, "many");
    assert_eq!(board.rows[0].input_tokens, 200);
    assert_eq!(board.rows[0].output_tokens, 2);
    assert_eq!(board.rows[0].turns, 1);
}

#[test]
fn provider_cost_sums_every_model() {
    let records = vec![
        record("a", 1, "cheap", "one", None, None, Some(0.1)),
        record("a", 2, "cheap", "two", None, None, Some(0.2)),
        record("b", 1, "dear", "three", None, None, Some(1.5)),
    ];

    let board = Leaderboard::build(
        &records,
        LeaderboardGroup::Cost,
        LeaderboardSubject::Provider,
    );

    assert_eq!(board.rows[0].name, "dear");
    assert_eq!(board.rows[0].cost_usd, 1.5);
    assert_eq!(board.rows[1].name, "cheap");
    assert!((board.rows[1].cost_usd - 0.3).abs() < 1e-9);
}

#[test]
fn selection_tabs_cycle_within_their_group() {
    assert_eq!(LeaderboardGroup::Usage.label(), "usage");
    assert_eq!(LeaderboardGroup::Cost.label(), "cost");
    assert_eq!(
        LeaderboardSubject::Model.next(),
        LeaderboardSubject::Provider
    );
    assert_eq!(
        LeaderboardSubject::Session.next(),
        LeaderboardSubject::Model
    );
}
