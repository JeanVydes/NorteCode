//! The cost ledger and budgets.
//!
//! Norte Code never prices anything: the only dollar figure is the one the
//! provider reports in a response's usage payload. The ledger stores one row
//! per model request, with the reported cost or nothing, and budgets are
//! evaluated against what was really spent.
//!
//! The types here are plain data: the `SQLite` store persists them, the service
//! records them from Norte's committed history, and the CLI prints rollups.
//! Numeric USD totals are only meaningful when [`CostSummary::usd_measurable`]
//! is true, which means every request in the summary reported a cost.

use std::collections::BTreeMap;

use crate::{Budget, BudgetAction, BudgetScope, BudgetUnit};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The fraction of a limit at which a warning is emitted.
pub const WARN_FRACTION: f64 = 0.8;

/// One model request, as the provider reported it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostRecord {
    /// The logical session key.
    pub session: String,
    /// The turn the request belonged to.
    pub turn: u64,
    /// The request's index within the session, starting at one.
    pub index: u64,
    /// The `provider/model` that was asked.
    pub model: String,
    /// Input tokens the provider reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Output tokens the provider reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Reasoning tokens, counted inside `output_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Tokens read from the provider's cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Tokens written to the provider's cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// The cost the provider reported, or `None` when it reported none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// When Norte Code recorded the row.
    pub recorded_at: DateTime<Utc>,
}

/// A rollup of zero or more requests.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CostSummary {
    /// Requests in the summary.
    pub requests: u64,
    /// Input tokens, summed over what providers reported.
    pub input_tokens: u64,
    /// Output tokens, summed over what providers reported.
    pub output_tokens: u64,
    /// Reasoning tokens, counted inside `output_tokens`.
    pub reasoning_tokens: u64,
    /// Cache-read tokens.
    pub cache_read_tokens: u64,
    /// Cache-write tokens.
    pub cache_write_tokens: u64,
    /// Provider-reported cost, summed.
    pub reported_cost_usd: f64,
    /// Requests whose payload carried no cost.
    pub not_reported: u64,
}

impl CostSummary {
    /// Adds one request to the summary.
    pub fn add(&mut self, record: &CostRecord) {
        self.count_request();
        self.add_usage(record);
    }

    /// Counts one request, without what it cost.
    ///
    /// A request that reported no usage still counts: the budget measures how
    /// much the session asked for, not only what the providers billed.
    pub fn count_request(&mut self) {
        self.requests += 1;
    }

    /// Adds what a request cost, without counting it again.
    pub fn add_usage(&mut self, record: &CostRecord) {
        self.input_tokens += record.input_tokens.unwrap_or(0);
        self.output_tokens += record.output_tokens.unwrap_or(0);
        self.reasoning_tokens += record.reasoning_tokens.unwrap_or(0);
        self.cache_read_tokens += record.cache_read_tokens.unwrap_or(0);
        self.cache_write_tokens += record.cache_write_tokens.unwrap_or(0);
        if let Some(cost) = record.cost_usd {
            self.reported_cost_usd += cost;
        } else {
            self.not_reported += 1;
        }
    }

    /// Adds another summary to this one.
    pub fn merge(&mut self, other: &Self) {
        self.requests += other.requests;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.reported_cost_usd += other.reported_cost_usd;
        self.not_reported += other.not_reported;
    }

    /// Input plus output tokens. Reasoning is already part of the output count.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// Whether a USD total is complete: every request reported a cost.
    ///
    /// An empty summary is measurable at zero, so a new session can start with
    /// a USD budget before its first request.
    #[must_use]
    pub fn usd_measurable(&self) -> bool {
        self.not_reported == 0
    }
}

/// Sums an iterator of records.
pub fn summarize<'a>(records: impl IntoIterator<Item = &'a CostRecord>) -> CostSummary {
    let mut summary = CostSummary::default();
    for record in records {
        summary.add(record);
    }
    summary
}

/// Rolls records up by session key.
#[must_use]
pub fn by_session(records: &[CostRecord]) -> BTreeMap<String, CostSummary> {
    let mut groups: BTreeMap<String, CostSummary> = BTreeMap::new();
    for record in records {
        groups
            .entry(record.session.clone())
            .or_default()
            .add(record);
    }
    groups
}

