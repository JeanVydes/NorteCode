use super::*;
use serde_json::json;

fn record(provider: &str, model: &str, status: RequestStatus) -> RequestRecord {
    RequestRecord {
        id: format!("{provider}-{model}-{}", status.as_str()),
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
        status,
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
        record("openai", "gpt-5", RequestStatus::Ok),
        record("openrouter", "gpt-5", RequestStatus::Ok),
    ];
    let report = Report::of(&records, Range::All);
    assert_eq!(report.models.len(), 2, "{:?}", report.models.keys());
    assert!(report.models.contains_key("openai/gpt-5"));
    assert!(report.models.contains_key("openrouter/gpt-5"));
}

#[test]
fn a_provider_expands_into_its_own_models_only() {
    let records = vec![
        record("openai", "gpt-5", RequestStatus::Ok),
        record("openai", "gpt-4", RequestStatus::Ok),
        record("openrouter", "gpt-5", RequestStatus::Ok),
    ];
    let report = Report::of(&records, Range::All);
    let models = report.models_of("openai", Metric::Tokens);
    assert_eq!(models.len(), 2, "only openai's own: {models:?}");
    assert!(models.iter().all(|(label, _)| label.starts_with("openai/")));
}

#[test]
fn errors_are_subdivided_by_their_typed_kind() {
    // A bar that says "three errors" is not enough: which three decides what
    // a person does about it, and the kind is in the row.
    let mut rate_limited = record("openai", "gpt-5", RequestStatus::Error);
    rate_limited.error_kind = Some("rate_limited".to_owned());
    let mut other = record("openai", "gpt-5", RequestStatus::Error);
    other.error_kind = None;
    let records = vec![rate_limited, other];
    let report = Report::of(&records, Range::All);
    let slice = report.providers.get("openai").expect("provider");
    assert_eq!(slice.errors, 2);
    let kinds = slice.error_tally.ranked();
    assert_eq!(kinds, [("other", 1), ("rate_limited", 1)]);
}

#[test]
fn the_rank_changes_with_the_metric() {
    // Same rows, two questions: the busiest provider and the one that costs
    // most, and the answer must be the row that is first for each.
    let mut cheap = record("cheap", "m", RequestStatus::Ok);
    cheap.input_tokens = Some(0);
    cheap.output_tokens = Some(10);
    cheap.cost_usd = Some(0.001);
    let mut busy = record("busy", "m", RequestStatus::Ok);
    busy.input_tokens = Some(1_000);
    busy.output_tokens = Some(1_000);
    busy.cost_usd = Some(0.5);
    let records = vec![cheap, busy];
    let report = Report::of(&records, Range::All);
    // By cost, the expensive one leads.
    let by_cost = report.providers_by(Metric::Cost);
    assert_eq!(by_cost[0].0, "busy");
    // By output tokens alone, only the answer counts: still busy here, so use
    // a case where they differ.
    let mut talker = record("talker", "m", RequestStatus::Ok);
    talker.input_tokens = Some(10);
    talker.output_tokens = Some(5_000);
    talker.cost_usd = Some(0.2);
    let report = Report::of(
        &[record("reader", "m", RequestStatus::Ok), talker],
        Range::All,
    );
    let by_output = report.providers_by(Metric::Output);
    assert_eq!(by_output[0].0, "talker", "the one that answered most");
    let by_tokens = report.providers_by(Metric::Tokens);
    assert_eq!(by_tokens[0].0, "talker");
}

#[test]
fn the_range_holds_what_falls_inside_it() {
    let now = Utc::now();
    let mut old = record("openai", "gpt-5", RequestStatus::Ok);
    old.started_at = now - Duration::days(40);
    let fresh = record("openai", "gpt-5", RequestStatus::Ok);
    assert!(!Range::Today.holds(&old, now));
    assert!(Range::Today.holds(&fresh, now));
    // A month reaches back thirty days, so the forty-day-old row is out.
    assert!(!Range::Month.holds(&old, now));
    // Everything holds everything.
    assert!(Range::All.holds(&old, now));
}

