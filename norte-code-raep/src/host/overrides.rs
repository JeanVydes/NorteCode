//! Decoding session and turn configuration overrides.

use serde_json::Value;

use super::HostError;

/// Extracts the reasoning, model and budget overrides from one override
/// object.
///
/// Only the fields in the closed set below are accepted; any other key
/// fails with `AGENT_OVERRIDE_DENIED`, so a client that sends a field this
/// host cannot honour gets an error naming it rather than a silent no-op.
pub(crate) type OverrideParts = (
    Option<norte_code_wire::Reasoning>,
    Option<(String, String)>,
    Option<norte_code_wire::Budget>,
);

pub(crate) fn decode_override(value: &Value) -> Result<OverrideParts, HostError> {
    let Value::Object(fields) = value else {
        return Err(HostError::InvalidFrame(
            "overrides must be an object".to_owned(),
        ));
    };
    let mut reasoning = None;
    let mut model = None;
    let mut budget = None;
    for (key, field) in fields {
        match key.as_str() {
            "reasoning" => {
                reasoning = Some(decode_reasoning_override(field)?);
            }
            "model" => {
                let id = field.get("id").and_then(Value::as_str).ok_or_else(|| {
                    HostError::InvalidFrame("model override requires `id`".to_owned())
                })?;
                let provider = field
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                model = Some((provider, id.to_owned()));
            }
            "provider" => {
                let id = field.as_str().ok_or_else(|| {
                    HostError::InvalidFrame("provider override must be a string".to_owned())
                })?;
                model = Some((id.to_owned(), model.map(|(_, m)| m).unwrap_or_default()));
            }
            "budgets" | "budget" => {
                budget = Some(decode_budget_override(field)?);
            }
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "override field `{other}` is not supported by this host"
                )));
            }
        }
    }
    Ok((reasoning, model, budget))
}

/// Rejects override values this host cannot honour.
///
/// This is the `AGENT_OVERRIDE_DENIED` check the spec requires (RFC 7):
/// narrowing is allowed, widening scopes, tool sets or budgets is not. This
/// host has no scopes or tool sets to widen, and budgets can only be set —
/// there is no raise — so the check is that the value decodes at all. A
/// malformed value fails here, naming the field, instead of deep inside the
/// service.
pub(crate) fn validate_override(
    reasoning: Option<&norte_code_wire::Reasoning>,
    model: Option<&(String, String)>,
    budget: Option<&norte_code_wire::Budget>,
) -> Result<(), HostError> {
    if let Some(norte_code_wire::Reasoning::Effort { effort }) = reasoning
        && effort.is_empty()
    {
        return Err(HostError::InvalidFrame(
            "reasoning.effort must not be empty".to_owned(),
        ));
    }
    if let Some((_, model_id)) = model
        && model_id.is_empty()
    {
        return Err(HostError::InvalidFrame(
            "model.id must not be empty".to_owned(),
        ));
    }
    if let Some(budget) = budget
        && budget.limit <= 0.0
    {
        return Err(HostError::InvalidFrame(
            "budget limit must be positive".to_owned(),
        ));
    }
    Ok(())
}