/// Rolls records up by `(session, turn)`, which is one user message.
#[must_use]
pub fn by_turn(records: &[CostRecord]) -> BTreeMap<(String, u64), CostSummary> {
    let mut groups: BTreeMap<(String, u64), CostSummary> = BTreeMap::new();
    for record in records {
        groups
            .entry((record.session.clone(), record.turn))
            .or_default()
            .add(record);
    }
    groups
}

/// Rolls records up by UTC day (`YYYY-MM-DD`).
#[must_use]
pub fn by_day(records: &[CostRecord]) -> BTreeMap<String, CostSummary> {
    let mut groups: BTreeMap<String, CostSummary> = BTreeMap::new();
    for record in records {
        groups
            .entry(record.recorded_at.format("%Y-%m-%d").to_string())
            .or_default()
            .add(record);
    }
    groups
}

/// One budget's measured position.
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetStatus {
    /// The scope the budget belongs to.
    pub scope: BudgetScope,
    /// What it measures.
    pub unit: BudgetUnit,
    /// What to do when the limit is reached.
    pub action: BudgetAction,
    /// Measured spend.
    pub spent: f64,
    /// The configured limit.
    pub limit: f64,
}

impl BudgetStatus {
    /// Spend divided by limit.
    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.limit <= 0.0 {
            f64::INFINITY
        } else {
            self.spent / self.limit
        }
    }

    /// Whether the warning threshold was reached.
    #[must_use]
    pub fn is_warning(&self) -> bool {
        self.ratio() >= WARN_FRACTION
    }

    /// Whether the limit was reached or exceeded.
    #[must_use]
    pub fn is_exceeded(&self) -> bool {
        self.spent >= self.limit
    }
}

/// Measures a budget against a summary.
///
/// A USD budget over requests that did not report cost is not measurable, so
/// this returns `None` rather than pretending the spend is zero.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn measure(budget: &Budget, summary: &CostSummary) -> Option<f64> {
    // Counters above 2^53 would lose precision as an `f64`, but a budget at
    // that scale is already far beyond any real conversation.
    match budget.unit {
        BudgetUnit::Usd => summary
            .usd_measurable()
            .then_some(summary.reported_cost_usd),
        BudgetUnit::Tokens => Some(summary.total_tokens() as f64),
        BudgetUnit::Requests => Some(summary.requests as f64),
    }
}

/// The status of one budget, when it can be measured.
#[must_use]
pub fn status(scope: BudgetScope, budget: &Budget, summary: &CostSummary) -> Option<BudgetStatus> {
    Some(BudgetStatus {
        scope,
        unit: budget.unit,
        action: budget.action,
        spent: measure(budget, summary)?,
        limit: budget.limit,
    })
}

/// A warning or an exhaustion notice newly produced by [`BudgetBook::notices`].
#[derive(Clone, Debug, PartialEq)]
pub enum BudgetNotice {
    /// The warning threshold was crossed.
    Warning(BudgetStatus),
    /// The limit was reached.
    Exceeded(BudgetStatus),
}

/// The budgets in force for one turn and the spend they measure.
#[derive(Clone, Debug, Default)]
pub struct BudgetBook {
    /// The budget for this message, if one was set.
    pub message: Option<Budget>,
    /// The budget for the session.
    pub session: Option<Budget>,
    /// The budget for the UTC day, across sessions.
    pub global_daily: Option<Budget>,
    /// Spend in the current message.
    pub message_spent: CostSummary,
    /// Spend in the session, including the current message.
    pub session_spent: CostSummary,
    /// Spend today, across sessions.
    pub global_spent: CostSummary,
    warned: Vec<BudgetScope>,
    exceeded: Vec<BudgetScope>,
    stopped: Option<BudgetStatus>,
}

impl BudgetBook {
    /// A book with no budgets but with the spend measured so far.
    #[must_use]
    pub fn new(
        message: Option<Budget>,
        session: Option<Budget>,
        global_daily: Option<Budget>,
        message_spent: CostSummary,
        session_spent: CostSummary,
        global_spent: CostSummary,
    ) -> Self {
        Self {
            message,
            session,
            global_daily,
            message_spent,
            session_spent,
            global_spent,
            warned: Vec::new(),
            exceeded: Vec::new(),
            stopped: None,
        }
    }