fn text_of(panel: &Panel) -> String {
    panel
        .lines()
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_panel_draws_the_tabs_the_range_and_the_rank() {
    let records = vec![
        record("openai", "gpt-5", RequestStatus::Ok),
        record("openrouter", "gpt-5", RequestStatus::Ok),
    ];
    let panel = Panel::new(records, None);
    let text = text_of(&panel);
    assert!(text.contains("this session"), "{text}");
    assert!(
        text.contains("[providers]") || text.contains("providers"),
        "{text}"
    );
    assert!(text.contains("range: Today"), "{text}");
    assert!(text.contains("rank by: total tokens"), "{text}");
}

#[test]
fn this_session_is_the_same_roll_up_filtered_by_session() {
    // The two tabs are one roll-up over two filters: this session is the
    // session's own slice of exactly what the provider tab rolls up, so a
    // row of another session is not in it.
    let mut mine = record("openai", "gpt-5", RequestStatus::Ok);
    mine.session = "mine".to_owned();
    let mut other = record("openai", "gpt-5", RequestStatus::Ok);
    other.session = "other".to_owned();
    other.id = "other".to_owned();
    let panel = Panel::new(vec![mine, other], Some("mine".to_owned()));
    let text = text_of(&panel);
    // One request, not two: the other session's is not this session's.
    assert!(text.contains("requests      1"), "{text}");
}

#[test]
fn a_provider_expands_in_the_panel_to_show_its_models() {
    let records = vec![
        record("openai", "gpt-5", RequestStatus::Ok),
        record("openai", "gpt-4", RequestStatus::Ok),
    ];
    let mut panel = Panel::new(records, None);
    panel.next_tab();
    let closed = text_of(&panel);
    assert!(
        !closed.contains("gpt-4"),
        "closed shows no models: {closed}"
    );
    panel.toggle_at_cursor();
    let open = text_of(&panel);
    assert!(open.contains("gpt-4"), "open shows them: {open}");
    assert!(open.contains("gpt-5"), "{open}");
}

#[test]
fn the_error_section_subdivides_a_bar_by_kind() {
    let mut rate = record("openai", "gpt-5", RequestStatus::Error);
    rate.error_kind = Some("rate_limited".to_owned());
    let mut timeout = record("openai", "gpt-5", RequestStatus::Error);
    timeout.id = "timeout".to_owned();
    timeout.error_kind = Some("timeout".to_owned());
    let panel = Panel::new(vec![rate, timeout], None);
    let text = text_of(&panel);
    assert!(text.contains("errors by provider, by kind"), "{text}");
    assert!(text.contains("rate_limited"), "{text}");
    assert!(text.contains("timeout"), "{text}");
}

#[test]
fn the_time_tab_draws_a_compact_sparkline_per_model() {
    // Two models over two days: each gets one compact line, not a tall
    // multi-row plot that pushes the useful content out of the panel.
    let today = Utc::now();
    let yesterday = today - Duration::days(1);
    let mut a1 = record("openai", "gpt-5", RequestStatus::Ok);
    a1.started_at = yesterday;
    a1.id = "a1".to_owned();
    a1.input_tokens = Some(100);
    let mut a2 = record("openai", "gpt-5", RequestStatus::Ok);
    a2.started_at = today;
    a2.id = "a2".to_owned();
    a2.input_tokens = Some(1_000);
    let mut b = record("openrouter", "gpt-5", RequestStatus::Ok);
    b.started_at = today;
    let mut panel = Panel::new(vec![a1, a2, b], None);
    panel.tab = Tab::Time;
    panel.range = Range::Week;
    let text = text_of(&panel);
    assert!(text.contains("over time"), "{text}");
    assert!(text.contains("daily"), "{text}");
    assert!(text.contains("openai/gpt-5"), "{text}");
    assert!(text.contains("openrouter/gpt-5"), "{text}");
    assert!(text.contains("bucket(s)"), "{text}");
    assert!(text.contains('█'), "{text}");
    assert!(
        text.lines().count() < 8,
        "each model should occupy a single row:\n{text}"
    );
}

#[test]
fn time_series_fills_missing_buckets_between_requests() {
    let now = Utc::now();
    let mut first = record("openai", "gpt-5", RequestStatus::Ok);
    first.started_at = now - Duration::days(3);
    let mut last = record("openai", "gpt-5", RequestStatus::Ok);
    last.started_at = now;
    let mut panel = Panel::new(vec![first, last], None);
    panel.tab = Tab::Time;
    panel.range = Range::Week;

    let text = text_of(&panel);
    assert!(text.contains("4 bucket(s)"), "{text}");
    let trend = text
        .lines()
        .find(|line| line.contains("openai/gpt-5"))
        .expect("model trend");
    assert_eq!(
        trend
            .chars()
            .filter(|character| "▁▂▃▄▅▆▇█·".contains(*character))
            .count(),
        4,
        "the sparkline includes zero-use days: {trend}"
    );
    assert!(
        trend.contains("█··█"),
        "idle days remain visibly empty: {trend}"
    );
}

#[test]
fn a_sparkline_of_one_peak_reads_as_a_block() {
    assert_eq!(sparkline(&[0, 0, 0]), "···");
    let line = sparkline(&[1, 8, 4]);
    let chars: Vec<char> = line.chars().collect();
    assert_eq!(chars.len(), 3);
    assert_eq!(chars[1], '█', "the peak is the full block");
    assert!(chars[0] < chars[1], "the small one is shorter");
}

#[test]
fn a_long_sparkline_is_bounded_and_keeps_its_peak() {
    let points: Vec<u64> = (0..100).collect();
    let line = sparkline(&points);
    assert_eq!(line.chars().count(), MAX_TREND_CELLS);
    assert_eq!(line.chars().last(), Some('█'));
}

#[test]
fn the_panel_animates_in_and_then_settles() {
    // The entry animation is the panel telling a person it just appeared: it
    // runs a few frames and then stops, so the clock that drew it stops too.
    let records = vec![record("openai", "gpt-5", RequestStatus::Ok)];
    let mut panel = Panel::new(records, None);
    assert!(panel.animating(), "it starts animating");
    let first = text_of(&panel);
    for _ in 0..Panel::REVEAL_FRAMES {
        panel.tick();
    }
    assert!(!panel.animating(), "and it settles");
    let last = text_of(&panel);
    assert_ne!(first, last, "the drawn frame changed as it grew");
    // A tick past the end changes nothing.
    let settled = text_of(&panel);
    panel.tick();
    assert_eq!(text_of(&panel), settled);
}

#[test]
fn the_cost_section_shows_what_was_not_reported() {
    let mut priced = record("openai", "gpt-5", RequestStatus::Ok);
    priced.cost_usd = Some(1.0);
    let mut unpriced = record("free", "m", RequestStatus::Ok);
    unpriced.cost_usd = None;
    let mut panel = Panel::new(vec![priced, unpriced], None);
    panel.tab = Tab::Providers;
    for _ in 0..Panel::REVEAL_FRAMES {
        panel.tick();
    }
    let text = text_of(&panel);
    assert!(text.contains("unpriced"), "{text}");
    assert!(text.contains("cost reported"), "{text}");
}

#[test]
fn the_metric_and_range_walk_in_a_cycle() {
    assert_eq!(Range::All.next(), Range::Today, "it wraps");
    assert_eq!(Metric::Errors.next(), Metric::Tokens, "it wraps");
    assert_eq!(Range::ALL.len(), 5);
    assert_eq!(Metric::ALL.len(), 7);
}
