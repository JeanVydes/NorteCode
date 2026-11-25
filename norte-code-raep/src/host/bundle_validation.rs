//! Validation of portable session bundles.

use serde_json::Value;

use super::{HostError, PortableHistoryItem, PortableHistoryRole};

pub(crate) fn parse_portable_history(
    bytes: &[u8],
) -> Result<Vec<norte_code_agent::service::ImportedHistoryItem>, HostError> {
    let mut items = Vec::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let item: PortableHistoryItem = serde_json::from_slice(line)?;
        if item.rewound {
            return Err(HostError::InvalidFrame(
                "BUNDLE_UNSUPPORTED: rewound history cannot be restored by this host".to_owned(),
            ));
        }
        let content = match item.content {
            Value::String(content) => content,
            Value::Array(parts) => {
                let mut content = String::new();
                for part in parts {
                    if part.get("type").and_then(Value::as_str) != Some("text") {
                        return Err(HostError::InvalidFrame(
                            "BUNDLE_UNSUPPORTED: non-text history parts are not supported"
                                .to_owned(),
                        ));
                    }
                    let text = part.get("text").and_then(Value::as_str).ok_or_else(|| {
                        HostError::InvalidFrame(
                            "BUNDLE_INVALID: text content part has no text".to_owned(),
                        )
                    })?;
                    content.push_str(text);
                }
                content
            }
            _ => {
                return Err(HostError::InvalidFrame(
                    "BUNDLE_INVALID: history content must be text".to_owned(),
                ));
            }
        };
        items.push(norte_code_agent::service::ImportedHistoryItem {
            role: match item.role {
                PortableHistoryRole::User => norte_code_agent::service::ImportedHistoryRole::User,
                PortableHistoryRole::Assistant => {
                    norte_code_agent::service::ImportedHistoryRole::Assistant
                }
            },
            content,
            turn_id: item.turn_id,
            timestamp: item.timestamp,
            origin: item.origin.or(item.provenance),
        });
        if items.len() > 100_000 {
            return Err(HostError::InvalidFrame(
                "BUNDLE_INVALID: too many history items".to_owned(),
            ));
        }
    }
    Ok(items)
}

pub(crate) fn validate_bundle_usage(usage: &Value) -> Result<(), HostError> {
    let Some(object) = usage.as_object() else {
        return Err(HostError::InvalidFrame(
            "BUNDLE_INVALID: usage_total must be an object".to_owned(),
        ));
    };
    for key in [
        "input_tokens",
        "output_tokens",
        "cached_tokens",
        "reasoning_tokens",
        "tool_calls",
        "duration_ms",
    ] {
        if object
            .get(key)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(HostError::InvalidFrame(format!(
                "BUNDLE_INVALID: usage_total.{key} must be a non-negative integer"
            )));
        }
    }
    if let Some(cost) = object.get("cost") {
        let valid = cost
            .get("amount")
            .and_then(Value::as_f64)
            .is_some_and(|amount| amount >= 0.0)
            && cost
                .get("currency")
                .and_then(Value::as_str)
                .is_some_and(|currency| currency == "USD");
        if !valid {
            return Err(HostError::InvalidFrame(
                "BUNDLE_UNSUPPORTED: usage_total.cost must use USD".to_owned(),
            ));
        }
    }
    let extra = object
        .get("extra")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let Some(extra) = extra.as_object() else {
        return Err(HostError::InvalidFrame(
            "BUNDLE_INVALID: usage_total.extra must be an object".to_owned(),
        ));
    };
    if extra
        .keys()
        .any(|key| key != "requests" && key != "unpriced_requests")
        || ["reasoning_tokens", "tool_calls", "duration_ms"]
            .into_iter()
            .any(|key| object.get(key).and_then(Value::as_u64).unwrap_or(0) > 0)
    {
        return Err(HostError::InvalidFrame(
            "BUNDLE_UNSUPPORTED: usage_total has counters this host cannot preserve".to_owned(),
        ));
    }
    if ["requests", "unpriced_requests"]
        .into_iter()
        .any(|key| extra.get(key).is_some_and(|value| value.as_u64().is_none()))
    {
        return Err(HostError::InvalidFrame(
            "BUNDLE_INVALID: usage_total.extra counters must be non-negative integers".to_owned(),
        ));
    }
    Ok(())
}