/// Decodes a `reasoning` override object into a [`norte_code_wire::Reasoning`].
///
/// The wire shape follows the `AgentSpec` fragment (RFC 7): `mode`
/// (`off`/`auto`/`on`), `effort` (a name) and `budget_tokens`. `mode: on`
/// without an effort maps to `Effort` with the service's default effort;
/// `mode: auto` maps to `Automatic` and `mode: off` to `Disabled`.
pub(crate) fn decode_reasoning_override(
    value: &Value,
) -> Result<norte_code_wire::Reasoning, HostError> {
    let Some(fields) = value.as_object() else {
        return Err(HostError::InvalidFrame(
            "reasoning override must be an object".to_owned(),
        ));
    };
    for key in fields.keys() {
        match key.as_str() {
            "mode" | "effort" | "budget_tokens" => {}
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "reasoning override field `{other}` is not supported"
                )));
            }
        }
    }
    let mode = fields.get("mode").and_then(Value::as_str).unwrap_or("auto");
    let effort = fields.get("effort").and_then(Value::as_str);
    let budget_tokens = fields.get("budget_tokens").and_then(Value::as_u64);
    Ok(match mode {
        "off" | "disabled" => norte_code_wire::Reasoning::Disabled,
        "auto" | "automatic" => norte_code_wire::Reasoning::Automatic,
        "on" => {
            if let Some(tokens) = budget_tokens {
                norte_code_wire::Reasoning::Budget {
                    tokens: u32::try_from(tokens).map_err(|_| {
                        HostError::InvalidFrame(
                            "reasoning.budget_tokens is out of range".to_owned(),
                        )
                    })?,
                }
            } else if let Some(effort) = effort {
                norte_code_wire::Reasoning::Effort {
                    effort: effort.to_owned(),
                }
            } else {
                norte_code_wire::Reasoning::Effort {
                    effort: "medium".to_owned(),
                }
            }
        }
        other => {
            return Err(HostError::InvalidFrame(format!(
                "reasoning.mode `{other}` is not supported"
            )));
        }
    })
}

/// Decodes a `budgets` override object into a [`norte_code_wire::Budget`].
///
/// The wire shape follows the `AgentSpec` fragment: `max_turn_cost` /
/// `max_session_cost` (with `currency`, default `USD`) and `max_turn_tokens`
/// map to `BudgetUnit::Usd` / `BudgetUnit::Tokens`. The service's current
/// budget semantics are a single per-session limit, so the first field
/// present wins; a field this host does not map fails instead of being
/// silently dropped.
pub(crate) fn decode_budget_override(value: &Value) -> Result<norte_code_wire::Budget, HostError> {
    let Some(fields) = value.as_object() else {
        return Err(HostError::InvalidFrame(
            "budget override must be an object".to_owned(),
        ));
    };
    for key in fields.keys() {
        match key.as_str() {
            "max_turn_cost" | "max_session_cost" | "currency" | "max_turn_tokens"
            | "max_session_tokens" | "on_exceed" => {}
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "budget override field `{other}` is not supported"
                )));
            }
        }
    }
    let action_name = match fields.get("on_exceed") {
        None => "stop",
        Some(Value::String(value)) => value.as_str(),
        Some(_) => {
            return Err(HostError::InvalidFrame(
                "budget on_exceed must be a string".to_owned(),
            ));
        }
    };
    let action = match action_name {
        "ask" => norte_code_wire::BudgetAction::Ask,
        "degrade" | "warn" => norte_code_wire::BudgetAction::Warn,
        "stop" => norte_code_wire::BudgetAction::Stop,
        other => {
            return Err(HostError::InvalidFrame(format!(
                "budget on_exceed `{other}` is not supported"
            )));
        }
    };
    if let Some(currency) = fields.get("currency")
        && currency.as_str() != Some("USD")
    {
        return Err(HostError::InvalidFrame(
            "budget currency must be `USD`".to_owned(),
        ));
    }
    if let Some(cost) = fields
        .get("max_turn_cost")
        .or_else(|| fields.get("max_session_cost"))
        .and_then(Value::as_f64)
        .or_else(|| {
            fields
                .get("max_turn_cost")
                .or_else(|| fields.get("max_session_cost"))
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok().map(f64::from))
        })
    {
        return Ok(norte_code_wire::Budget {
            unit: norte_code_wire::BudgetUnit::Usd,
            limit: cost,
            action,
        });
    }
    if let Some(tokens) = fields
        .get("max_turn_tokens")
        .or_else(|| fields.get("max_session_tokens"))
        .and_then(Value::as_u64)
    {
        let tokens = u32::try_from(tokens).map_err(|_| {
            HostError::InvalidFrame("budget token limit is out of range".to_owned())
        })?;
        return Ok(norte_code_wire::Budget {
            unit: norte_code_wire::BudgetUnit::Tokens,
            limit: f64::from(tokens),
            action,
        });
    }
    Err(HostError::InvalidFrame(
        "budget override requires a cost or token limit".to_owned(),
    ))
}
