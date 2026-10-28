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

#![allow(clippy::cast_precision_loss)] // a token count is far under 2^52 in any real ledger.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use norte_code_store_sqlite::{RequestRecord, RequestStatus};

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

    /// The range before it, wrapping.
    #[must_use]
    pub(crate) fn prev(self) -> Self {
        let at = Self::ALL
            .iter()
            .position(|range| *range == self)
            .unwrap_or(0);
        Self::ALL[(at + Self::ALL.len() - 1) % Self::ALL.len()]
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

    /// The metric before it, wrapping.
    #[must_use]
    pub(crate) fn prev(self) -> Self {
        let at = Self::ALL
            .iter()
            .position(|metric| *metric == self)
            .unwrap_or(0);
        Self::ALL[(at + Self::ALL.len() - 1) % Self::ALL.len()]
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
        // Exhaustive over the enum and with no catch-all: a status the store
        // grows is a compile error here rather than a request silently counted
        // as an error, which is what the string's `_` arm did.
        match record.status {
            RequestStatus::Ok => {}
            RequestStatus::Cancelled => self.cancelled += 1,
            RequestStatus::Error => {
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
    pub(crate) fn of(records: &[RequestRecord], range: Range) -> Self {
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
                .entry(bucket_of(record.started_at, range))
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

/// The bucket a record falls in: the hour for a day's range, the day otherwise.
///
/// A day read by day is one point, which is no line; the same day read by hour is
/// the shape of the working day. Everything longer is read by day, because an
/// hour bucket over a year is thousands of points that say nothing.
#[must_use]
fn bucket_of(at: DateTime<Utc>, range: Range) -> String {
    match range {
        Range::Today => at.format("%Y-%m-%d %H:00").to_string(),
        _ => at.format("%Y-%m-%d").to_string(),
    }
}

/// The bars a rank is drawn as: `filled` cells of a full **█** at most.
///
/// It is text and not a widget, so a bar survives a pipe and reads the same in
/// the panel as in `--jsonl`'s neighbour. The width is fixed, so a row of bars
/// lines up down the panel and the eye can compare them.
const BAR_CELLS: usize = 24;

/// The label column, so the bars and the numbers line up down the panel.
const LABEL_COLUMN: usize = 22;

/// One drawn line: text plus the ink it is written in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ViewLine {
    /// What the line says.
    pub(crate) text: String,
    /// Phrase the ink: `None` is ordinary, `Some` is a muted note.
    pub(crate) dim: bool,
    /// Whether the line is a heading, drawn bold.
    pub(crate) bold: bool,
}

impl ViewLine {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            dim: false,
            bold: false,
        }
    }

    fn dim(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            dim: true,
            bold: false,
        }
    }

    fn bold(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            dim: false,
            bold: true,
        }
    }
}

/// The tabs the panel switches between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    /// The session the client is in.
    ThisSession,
    /// Every provider, with the models each served.
    Providers,
    /// How each model's use moved over time.
    Time,
}

impl Tab {
    /// Every tab, in the order the key walks them.
    pub(crate) const ALL: [Self; 3] = [Self::ThisSession, Self::Providers, Self::Time];

