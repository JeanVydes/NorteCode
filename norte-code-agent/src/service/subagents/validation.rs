//! Checking a child's answer against the schema it was asked for.

/// Whether a JSON Schema validates `output` (RFC 8.2.6).
///
/// The schema is trusted input from the model, not from a file: a schema
/// that does not compile is not an invalid output, it is an invalid ask,
/// and the tool says so rather than marking the child failed. Validation
/// is a tiny subset: `type`, `required`, `properties`, `items`, `enum` -
/// enough for the schemas the registry carries, without a new dependency.
pub(crate) fn validate_output_schema(schema: &serde_json::Value, output: &str) -> bool {
    let value = serde_json::from_str::<serde_json::Value>(output)
        .unwrap_or(serde_json::Value::String(output.to_owned()));
    schema_valid(schema, &value)
}

fn schema_valid(schema: &serde_json::Value, value: &serde_json::Value) -> bool {
    let Some(object) = schema.as_object() else {
        return true;
    };
    if let Some(kind) = object.get("type").and_then(serde_json::Value::as_str) {
        let ok = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.is_i64() || value.is_u64(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    if let Some(values) = object.get("enum").and_then(serde_json::Value::as_array)
        && !values.iter().any(|candidate| candidate == value)
    {
        return false;
    }
    if let Some(required) = object.get("required").and_then(serde_json::Value::as_array)
        && let Some(fields) = value.as_object()
    {
        for field in required {
            let Some(name) = field.as_str() else {
                continue;
            };
            if !fields.contains_key(name) {
                return false;
            }
        }
    }
    if let Some(properties) = object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        && let Some(fields) = value.as_object()
    {
        for (name, subschema) in properties {
            if let Some(field) = fields.get(name)
                && !schema_valid(subschema, field)
            {
                return false;
            }
        }
    }
    if let Some(items) = object.get("items")
        && let Some(list) = value.as_array()
    {
        for item in list {
            if !schema_valid(items, item) {
                return false;
            }
        }
    }
    true
}

/// Clips `text` to `max` words, for a summary the stream carries.
/// Clips `text` to `max` words, for a summary the stream carries.
pub(crate) fn clip(text: &str, max: usize) -> String {
    let clipped: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        format!("{clipped}…")
    } else {
        clipped
    }
}
