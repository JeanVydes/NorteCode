use ratatui::text::Line;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{SplitDiffComponent, UnifiedDiffComponent};
use crate::text_builder::TextBuilder;

const MAX_DIFF_CELLS: usize = 1_000_000;

pub(crate) struct EditDiffComponent;

impl EditDiffComponent {
    pub(crate) fn render(
        before: Option<&str>,
        arguments: Option<&Value>,
        mode: DiffMode,
        width: u16,
    ) -> Vec<Line<'static>> {
        let Some(before) = before else {
            return vec![TextBuilder::diff_unavailable()];
        };
        let Some(after) = Self::apply_edits(before, arguments) else {
            return vec![TextBuilder::diff_unavailable()];
        };
        let Some(rows) = DiffRows::new(before, &after) else {
            return vec![TextBuilder::diff_too_large()];
        };
        match mode {
            DiffMode::Unified => UnifiedDiffComponent::render(&rows),
            DiffMode::Split => SplitDiffComponent::render(&rows, width),
        }
    }

    fn apply_edits(source: &str, arguments: Option<&Value>) -> Option<String> {
        let edits = arguments?.get("edits")?.as_array()?;
        if edits.is_empty() {
            return None;
        }
        let mut content = source.to_owned();
        for edit in edits {
            let search = edit.get("search")?.as_str()?;
            let replace = edit.get("replace")?.as_str()?;
            if search.is_empty() {
                return None;
            }
            let matches = content.match_indices(search).collect::<Vec<_>>();
            let selected = match edit.get("occurrence").and_then(Value::as_u64) {
                Some(occurrence) => {
                    let index = usize::try_from(occurrence).ok()?.checked_sub(1)?;
                    *matches.get(index)?
                }
                None if matches.len() == 1 => *matches.first()?,
                None => return None,
            };
            let end = selected.0.checked_add(search.len())?;
            content.replace_range(selected.0..end, replace);
        }
        Some(content)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiffMode {
    #[default]
    Unified,
    Split,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DiffRow {
    Context(String),
    Change(String, String),
    Removed(String),
    Added(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DiffRows(pub(super) Vec<DiffRow>);

impl DiffRows {
    fn new(before: &str, after: &str) -> Option<Self> {
        let before = before.lines().collect::<Vec<_>>();
        let after = after.lines().collect::<Vec<_>>();
        let row_count = before.len().checked_add(1)?;
        let column_count = after.len().checked_add(1)?;
        if row_count.checked_mul(column_count)? > MAX_DIFF_CELLS {
            return None;
        }

        let mut lcs = vec![vec![0; column_count]; row_count];
        for old_index in (0..before.len()).rev() {
            for new_index in (0..after.len()).rev() {
                lcs[old_index][new_index] = if before[old_index] == after[new_index] {
                    lcs[old_index + 1][new_index + 1] + 1
                } else {
                    lcs[old_index + 1][new_index].max(lcs[old_index][new_index + 1])
                };
            }
        }

        let mut operations = Vec::new();
        let (mut old_index, mut new_index) = (0, 0);
        while old_index < before.len() || new_index < after.len() {
            if old_index < before.len()
                && new_index < after.len()
                && before[old_index] == after[new_index]
            {
                operations.push(DiffRow::Context(before[old_index].to_owned()));
                old_index += 1;
                new_index += 1;
            } else if new_index < after.len()
                && (old_index == before.len()
                    || lcs[old_index][new_index + 1] >= lcs[old_index + 1][new_index])
            {
                operations.push(DiffRow::Added(after[new_index].to_owned()));
                new_index += 1;
            } else {
                operations.push(DiffRow::Removed(before[old_index].to_owned()));
                old_index += 1;
            }
        }
        Some(Self(Self::pair_changes(&operations)))
    }

    fn pair_changes(operations: &[DiffRow]) -> Vec<DiffRow> {
        let mut rows = Vec::with_capacity(operations.len());
        let mut index = 0;
        while index < operations.len() {
            if let DiffRow::Context(_) = &operations[index] {
                rows.push(operations[index].clone());
                index += 1;
                continue;
            }

            let mut removed = Vec::new();
            let mut added = Vec::new();
            while index < operations.len() {
                match &operations[index] {
                    DiffRow::Removed(line) => removed.push(line.clone()),
                    DiffRow::Added(line) => added.push(line.clone()),
                    DiffRow::Context(_) | DiffRow::Change(_, _) => break,
                }
                index += 1;
            }
            let paired = removed.len().min(added.len());
            rows.extend(
                removed[..paired]
                    .iter()
                    .zip(&added[..paired])
                    .map(|(old, new)| DiffRow::Change(old.clone(), new.clone())),
            );
            rows.extend(removed[paired..].iter().cloned().map(DiffRow::Removed));
            rows.extend(added[paired..].iter().cloned().map(DiffRow::Added));
        }
        rows
    }
}

#[cfg(test)]
#[path = "edit_diff_tests.rs"]
mod tests;
