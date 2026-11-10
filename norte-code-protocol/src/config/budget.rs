//! Spending limits by scope.

use serde::{Deserialize, Serialize};

use norte_code_wire::{BudgetAction, BudgetUnit};

/// The three budget scopes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetsConfig {
    /// One message.
    pub message: Option<BudgetConfig>,
    /// One session.
    pub session: Option<BudgetConfig>,
    /// The day, across sessions.
    pub global_daily: Option<BudgetConfig>,
}

/// One budget.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    /// `usd`, `tokens` or `requests`.
    pub unit: BudgetUnit,
    /// The limit.
    pub limit: f64,
    /// `warn`, `ask` or `stop`.
    pub action: BudgetAction,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            unit: BudgetUnit::Usd,
            limit: 0.0,
            action: BudgetAction::Warn,
        }
    }
}
