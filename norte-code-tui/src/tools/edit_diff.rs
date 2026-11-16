//! The change an `edit` made, drawn as a code box.

use ratatui::text::Line;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::code_box::{CodeBox, CodeBoxRenderer, CodeRow};
use super::split_diff::SplitDiffComponent;
use crate::ui_model::ThemeMode;

const MAX_DIFF_CELLS: usize = 1_000_000;

pub(crate) struct EditDiffComponent;

impl EditDiffComponent {
    /// Draws what an `edit` changed as a code box.
    ///
    /// The host snapshots the file just before the edit, so the box can show
    /// the real line numbers and the lines around the change. When no
    /// snapshot reached the client — a call replayed after a reload carries
    /// none — the box is drawn from the edits themselves: the search is the
    /// removed line, the replacement the added one, and the gutter carries a
    /// sign instead of a number rather than a number the file never gave us.
    pub(crate) fn render(
        before: Option<&str>,
        arguments: Option<&Value>,
        mode: DiffMode,
        width: u16,
        theme: ThemeMode,
    ) -> Vec<Line<'static>> {
        let title = Self::title(arguments);
        if let Some(before) = before
            && let Some(rows) = Self::rows(before, arguments, mode)
        {
            return match mode {
                DiffMode::Unified => Self::unified(title, &rows, width, theme),
                DiffMode::Split => SplitDiffComponent::render(&rows, width, theme),
            };
        }
        if let Some(arguments) = arguments {
            let rows = cut(&Self::edit_rows(arguments));
            if !rows.is_empty() {
                return Self::unified(title, &rows, width, theme);
            }
        }
        Self::unified(
            title,
            &[CodeRow::context(
                None,
                "edit diff unavailable: the host sent no source to compare",
            )],
            width,
            theme,
        )
    }

    fn unified(title: &str, rows: &[CodeRow], width: u16, theme: ThemeMode) -> Vec<Line<'static>> {
        CodeBoxRenderer::render(&CodeBox { title, rows }, width, theme)
    }

    /// The name the box carries: the file the edit touched.
    fn title(arguments: Option<&Value>) -> &str {
        arguments
            .and_then(|arguments| arguments.get("path"))
            .and_then(Value::as_str)
            .unwrap_or("edit")
    }

    /// The rows the captured file and the edit produce, as a code box draws
    /// them.
    ///
    /// A changed line is two rows under one number: the line that was there
    /// and the line that replaced it. The rows are numbered against the full
    /// diff first and cut to the visible window afterwards, so the number on a
    /// row is the file's, not the window's.
    fn rows(before: &str, arguments: Option<&Value>, mode: DiffMode) -> Option<Vec<CodeRow>> {
        let after = Self::apply_edits(before, arguments)?;
        let rows = DiffRows::new(before, &after)?.0;
        let numbers = numbered(&rows);
        let numbered_rows = rows
            .iter()
            .zip(numbers)
            .flat_map(|(row, number)| Self::code_row(row, number))
            .collect::<Vec<_>>();
        Some(if mode == DiffMode::Split {
            numbered_rows
        } else {
            cut(&numbered_rows)
        })
    }

    /// The rows a call carries when the file never reached the client.
    fn edit_rows(arguments: &Value) -> Vec<CodeRow> {
        let Some(edits) = arguments.get("edits").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (index, edit) in edits.iter().enumerate() {
            let (Some(search), Some(replace)) = (
                edit.get("search").and_then(Value::as_str),
                edit.get("replace").and_then(Value::as_str),
            ) else {
                continue;
            };
            if index > 0 {
                rows.push(CodeRow::Gap);
            }
            for line in search.lines() {
                rows.push(CodeRow::removed(None, line));
            }
            for line in replace.lines() {
                rows.push(CodeRow::added(None, line));
            }
        }
        rows
    }

    /// One diff row, as one or two code rows.
    fn code_row(row: &DiffRow, number: Option<usize>) -> Vec<CodeRow> {
        match row {
            DiffRow::Context(text) => vec![CodeRow::context(number, text.clone())],
            DiffRow::Change(before, after) => {
                let mut rows = vec![CodeRow::removed(number, before.clone())];
                for (offset, line) in after.lines().enumerate() {
                    rows.push(CodeRow::added(number.map(|number| number + offset), line));
                }
                if after.is_empty() {
                    rows.push(CodeRow::added(number, ""));
                }
                rows
            }
            DiffRow::Removed(text) => vec![CodeRow::removed(number, text.clone())],
            DiffRow::Added(text) => vec![CodeRow::added(number, text.clone())],
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

/// How many unchanged lines a box keeps around a change.
const CONTEXT_LINES: usize = 3;

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

/// The file line numbers the rows sit on.
///
/// A removed line keeps the number it had in the file before the edit; an
/// added line carries its number in the file after it.
fn numbered(rows: &[DiffRow]) -> Vec<Option<usize>> {
    let mut before = 1_usize;
    let mut after = 1_usize;
    let mut numbers = Vec::with_capacity(rows.len());
    for row in rows {
        match row {
            DiffRow::Context(_) | DiffRow::Change(_, _) => {
                numbers.push(Some(before));
                before += 1;
                after += 1;
            }
            DiffRow::Removed(_) => {
                numbers.push(Some(before));
                before += 1;
            }
            DiffRow::Added(_) => {
                numbers.push(Some(after));
                after += 1;
            }
        }
    }
    numbers
}

/// The rows a box draws: the changes and the lines around them.
///
/// A whole file is rarely what the reader needs, so anything further than
/// `CONTEXT_LINES` from a change is left out and marked with a gap.
fn cut(rows: &[CodeRow]) -> Vec<CodeRow> {
    let changed: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| !matches!(row, CodeRow::Context { .. }))
        .map(|(index, _)| index)
        .collect();
    let mut keep = vec![false; rows.len()];
    for index in &changed {
        let start = index.saturating_sub(CONTEXT_LINES);
        let end = (index + CONTEXT_LINES + 1).min(rows.len());
        keep[start..end].fill(true);
    }
    if changed.is_empty() && !rows.is_empty() {
        keep[0] = true;
    }

    let mut drawn = Vec::new();
    let mut skipped = false;
    for (row, kept) in rows.iter().zip(keep) {
        if !kept {
            skipped = true;
            continue;
        }
        if skipped {
            skipped = false;
            drawn.push(CodeRow::Gap);
        }
        drawn.push(row.clone());
    }
    drawn
}

#[cfg(test)]
#[path = "edit_diff_tests.rs"]
mod tests;