    /// Replaces the session budget, clearing its previous notice state.
    pub fn set_session_budget(&mut self, budget: Option<Budget>) {
        self.session = budget;
        self.warned.retain(|scope| *scope != BudgetScope::Session);
        self.exceeded.retain(|scope| *scope != BudgetScope::Session);
        if self
            .stopped
            .as_ref()
            .is_some_and(|s| s.scope == BudgetScope::Session)
        {
            self.stopped = None;
        }
    }

    /// Adds one recorded request to every measured scope.
    pub fn record(&mut self, record: &CostRecord) {
        self.message_spent.add(record);
        self.session_spent.add(record);
        self.global_spent.add(record);
    }

    /// Counts one request in every measured scope.
    ///
    /// The count does not wait for the usage: a request that reported none
    /// still counts, and the gate that measures the budget sees it before it
    /// decides about the next request.
    pub fn count_request(&mut self) {
        self.message_spent.count_request();
        self.session_spent.count_request();
        self.global_spent.count_request();
    }

    /// Adds what a request cost to every measured scope, without counting it
    /// again.
    pub fn record_usage(&mut self, record: &CostRecord) {
        self.message_spent.add_usage(record);
        self.session_spent.add_usage(record);
        self.global_spent.add_usage(record);
    }

    /// The budgets and summaries in evaluation order.
    fn entries(&self) -> [(&Option<Budget>, BudgetScope, &CostSummary); 3] {
        [
            (&self.message, BudgetScope::Message, &self.message_spent),
            (&self.session, BudgetScope::Session, &self.session_spent),
            (
                &self.global_daily,
                BudgetScope::GlobalDaily,
                &self.global_spent,
            ),
        ]
    }

    /// The current status of one scope, when its budget is measurable.
    #[must_use]
    pub fn status_of(&self, scope: BudgetScope) -> Option<BudgetStatus> {
        let (budget, _, summary) = self
            .entries()
            .into_iter()
            .find(|(_, candidate, _)| *candidate == scope)?;
        status(scope, budget.as_ref()?, summary)
    }

    /// The first exhaustion that should stop the turn, if any.
    ///
    /// `Warn` budgets never stop. A USD budget over unreported requests is
    /// skipped rather than treated as zero.
    pub fn stop(&mut self) -> Option<BudgetStatus> {
        let statuses = self.statuses();
        self.stopped = statuses
            .into_iter()
            .find(|status| status.is_exceeded() && status.action != BudgetAction::Warn);
        if let Some(status) = &self.stopped
            && !self.exceeded.contains(&status.scope)
        {
            // The layer reports the exhaustion itself; the final check must
            // not report the same scope again.
            self.exceeded.push(status.scope);
        }
        self.stopped.clone()
    }

    /// The last stop decision, without re-measuring.
    #[must_use]
    pub fn stopped(&self) -> Option<&BudgetStatus> {
        self.stopped.as_ref()
    }

    /// The measurable status of every configured budget.
    fn statuses(&self) -> Vec<BudgetStatus> {
        self.entries()
            .into_iter()
            .filter_map(|(budget, scope, summary)| status(scope, budget.as_ref()?, summary))
            .collect()
    }

    /// Newly crossed warnings, each reported once.
    ///
    /// Mid-turn checks use this: an exhaustion is reported by the layer that
    /// stops the next request, or by the final [`Self::notices`] call when the
    /// turn ends first.
    pub fn warnings(&mut self) -> Vec<BudgetNotice> {
        self.take_notices(false)
    }

    /// Newly crossed warnings and exhaustions, each reported once.
    pub fn notices(&mut self) -> Vec<BudgetNotice> {
        self.take_notices(true)
    }

    fn take_notices(&mut self, include_exceeded: bool) -> Vec<BudgetNotice> {
        let mut notices = Vec::new();
        for status in self.statuses() {
            let scope = status.scope;
            if status.is_warning() && !self.warned.contains(&scope) {
                self.warned.push(scope);
                notices.push(BudgetNotice::Warning(status.clone()));
            }
            if include_exceeded && status.is_exceeded() && !self.exceeded.contains(&scope) {
                self.exceeded.push(scope);
                notices.push(BudgetNotice::Exceeded(status));
            }
        }
        notices
    }
}

#[cfg(test)]
mod tests {
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
}
