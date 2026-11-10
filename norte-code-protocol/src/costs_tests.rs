use super::*;

fn record(index: u64, input: u64, output: u64, cost: Option<f64>) -> CostRecord {
    CostRecord {
        session: "s".to_owned(),
        turn: 1,
        index,
        model: "p/m".to_owned(),
        input_tokens: Some(input),
        output_tokens: Some(output),
        reasoning_tokens: Some(0),
        cache_read_tokens: None,
        cache_write_tokens: None,
        cost_usd: cost,
        recorded_at: Utc::now(),
    }
}

fn budget(unit: BudgetUnit, limit: f64, action: BudgetAction) -> Budget {
    Budget {
        unit,
        limit,
        action,
    }
}

#[test]
fn a_summary_counts_unreported_requests_beside_the_usd_total() {
    let records = [record(1, 10, 5, Some(0.25)), record(2, 20, 5, None)];
    let summary = summarize(&records);
    assert_eq!(summary.requests, 2);
    assert_eq!(summary.total_tokens(), 40);
    assert!((summary.reported_cost_usd - 0.25).abs() < f64::EPSILON);
    assert_eq!(summary.not_reported, 1);
    assert!(!summary.usd_measurable());
}

#[test]
fn rollups_keep_sessions_turns_and_days_apart() {
    let mut first = record(1, 10, 5, Some(0.1));
    first.recorded_at = "2026-01-02T10:00:00Z".parse().expect("time");
    let mut second = record(2, 20, 5, None);
    second.session = "other".to_owned();
    second.turn = 2;
    second.recorded_at = "2026-01-03T10:00:00Z".parse().expect("time");
    let records = [first, second];
    assert_eq!(by_session(&records).len(), 2);
    assert_eq!(by_turn(&records).len(), 2);
    let days = by_day(&records);
    assert_eq!(days.len(), 2);
    assert_eq!(days["2026-01-02"].requests, 1);
}

#[test]
fn a_usd_budget_is_not_measured_over_unreported_cost() {
    let summary = summarize(&[record(1, 10, 5, None)]);
    assert!(measure(&budget(BudgetUnit::Usd, 1.0, BudgetAction::Stop), &summary).is_none());
}

#[test]
fn warnings_cross_at_eighty_percent_and_exhaustion_at_the_limit() {
    let mut book = BudgetBook::new(
        None,
        Some(budget(BudgetUnit::Tokens, 100.0, BudgetAction::Stop)),
        None,
        CostSummary::default(),
        CostSummary::default(),
        CostSummary::default(),
    );
    book.record(&record(1, 40, 40, None));
    let notices = book.notices();
    assert!(matches!(notices.as_slice(), [BudgetNotice::Warning(_)]));
    assert!(book.stop().is_none());
    book.record(&record(2, 10, 10, None));
    let notices = book.notices();
    assert!(matches!(notices.as_slice(), [BudgetNotice::Exceeded(_)]));
    assert!(book.stop().is_some());
}

#[test]
fn a_mid_turn_exhaustion_is_reported_once_by_the_stopping_layer() {
    let mut book = BudgetBook::new(
        None,
        Some(budget(BudgetUnit::Tokens, 100.0, BudgetAction::Stop)),
        None,
        CostSummary::default(),
        CostSummary::default(),
        CostSummary::default(),
    );
    book.record(&record(1, 40, 40, None));
    assert!(matches!(
        book.warnings().as_slice(),
        [BudgetNotice::Warning(_)]
    ));
    book.record(&record(2, 10, 10, None));
    assert!(book.warnings().is_empty(), "only the first layer warns");
    assert!(book.stop().is_some());
    assert!(book.notices().is_empty(), "a warning is not a notice");
}

#[test]
fn the_final_check_reports_an_exhaustion_the_layer_never_saw() {
    let mut book = BudgetBook::new(
        None,
        Some(budget(BudgetUnit::Tokens, 100.0, BudgetAction::Stop)),
        None,
        CostSummary::default(),
        CostSummary::default(),
        CostSummary::default(),
    );
    book.record(&record(1, 60, 60, None));
    assert!(matches!(
        book.warnings().as_slice(),
        [BudgetNotice::Warning(_)]
    ));
    assert!(matches!(
        book.notices().as_slice(),
        [BudgetNotice::Exceeded(_)]
    ));
    assert!(
        book.notices().is_empty(),
        "the layer reports the exhaustion, the final check does not repeat it"
    );
}

#[test]
fn a_warn_budget_reports_but_does_not_stop() {
    let mut book = BudgetBook::new(
        None,
        Some(budget(BudgetUnit::Requests, 1.0, BudgetAction::Warn)),
        None,
        CostSummary::default(),
        CostSummary::default(),
        CostSummary::default(),
    );
    book.record(&record(1, 1, 1, None));
    book.record(&record(2, 1, 1, None));
    let notices = book.notices();
    assert!(
        notices
            .iter()
            .any(|notice| matches!(notice, BudgetNotice::Exceeded(_)))
    );
    assert!(book.stop().is_none());
}

#[test]
fn raising_the_session_budget_clears_its_stop() {
    let mut book = BudgetBook::new(
        None,
        Some(budget(BudgetUnit::Requests, 1.0, BudgetAction::Stop)),
        None,
        CostSummary::default(),
        CostSummary::default(),
        CostSummary::default(),
    );
    book.record(&record(1, 1, 1, None));
    assert!(book.stop().is_some());
    book.set_session_budget(Some(budget(BudgetUnit::Requests, 5.0, BudgetAction::Stop)));
    assert!(book.stop().is_none());
}
