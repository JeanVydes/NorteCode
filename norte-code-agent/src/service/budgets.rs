//! The budgets of a session and the ledger they are measured against.
//!
//! A budget says how much a session may spend, in one of three units, and
//! what happens when it runs out. The book that measures it lives here, along
//! with the rules that decide whether a budget can be set at all.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::Utc;

use norte_code_cost::{BudgetBook, BudgetNotice, BudgetStatus, CostSummary, summarize};
use norte_code_protocol::{Budget, BudgetUnit, ServerEventKind, SessionKey};

use super::mapping::configured_budget;
use super::{Service, ServiceError, single};

/// The budget state of every session the service holds.
pub(super) type BudgetStates = BTreeMap<SessionKey, Arc<Mutex<BudgetBook>>>;

impl Service {
    /// The budget state of a session, with the message budget the
    /// configuration gives.
    pub(super) async fn budget_book(
        &self,
        key: &SessionKey,
    ) -> Result<Arc<Mutex<BudgetBook>>, ServiceError> {
        if let Some(book) = self.lock_budget_states().get(key).cloned() {
            return Ok(book);
        }
        let message = configured_budget(self.config().budgets.message.as_ref());
        let book = self.new_budget_book(key, message).await?;
        self.lock_budget_states()
            .insert(key.clone(), Arc::clone(&book));
        Ok(book)
    }

    /// The budget state of every session.
    pub(super) fn lock_budget_states(&self) -> MutexGuard<'_, BudgetStates> {
        self.budget_states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Builds the budget state for a turn from the session and the day's ledger.
    pub(super) async fn new_budget_book(
        &self,
        key: &SessionKey,
        message: Option<Budget>,
    ) -> Result<Arc<Mutex<BudgetBook>>, ServiceError> {
        let (session_budget, session_summary) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (
                record.budget.clone(),
                CostSummary {
                    requests: record.requests,
                    input_tokens: record.input_tokens,
                    output_tokens: record.output_tokens,
                    reported_cost_usd: record.cost,
                    not_reported: record.unpriced,
                    ..CostSummary::default()
                },
            )
        };
        let today = Utc::now().date_naive();
        let costs = self.directory.costs(None).await?;
        let global_summary = summarize(
            costs
                .iter()
                .filter(|record| record.recorded_at.date_naive() == today),
        );
        Ok(Arc::new(Mutex::new(BudgetBook::new(
            message,
            session_budget,
            configured_budget(self.config().budgets.global_daily.as_ref()),
            CostSummary::default(),
            session_summary,
            global_summary,
        ))))
    }

    /// The budget the configuration gives every session.
    pub(super) fn configured_session_budget(&self) -> Option<Budget> {
        configured_budget(self.config().budgets.session.as_ref())
    }

    /// The notices the book has for the cost it recorded.
    ///
    /// A warning crosses once and an exhaustion is only reported when the turn
    /// ends, so the client hears about each of them once. The book is the one
    /// that records: the budget layer writes what each request cost, in the
    /// engine's path, so the gate knows the cost of the request it let through
    /// before it decides about the next one.
    pub(super) fn budget_notices(&self, key: &SessionKey, final_check: bool) -> Vec<BudgetNotice> {
        self.lock_budget_states()
            .get(key)
            .cloned()
            .map_or_else(Vec::new, |book| {
                let mut book = book.lock().unwrap_or_else(PoisonError::into_inner);
                if final_check {
                    book.notices()
                } else {
                    book.warnings()
                }
            })
    }

    /// The status of the budget that stopped the session, if any.
    pub(super) fn budget_stopped(&self, key: &SessionKey) -> Option<BudgetStatus> {
        self.lock_budget_states().get(key).and_then(|book| {
            book.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .stopped()
                .cloned()
        })
    }

    /// Validates and sets the session's budget.
    ///
    /// A USD budget over a provider that never reported cost cannot be
    /// measured, so it is refused instead of silently comparing against zero.
    pub(super) async fn set_budget(
        &self,
        request_id: u64,
        key: SessionKey,
        budget: Budget,
    ) -> Result<super::EventStream, ServiceError> {
        self.validate_budget(&key, &budget).await?;
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.budget = Some(budget.clone());
        }
        self.persist(&key).await?;
        if let Some(book) = self.lock_budget_states().get(&key).cloned() {
            book.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .set_session_budget(Some(budget.clone()));
        }
        Ok(single(
            request_id,
            Some(key),
            ServerEventKind::BudgetChanged {
                budget: Some(budget),
            },
        ))
    }

    /// Refuses a budget the session cannot measure.
    pub(super) async fn validate_budget(
        &self,
        key: &SessionKey,
        budget: &Budget,
    ) -> Result<(), ServiceError> {
        if !budget.limit.is_finite() || budget.limit <= 0.0 {
            return Err(ServiceError::Budget(
                "the limit must be a positive, finite number".to_owned(),
            ));
        }
        if budget.unit != BudgetUnit::Usd {
            return Ok(());
        }
        let model = self.model_label(key);
        let unpriced_in_session = {
            let sessions = self.lock_sessions();
            sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?
                .unpriced
        };
        let unpriced_model = self
            .directory
            .costs(Some(key.as_str()))
            .await?
            .iter()
            .filter(|record| record.model == model && record.cost_usd.is_none())
            .count();
        if unpriced_in_session > 0 || unpriced_model > 0 {
            return Err(ServiceError::Budget(format!(
                "`{model}` has requests with no reported cost; use a tokens or requests \
                 budget, or a provider that reports cost"
            )));
        }
        Ok(())
    }

    /// Blocks a switch that would leave a USD budget unmeasurable.
    pub(super) async fn ensure_model_supports_budget(
        &self,
        key: &SessionKey,
        spec: &super::ModelSpec,
    ) -> Result<(), ServiceError> {
        let (budget, unpriced) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (record.budget.clone(), record.unpriced)
        };
        if budget
            .as_ref()
            .is_none_or(|budget| budget.unit != BudgetUnit::Usd)
        {
            return Ok(());
        }
        let model = spec.label();
        let costs = self.directory.costs(Some(key.as_str())).await?;
        let new_model_costs: Vec<_> = costs
            .into_iter()
            .filter(|record| record.model == model)
            .collect();
        let unpriced_new = new_model_costs
            .iter()
            .filter(|record| record.cost_usd.is_none())
            .count();
        if unpriced > 0 || unpriced_new > 0 {
            return Err(ServiceError::Budget(format!(
                "the session has a USD budget, but `{model}` has requests without a \
                 reported cost; set a tokens or requests budget before switching"
            )));
        }
        Ok(())
    }
}
