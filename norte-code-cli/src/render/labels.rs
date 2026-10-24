//! Human labels for the budget, error and reasoning enums, plus the
//! parsers the command line shares with them.

use norte_code_agent::DEFAULT_SYSTEM_PROMPT;

use super::{Budget, BudgetAction, BudgetScope, BudgetUnit, ErrorCode, Reasoning};

pub(crate) fn short(call_id: &str) -> &str {
    call_id.get(..8).unwrap_or(call_id)
}

pub(crate) fn scope_name(scope: BudgetScope) -> &'static str {
    match scope {
        BudgetScope::Message => "message",
        BudgetScope::Session => "session",
        BudgetScope::GlobalDaily => "today",
    }
}

pub(crate) fn unit_name(unit: BudgetUnit) -> &'static str {
    match unit {
        BudgetUnit::Usd => "USD",
        BudgetUnit::Tokens => "tokens",
        BudgetUnit::Requests => "requests",
    }
}

pub(crate) fn budget_action_name(action: BudgetAction) -> &'static str {
    match action {
        BudgetAction::Warn => "warn",
        BudgetAction::Ask => "ask",
        BudgetAction::Stop => "stop",
    }
}

pub(crate) fn format_measure(unit: BudgetUnit, value: f64) -> String {
    match unit {
        BudgetUnit::Usd => format!("${value:.6}"),
        BudgetUnit::Tokens | BudgetUnit::Requests => format!("{value:.0}"),
    }
}

pub(crate) fn parse_budget_unit(unit: &str) -> Result<BudgetUnit, String> {
    match unit {
        "usd" => Ok(BudgetUnit::Usd),
        "tokens" => Ok(BudgetUnit::Tokens),
        "requests" => Ok(BudgetUnit::Requests),
        other => Err(format!("unknown budget unit `{other}`")),
    }
}

pub(crate) fn parse_budget_action(action: &str) -> Result<BudgetAction, String> {
    match action {
        "warn" => Ok(BudgetAction::Warn),
        "ask" => Ok(BudgetAction::Ask),
        "stop" => Ok(BudgetAction::Stop),
        other => Err(format!("unknown budget action `{other}`")),
    }
}

/// Selects the system prompt: flag, then config, then the built-in default.
pub(crate) fn select_system_prompt(
    flag: Option<String>,
    configured: Option<String>,
) -> (&'static str, String) {
    if let Some(prompt) = flag {
        ("flag", prompt)
    } else if let Some(prompt) = configured {
        ("config", prompt)
    } else {
        ("default", DEFAULT_SYSTEM_PROMPT.to_owned())
    }
}

/// Parses `--reasoning`. The service still checks the setting against the
/// selected model, so this only catches names no model could accept.
pub(crate) fn parse_reasoning_setting(value: &str) -> Result<Reasoning, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if let Some(tokens) = normalized
        .strip_prefix("budget:")
        .or_else(|| normalized.strip_prefix("budget="))
    {
        let tokens = tokens.parse::<u32>().map_err(|_| {
            format!("`{value}` is not a reasoning budget; use budget:<positive token count>")
        })?;
        if tokens == 0 {
            return Err("the reasoning budget must be greater than zero".to_owned());
        }
        return Ok(Reasoning::Budget { tokens });
    }
    match normalized.as_str() {
        "auto" | "automatic" => Ok(Reasoning::Automatic),
        "off" | "none" | "disabled" => Ok(Reasoning::Disabled),
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
            let effort = match normalized.as_str() {
                "minimal" => "minimal",
                "low" => "low",
                "medium" => "medium",
                "high" => "high",
                "xhigh" => "xhigh",
                _ => "max",
            };
            Ok(Reasoning::Effort {
                effort: effort.to_owned(),
            })
        }
        _ => Err(format!(
            "`{value}` is not a reasoning setting; use auto, off, minimal, low, medium, high, \
             xhigh, max or budget:<tokens>"
        )),
    }
}

pub(crate) fn budget_label(budget: &Budget) -> String {
    format!(
        "{} {} ({})",
        format_measure(budget.unit, budget.limit),
        unit_name(budget.unit),
        budget_action_name(budget.action)
    )
}

pub(crate) fn code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidRequest => "invalid request",
        ErrorCode::NotFound => "not found",
        ErrorCode::Conflict => "conflict",
        ErrorCode::Capability => "capability",
        ErrorCode::Budget => "budget",
        ErrorCode::Provider => "provider",
        ErrorCode::Internal => "internal",
        ErrorCode::Unsupported => "unsupported",
    }
}
