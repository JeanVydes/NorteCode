//! Shared Markdown plan metadata and parsing.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::AsyncReadExt;

/// The largest prefix of a plan file read to list plans without their bodies.
///
/// The front matter is small (status, title, timestamps, provider, model,
/// revision), so a bounded prefix is enough and a huge body costs nothing
/// to list. A file whose front matter does not fit in this prefix fails to
/// parse rather than being read in full.
pub const MAX_PLAN_HEADER_BYTES: u64 = 4096;

/// The lifecycle state of one plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
pub enum PlanStatus {
    /// The user approved this plan.
    Active,
    /// The plan is awaiting the user's decision.
    Pending,
    /// The user rejected or canceled this plan.
    Canceled,
}

/// A plan saved as a Markdown file in its session directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Stable plan identifier, also used as the Markdown filename.
    pub id: String,
    /// The plan text, editable by the user.
    pub content: String,
    /// Current approval state, stored in the file's front matter.
    pub status: PlanStatus,
    /// When the plan was first saved.
    pub created_at: Option<DateTime<Utc>>,
    /// When the plan was last changed.
    pub updated_at: Option<DateTime<Utc>>,
    /// Provider selected when the proposal was created.
    pub provider: Option<String>,
    /// Model selected when the proposal was created.
    pub model: Option<String>,
    /// Optional title from the front matter.
    pub title: Option<String>,
    /// Number of revisions after the initial proposal.
    pub revision: u32,
}

impl Plan {
    /// Creates a new pending plan before it is saved and approved.
    #[must_use]
    pub fn pending(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            status: PlanStatus::Pending,
            created_at: None,
            updated_at: None,
            provider: None,
            model: None,
            title: None,
            revision: 0,
        }
    }
}

/// Parses a complete plan document or a front matter-only prefix.
///
/// The closing delimiter is recognized as its own line. This keeps Markdown
/// horizontal rules in the body from being mistaken for the front matter
/// boundary, and lets callers parse a bounded prefix for list views.
///
/// # Errors
///
/// Returns an error when the document has no valid front matter, status, or
/// recognized metadata value.
pub fn parse_plan(id: &str, contents: &str) -> Result<Plan, String> {
    let rest = contents
        .strip_prefix("---\n")
        .ok_or_else(|| "missing YAML front matter".to_owned())?;
    let mut metadata_end = None;
    let mut body_start = None;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            metadata_end = Some(offset);
            body_start = Some(offset + line.len());
            break;
        }
        offset += line.len();
    }
    let (Some(metadata_end), Some(body_start)) = (metadata_end, body_start) else {
        return Err("unterminated YAML front matter".to_owned());
    };
    let metadata = rest.get(..metadata_end).unwrap_or_default();
    let content = rest
        .get(body_start..)
        .unwrap_or_default()
        .trim_start_matches(['\r', '\n']);
    let value = |key: &str| {
        metadata
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}: ")))
    };
    let status = match value("status").ok_or_else(|| "front matter has no `status`".to_owned())? {
        "active" => PlanStatus::Active,
        "pending" => PlanStatus::Pending,
        "canceled" => PlanStatus::Canceled,
        other => return Err(format!("unknown plan status `{other}`")),
    };
    let optional_datetime = |key: &str| -> Result<Option<DateTime<Utc>>, String> {
        value(key)
            .map(|raw| {
                DateTime::parse_from_rfc3339(raw)
                    .map(|timestamp| timestamp.with_timezone(&Utc))
                    .map_err(|error| format!("invalid `{key}` timestamp: {error}"))
            })
            .transpose()
    };
    let optional_string = |key: &str| -> Result<Option<String>, String> {
        value(key)
            .map(|raw| {
                serde_json::from_str(raw)
                    .or_else(|_| Ok::<_, serde_json::Error>(raw.to_owned()))
                    .map_err(|error| format!("invalid `{key}` value: {error}"))
            })
            .transpose()
    };
    let revision = value("revision")
        .map(|raw| {
            raw.parse::<u32>()
                .map_err(|error| format!("invalid `revision`: {error}"))
        })
        .transpose()?
        .unwrap_or(0);
    Ok(Plan {
        id: id.to_owned(),
        content: content.to_owned(),
        status,
        created_at: optional_datetime("created_at")?,
        updated_at: optional_datetime("updated_at")?,
        provider: optional_string("provider")?,
        model: optional_string("model")?,
        title: optional_string("title")?,
        revision,
    })
}

/// Parses a bounded UTF-8 prefix containing a plan's front matter.
///
/// # Errors
///
/// Returns an error when the bytes are not UTF-8 or the front matter is
/// invalid or incomplete.
pub fn parse_plan_header(id: &str, bytes: &[u8]) -> Result<Plan, String> {
    let contents = std::str::from_utf8(bytes).map_err(|_| "plan is not valid UTF-8".to_owned())?;
    parse_plan(id, contents)
}

/// Reads and parses only the bounded front matter of a plan file.
///
/// # Errors
///
/// Returns an error when the file cannot be opened/read, is not UTF-8, or
/// contains invalid or incomplete front matter.
pub async fn read_plan_header(path: impl AsRef<Path>, id: &str) -> Result<Plan, String> {
    let path = path.as_ref();
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_PLAN_HEADER_BYTES)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    parse_plan_header(id, &bytes)
        .map_err(|error| format!("invalid Markdown plan {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_horizontal_rule_does_not_close_front_matter() {
        let plan = parse_plan(
            "one",
            "---\nstatus: pending\n---\n# Plan\n\n---\n\ncontinued",
        )
        .expect("valid plan");
        assert_eq!(plan.content, "# Plan\n\n---\n\ncontinued");
    }

    #[test]
    fn metadata_prefix_is_enough_for_listing() {
        let plan = parse_plan(
            "one",
            "---\nstatus: active\nupdated_at: 2026-11-04T00:00:00Z\n---",
        )
        .expect("valid metadata");
        assert_eq!(plan.status, PlanStatus::Active);
        assert_eq!(plan.content, "");
    }
}