    /// The word the tab row shows.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ThisSession => "this session",
            Self::Providers => "providers",
            Self::Time => "over time",
        }
    }

    /// The next tab, wrapping.
    #[must_use]
    pub(crate) fn next(self) -> Self {
        let at = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

/// The whole panel state: the rows, and what is shown of them.
///
/// It holds the rows and not a rolled-up report: the tab and the range decide
/// what is rolled up, and `this session` and `providers` are the same roll-up
/// over two different filters. A report built once at construction would answer
/// the wrong question the moment the range moved.
#[derive(Clone, Debug)]
pub(crate) struct Panel {
    /// Every recorded row, as read from the store.
    records: Vec<RequestRecord>,
    /// The session key the `this session` tab filters by, when one is open.
    session: Option<String>,
    /// When "now" is, so the range does not move under a frame.
    now: DateTime<Utc>,
    /// Which tab is shown.
    pub(crate) tab: Tab,
    /// The window the numbers cover.
    pub(crate) range: Range,
    /// What the ranks are ordered by.
    pub(crate) metric: Metric,
    /// The providers whose models are shown, by provider id.
    open: BTreeSet<String>,
    /// Which provider row the cursor is on, in the `providers` tab.
    pub(crate) cursor: usize,
    /// How many frames have been drawn since the panel opened.
    ///
    /// It is what an entry animation counts on: the bars grow from nothing to
    /// their value over the first few frames, which is what tells a person the
    /// panel just appeared and is not a screen that was always there. Past the
    /// last frame the panel stops animating and the clock stops drawing it.
    pub(crate) frame: usize,
}

impl Panel {
    /// A panel over every recorded row, with a session to answer for.
    #[must_use]
    pub(crate) fn new(records: Vec<RequestRecord>, session: Option<String>) -> Self {
        Self {
            records,
            session,
            now: Utc::now(),
            tab: Tab::ThisSession,
            range: Range::Today,
            metric: Metric::Tokens,
            open: BTreeSet::new(),
            cursor: 0,
            frame: 0,
        }
    }

    /// How many frames the entry animation runs for.
    const REVEAL_FRAMES: usize = 8;

    /// Whether the panel is still animating, so the clock keeps drawing it.
    #[must_use]
    pub(crate) fn animating(&self) -> bool {
        self.frame < Self::REVEAL_FRAMES
    }

    /// Advances the animation one frame.
    pub(crate) fn tick(&mut self) {
        if self.frame < Self::REVEAL_FRAMES {
            self.frame += 1;
        }
    }

    /// The share of a bar the reveal has grown to, in cells.
    ///
    /// It is the animation: a bar is drawn at `full * reveal / frames`, so the
    /// first frame shows nothing and the last shows the bar. A percentage and not
    /// a per-bar counter, so every bar grows together and the chart assembles
    /// itself instead of each bar racing the others.
    fn reveal(&self, full: usize) -> usize {
        if self.frame >= Self::REVEAL_FRAMES {
            return full;
        }
        full * self.frame / Self::REVEAL_FRAMES
    }

    /// The rows the current tab and range cover.
    ///
    /// `this session` is the same roll-up as `providers`, filtered to the open
    /// session: the numbers a person sees there are the session's own slice of
    /// exactly what the other tab rolls up.
    fn scoped(&self) -> Vec<&RequestRecord> {
        self.records
            .iter()
            .filter(|record| self.range.holds(record, self.now))
            .filter(|record| {
                self.tab != Tab::ThisSession
                    || self
                        .session
                        .as_deref()
                        .is_none_or(|session| record.session == session)
            })
            .collect()
    }

    /// The roll-up the current tab and range describe.
    #[must_use]
    fn report(&self) -> Report {
        let rows: Vec<RequestRecord> = self.scoped().into_iter().cloned().collect();
        Report::of(&rows, self.range)
    }

    /// Switches to the next tab.
    pub(crate) fn next_tab(&mut self) {
        self.tab = self.tab.next();
        self.cursor = 0;
    }

    /// Walks the range selector on.
    pub(crate) fn next_range(&mut self) {
        self.range = self.range.next();
    }

    /// Walks the range selector back.
    pub(crate) fn prev_range(&mut self) {
        self.range = self.range.prev();
    }

    /// Walks the metric the ranks are ordered by.
    pub(crate) fn next_metric(&mut self) {
        self.metric = self.metric.next();
    }

    /// Walks the metric back.
    pub(crate) fn prev_metric(&mut self) {
        self.metric = self.metric.prev();
    }

    /// Moves the provider cursor, holding at both ends.
    pub(crate) fn move_cursor(&mut self, delta: isize) {
        let rows = self.report().providers_by(self.metric).len();
        if rows == 0 {
            self.cursor = 0;
            return;
        }
        let at = self.cursor.saturating_add_signed(delta);
        self.cursor = at.min(rows - 1);
    }

    /// Opens or closes the provider under the cursor.
    pub(crate) fn toggle_at_cursor(&mut self) {
        let name = self
            .report()
            .providers_by(self.metric)
            .get(self.cursor)
            .map(|(provider, _)| (*provider).to_owned());
        if let Some(provider) = name {
            if !self.open.remove(&provider) {
                self.open.insert(provider);
            }
        }
    }

    /// The tab row, so a person sees which of them they are on.
    #[must_use]
    pub(crate) fn tab_row(&self) -> ViewLine {
        let tabs: Vec<String> = Tab::ALL
            .iter()
            .map(|tab| {
                if *tab == self.tab {
                    format!("[{}]", tab.label())
                } else {
                    format!(" {} ", tab.label())
                }
            })
            .collect();
        ViewLine::bold(format!(
            "{}   ·   range: {}   ·   rank by: {}",
            tabs.join(" "),
            self.range.label(),
            self.metric.label()
        ))
    }

    /// The lines of the current tab.
    #[must_use]
    pub(crate) fn lines(&self) -> Vec<ViewLine> {
        let report = self.report();
        let mut out = vec![self.tab_row(), ViewLine::plain("")];
        match self.tab {
            Tab::ThisSession => self.session_lines(&mut out, &report),
            Tab::Providers => self.provider_lines(&mut out, &report),
            Tab::Time => self.time_lines(&mut out),
        }
        out
    }

    /// This session's own figures, its own models ranked, and its errors.
    fn session_lines(&self, out: &mut Vec<ViewLine>, report: &Report) {
        if report.total.requests == 0 {
            out.push(ViewLine::dim(
                "no request recorded for this session, in this range",
            ));
            return;
        }
        out.push(ViewLine::bold("summary"));
        for (label, value) in summary_facts(&report.total) {
            out.push(ViewLine::plain(format!("  {label:<14}{value}")));
        }
        out.push(ViewLine::plain(""));
        out.push(ViewLine::bold(format!("models by {}", self.metric.label())));
        let rows = report.models_by(self.metric);
        let busiest = busiest_of(&rows, self.metric);
        for (model, slice) in &rows {
            out.push(ViewLine::plain(rank_row(
                model,
                slice,
                self.metric,
                busiest,
                self.reveal(BAR_CELLS),
            )));
        }
        error_section(out, report, self.reveal(BAR_CELLS));
    }

    /// Every provider, each expandable into its models.
    fn provider_lines(&self, out: &mut Vec<ViewLine>, report: &Report) {
        let rows = report.providers_by(self.metric);
        if rows.is_empty() {
            out.push(ViewLine::dim("nothing recorded"));
            return;
        }
        let busiest = busiest_of(&rows, self.metric);
        out.push(ViewLine::bold(format!(
            "providers by {}",
            self.metric.label()
        )));
        for (index, (provider, slice)) in rows.iter().enumerate() {
            let open = self.open.contains(*provider);
            let marker = if open { "▾" } else { "▸" };
            let lead = if index == self.cursor { "›" } else { " " };
            out.push(ViewLine::plain(format!(
                "{lead}{marker} {}",
                rank_row(
                    provider,
                    slice,
                    self.metric,
                    busiest,
                    self.reveal(BAR_CELLS)
                )
            )));
            if open {
                for (model, model_slice) in report.models_of(provider, self.metric) {
                    let name = model.split_once('/').map_or(model, |(_, name)| name);
                    out.push(ViewLine::plain(format!(
                        "     {}",
                        rank_row(
                            name,
                            model_slice,
                            self.metric,
                            busiest,
                            self.reveal(BAR_CELLS)
                        )
                    )));
                }
            }
        }
        cost_section(out, report, self.reveal(BAR_CELLS));
        error_section(out, report, self.reveal(BAR_CELLS));
    }

    /// One sparkline per model: its metric across the day buckets of the range.
    ///
    /// The line is a sparkline and not a chart with axes: a terminal has room
    /// for the shape of the use and not for a plot, and the shape — a trough
    /// after a key rotated, a spike after a long turn — is what a person reads.
    /// Each model is its own line, never merged with another provider's model of
    /// the same name.
    fn time_lines(&self, out: &mut Vec<ViewLine>) {
        let series = self.model_series(self.metric);
        if series.is_empty() {
            out.push(ViewLine::dim("nothing recorded, in this range"));
            return;
        }
        let buckets: Vec<String> = report_buckets(&self.scoped(), self.range);
        out.push(ViewLine::bold(format!(
            "{} by day, {} to {}",
            self.metric.label(),
            buckets.first().map_or("–", String::as_str),
            buckets.last().map_or("–", String::as_str)
        )));
        for (label, points) in &series {
            let spark = sparkline(points);
            let peak = points.iter().copied().max().unwrap_or(0);
            out.push(ViewLine::plain(format!(
                "{:<LABEL_COLUMN$} {spark}  peak {}",
                clip(label, LABEL_COLUMN),
                metric_scalar(peak, self.metric)
            )));
        }
    }

    /// One point per bucket, per model, for the metric in force.
    fn model_series(&self, metric: Metric) -> Vec<(String, Vec<u64>)> {
        let rows = self.scoped();
        let buckets = report_buckets(&rows, self.range);
        // A model's value in a bucket: its own slice of that bucket's rows.
        let mut per_model: BTreeMap<String, BTreeMap<String, Slice>> = BTreeMap::new();
        for record in rows {
            per_model
                .entry(format!("{}/{}", record.provider, record.model))
                .or_default()
                .entry(bucket_of(record.started_at, self.range))
                .or_default()
                .add_within(record, self.now);
        }
        let mut out: Vec<(String, Vec<u64>)> = per_model
            .into_iter()
            .map(|(model, by_bucket)| {
                let points = buckets
                    .iter()
                    .map(|bucket| {
                        by_bucket
                            .get(bucket)
                            .map_or(0, |slice| metric_total(slice, metric))
                    })
                    .collect();
                (model, points)
            })
            .collect();
        // The busiest model first, so the line that matters is at the top.
        out.sort_by(|left, right| {
            let peak = |points: &Vec<u64>| points.iter().copied().max().unwrap_or(0);
            peak(&right.1)
                .cmp(&peak(&left.1))
                .then_with(|| left.0.cmp(&right.0))
        });
        out
    }
}

/// Every day bucket the rows fall in, oldest first.
fn report_buckets(rows: &[&RequestRecord], range: Range) -> Vec<String> {
    let mut buckets: BTreeSet<String> = BTreeSet::new();
    for record in rows {
        buckets.insert(bucket_of(record.started_at, range));
    }
    buckets.into_iter().collect()
}

/// A model label with its provider stripped, clipped to a column.
fn clip(label: &str, width: usize) -> String {
    label.chars().take(width).collect()
}

/// The eight-step sparkline of a series, one character per point.
fn sparkline(points: &[u64]) -> String {
    const STEPS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let peak = points.iter().copied().max().unwrap_or(0);
    if peak == 0 {
        return "·".repeat(points.len());
    }
    points
        .iter()
        .map(|point| {
            let step = usize::try_from(point.saturating_mul(7) / peak).unwrap_or(0);
            STEPS[step.min(7)]
        })
        .collect()
}

/// A metric's peak as a short word, for the line that names it.
fn metric_scalar(value: u64, metric: Metric) -> String {
    match metric {
        Metric::Tokens => format!("{} tokens", human(value)),
        Metric::Output => format!("{} output", human(value)),
        Metric::CacheRate => format!("{value}%"),
        Metric::Latency | Metric::FirstToken => format!("{value}ms"),
        Metric::Cost => format!("{value}µ$"),
        Metric::Errors => format!("{value} errors"),
    }
}

/// A stacked bar per provider: the money reported against the requests with no
/// price, because a provider that reports no cost is a fact about the provider
/// and not a zero.
///
/// The two are not the same quantity and not on the same scale — one is dollars
/// and the other a count — so the bar is two bars side by side: the cost as a
/// length, and beside it how much of the provider's traffic came back unpriced,
/// as a share. A provider that reports nothing shows the second bar full and the
/// first empty, which is exactly the thing a person needs to see.
fn cost_section(out: &mut Vec<ViewLine>, report: &Report, grow: usize) {
    let rows: Vec<(&str, &Slice)> = report
        .providers
        .iter()
        .map(|(name, slice)| (name.as_str(), slice))
        .collect();
    if rows.is_empty() {
        return;
    }
    out.push(ViewLine::plain(""));
    out.push(ViewLine::bold("cost reported, and the share not reported"));
    let most = rows
        .iter()
        .map(|(_, slice)| cost_cents(slice.cost_usd))
        .max()
        .unwrap_or(1)
        .max(1);
    for (provider, slice) in rows {
        let priced = usize::try_from(cost_cents(slice.cost_usd))
            .unwrap_or(usize::MAX)
            .saturating_mul(grow)
            .checked_div(usize::try_from(most).unwrap_or(1))
            .unwrap_or(0)
            .min(BAR_CELLS);
        let unpriced_share = usize::try_from(
            slice
                .unpriced
                .saturating_mul(BAR_CELLS as u64)
                .checked_div(slice.requests)
                .unwrap_or(0),
        )
        .unwrap_or(0)
        .min(BAR_CELLS);
        let cost_bar: String = "█".repeat(priced) + &"·".repeat(BAR_CELLS - priced);
        // The unpriced share is drawn in a lighter mark, so the two bars cannot
        // be read as one quantity.
        let miss_bar: String = "▒".repeat(unpriced_share) + &"·".repeat(BAR_CELLS - unpriced_share);
        out.push(ViewLine::plain(format!(
            "  {:<LABEL_COLUMN$} ${:<10.4} {miss_bar} {} of {} unpriced",
            clip(provider, LABEL_COLUMN),
            slice.cost_usd,
            slice.unpriced,
            slice.requests
        )));
        // The cost bar on a line of its own keeps the two scales apart.
        out.push(ViewLine::dim(format!("  {:<LABEL_COLUMN$} {cost_bar}", "")));
    }
}

/// The error bars: one per provider, subdivided by the kind of error.
///
/// A bar that says "three errors" is not enough — rate limits, auth and timeouts
/// are three different things to do something about — so each provider's bar is
/// cut into its kinds, and the longest bar sets the scale.
fn error_section(out: &mut Vec<ViewLine>, report: &Report, grow: usize) {
    let with_errors: Vec<(&str, &Slice)> = report
        .providers
        .iter()
        .filter(|(_, slice)| slice.errors > 0)
        .map(|(name, slice)| (name.as_str(), slice))
        .collect();
    if with_errors.is_empty() {
        return;
    }
    out.push(ViewLine::plain(""));
    out.push(ViewLine::bold("errors by provider, by kind"));
    let busiest = with_errors
        .iter()
        .map(|(_, slice)| slice.errors)
        .max()
        .unwrap_or(1)
        .max(1);
    for (provider, slice) in with_errors {
        out.push(ViewLine::plain(format!(
            "  {provider:<LABEL_COLUMN$} {}",
            error_bar(slice, busiest, grow)
        )));
        for (kind, count) in slice.error_tally.ranked() {
            out.push(ViewLine::dim(format!("      {count:>4}  {kind}")));
        }
    }
}

/// One provider's errors as a bar cut into segments by kind.
fn error_bar(slice: &Slice, busiest: u64, grow: usize) -> String {
    let filled = usize::try_from(slice.errors)
        .unwrap_or(usize::MAX)
        .saturating_mul(grow)
        .checked_div(usize::try_from(busiest).unwrap_or(1))
        .unwrap_or(0)
        .clamp(1, BAR_CELLS);
    // Each kind takes a slice of the bar, in turns, so a bar of one kind is one
    // run and a bar of three shows three.
    let segments = "▚▞▓▒░█";
    let kinds = slice.error_tally.ranked();
    let mut bar = String::new();
    for cell in 0..filled {
        let kind = kinds
            .get(cell * kinds.len() / filled.max(1))
            .map_or('·', |_| '█');
        let shade = segments
            .chars()
            .nth(cell % segments.chars().count())
            .unwrap_or(kind);
        bar.push(shade);
    }
    format!("{bar:<BAR_CELLS$} {}", slice.errors)
}

/// The busiest row's metric, which is the scale a bar is drawn against.
fn busiest_of(rows: &[(&str, &Slice)], metric: Metric) -> u64 {
    rows.iter()
        .map(|(_, slice)| metric_total(slice, metric))
        .max()
        .unwrap_or(1)
        .max(1)
}

/// The value a metric measures on a slice, used to size the bar.
#[must_use]
fn metric_total(slice: &Slice, metric: Metric) -> u64 {
    match metric {
        Metric::Tokens => slice.tokens(),
        Metric::Output => slice.output_tokens,
        Metric::CacheRate => slice.cache_rate().unwrap_or(0),
        Metric::Latency => slice.p50_ms(),
        Metric::FirstToken => slice.p50_ttft_ms(),
        Metric::Cost => cost_cents(slice.cost_usd),
        Metric::Errors => slice.errors,
    }
}

/// One ranked row: a label, a bar, and the value the bar is drawn for.
///
/// `grow` is how much of each bar is drawn: the entry animation passes a value
/// under `BAR_CELLS` for the first frames, and `BAR_CELLS` once the panel has
/// settled, which is every frame after the last.
#[must_use]
fn rank_row(label: &str, slice: &Slice, metric: Metric, busiest: u64, grow: usize) -> String {
    let value = metric_total(slice, metric);
    let filled = usize::try_from(value)
        .unwrap_or(usize::MAX)
        .saturating_mul(grow)
        .checked_div(usize::try_from(busiest).unwrap_or(1))
        .unwrap_or(0)
        .min(grow);
    let bar: String = "█".repeat(filled) + &"·".repeat(BAR_CELLS - filled);
    let clipped: String = label.chars().take(LABEL_COLUMN).collect();
    format!(
        "{clipped:<LABEL_COLUMN$} {bar}  {}",
        metric_value(slice, metric)
    )
}

/// A metric's value on a slice, as the row prints it.
#[must_use]
fn metric_value(slice: &Slice, metric: Metric) -> String {
    match metric {
        Metric::Tokens => format!("{} tokens", human(slice.tokens())),
        Metric::Output => format!("{} output", human(slice.output_tokens)),
        Metric::CacheRate => slice
            .cache_rate()
            .map_or_else(|| "no cache".to_owned(), |rate| format!("{rate}% cached")),
        Metric::Latency => format!("{}ms p50", slice.p50_ms()),
        Metric::FirstToken => format!("{}ms ttft", slice.p50_ttft_ms()),
        Metric::Cost => {
            if slice.unpriced == slice.requests && slice.requests > 0 {
                "cost not reported".to_owned()
            } else {
                format!("${:.4}", slice.cost_usd)
            }
        }
        Metric::Errors => format!("{} error(s)", slice.errors),
    }
}

/// The cost in millionths of a dollar, clamped at zero, for sizing a bar.
///
/// The bar needs an integer and the cost is a float; a negative cost is not a
/// thing a provider reports, and clamping means it cannot size a bar backwards.
#[must_use]
fn cost_cents(cost_usd: f64) -> u64 {
    let scaled = (cost_usd * 1_000_000.0).round();
    if scaled <= 0.0 {
        return 0;
    }
    // The value is checked non-negative above and bounded here, so the cast
    // cannot lose a sign or wrap.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bounded = scaled.min(u64::MAX as f64) as u64;
    bounded
}

/// A count of tokens, short: `1.2M`, `34k`, `900`.
#[must_use]
fn human(tokens: u64) -> String {
    crate::render::human_tokens(tokens)
}

/// The summary facts of a slice, in the order the panel prints them.
#[must_use]
fn summary_facts(slice: &Slice) -> Vec<(&'static str, String)> {
    let mut facts = vec![
        ("requests", slice.requests.to_string()),
        (
            "tokens",
            format!(
                "{} in · {} out",
                human(slice.input_tokens),
                human(slice.output_tokens)
            ),
        ),
        (
            "latency",
            format!("{}ms p50 · {}ms p95", slice.p50_ms(), slice.p95_ms()),
        ),
        ("first token", format!("{}ms p50", slice.p50_ttft_ms())),
    ];
    if let Some(rate) = slice.cache_rate() {
        facts.push(("cache", format!("{rate}% of the prompt")));
    }
    facts.push((
        "cost",
        if slice.unpriced == slice.requests && slice.requests > 0 {
            "not reported".to_owned()
        } else {
            format!("${:.4}", slice.cost_usd)
        },
    ));
    if slice.errors + slice.cancelled > 0 {
        facts.push((
            "errors",
            format!("{} error(s), {} cancelled", slice.errors, slice.cancelled),
        ));
    }
    facts
}

#[cfg(test)]
mod tests {
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
    fn the_time_tab_draws_a_sparkline_per_model() {
        // Two models over two days: each gets its own line, and the shape is the
        // use, not a merged total.
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
        let text = text_of(&panel);
        assert!(text.contains("over time"), "{text}");
        assert!(text.contains("openai/gpt-5"), "{text}");
        assert!(text.contains("openrouter/gpt-5"), "{text}");
        // A full block for the peak day and a lower step for the smaller one.
        assert!(text.contains('█') || text.contains('▇'), "{text}");
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
}
