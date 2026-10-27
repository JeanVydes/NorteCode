//! The side-by-side preview of an `edit` call.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::Style;

/// A search/replace edit rendered as a side-by-side, boxed preview.
#[derive(Debug)]
pub(crate) struct EditPreview {
    pub(crate) path: String,
    pub(crate) boxes: Vec<EditBox>,
}

/// One edit's box: the lines it touched plus surrounding context.
#[derive(Debug)]
pub(crate) struct EditBox {
    pub(crate) index: usize,
    pub(crate) total: usize,
    pub(crate) old_range: Option<(usize, usize)>,
    pub(crate) new_range: Option<(usize, usize)>,
    pub(crate) approximate: bool,
    pub(crate) rows: Vec<DiffRow>,
    pub(crate) omitted: usize,
}

/// One row of the side-by-side view. A missing side is a deletion or insertion.
#[derive(Debug)]
pub(crate) struct DiffRow {
    pub(crate) left: Option<Cell>,
    pub(crate) right: Option<Cell>,
}

/// One numbered line in a panel.
#[derive(Debug)]
pub(crate) struct Cell {
    pub(crate) number: Option<usize>,
    pub(crate) text: String,
    pub(crate) changed: bool,
}

/// Which panel a cell belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    /// The file before the edit.
    Left,
    /// The file after the edit.
    Right,
}

/// Lines of context kept before and after an edit.
const EDIT_CONTEXT_LINES: usize = 2;
/// Rows kept per box, so a huge search block cannot flood the terminal.
const EDIT_MAX_ROWS: usize = 40;
/// The largest file read for a preview. The edit tool itself is bounded too.
const EDIT_MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Builds the preview shown for an `edit` call from its search/replace blocks.
///
/// The tool's structured result records where an edit landed, not the text it
/// replaced, so the request and the file before the edit are the only sources.
/// The host captures the file in the tool metadata callback, immediately before
/// the executor runs; reading it here would race with a fast edit.
pub(crate) fn edit_preview(
    arguments: Option<&serde_json::Value>,
    source: Option<&str>,
) -> Option<EditPreview> {
    let arguments = arguments?;
    let path = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("(unknown path)")
        .to_owned();
    let edits = arguments.get("edits")?.as_array()?;
    if edits.is_empty() {
        return None;
    }
    let source = source
        .map(str::to_owned)
        .or_else(|| read_edit_source(&path));
    Some(build_edit_preview(path, edits, source))
}

pub(crate) fn build_edit_preview(
    path: String,
    edits: &[serde_json::Value],
    source: Option<String>,
) -> EditPreview {
    let mut working = source;
    let total = edits.len();
    let mut boxes = Vec::with_capacity(total);
    for (index, edit) in edits.iter().enumerate() {
        let search = edit
            .get("search")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let replace = edit
            .get("replace")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let changed = search != replace;
        let built = working.as_mut().and_then(|source| {
            let found = find_edit(source, search)?;
            let after = if found.approximate {
                // The whitespace fallback matched whole lines, including the
                // last newline. Keep the block boundary intact when the
                // replacement does not carry its own trailing newline.
                let matched = &source[found.start..found.end];
                let mut replacement = replace.to_owned();
                if matched.ends_with('\n') && !replacement.ends_with('\n') {
                    replacement.push('\n');
                } else if !matched.ends_with('\n') && replacement.ends_with('\n') {
                    replacement.pop();
                }
                format!(
                    "{}{}{}",
                    &source[..found.start],
                    replacement,
                    &source[found.end..]
                )
            } else {
                format!(
                    "{}{}{}",
                    &source[..found.start],
                    replace,
                    &source[found.end..]
                )
            };
            let built = build_edit_box(
                source,
                &after,
                found.start,
                found.end,
                replace,
                found.approximate,
                changed,
            );
            if !found.approximate {
                *source = after;
            }
            Some(built)
        });
        let (rows, old_range, new_range, approximate, omitted) =
            built.unwrap_or_else(|| fallback_edit_box(search, replace, changed));
        boxes.push(EditBox {
            index: index + 1,
            total,
            rows,
            old_range,
            new_range,
            approximate,
            omitted,
        });
    }
    EditPreview { path, boxes }
}

#[derive(Clone, Copy, Debug)]
struct Found {
    start: usize,
    end: usize,
    approximate: bool,
}

/// Finds the block an edit applies to. Exact matches win; a whitespace-only
/// mismatch is still located for the context preview, but not simulated.
fn find_edit(source: &str, search: &str) -> Option<Found> {
    if search.is_empty() {
        return None;
    }
    if let Some(start) = source.find(search) {
        return Some(Found {
            start,
            end: start + search.len(),
            approximate: false,
        });
    }
    let (start, end) = find_line_window(source, search)?;
    Some(Found {
        start,
        end,
        approximate: true,
    })
}

/// Finds a window of whole source lines that matches the search block after
/// trimming each line. This mirrors the edit tool's whitespace-only fallback
/// closely enough for a preview.
fn find_line_window(source: &str, search: &str) -> Option<(usize, usize)> {
    if search.trim().is_empty() {
        return None;
    }
    let needle: Vec<&str> = search.lines().map(str::trim).collect();
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    if needle.is_empty() || needle.len() > lines.len() {
        return None;
    }
    let mut offsets = Vec::with_capacity(lines.len() + 1);
    let mut offset = 0usize;
    for line in &lines {
        offsets.push(offset);
        offset += line.len();
    }
    offsets.push(offset);
    for start in 0..=lines.len() - needle.len() {
        if needle
            .iter()
            .enumerate()
            .all(|(index, needle)| lines[start + index].trim() == *needle)
        {
            return Some((offsets[start], offsets[start + needle.len()]));
        }
    }
    None
}

#[allow(clippy::type_complexity)]
fn build_edit_box(
    source: &str,
    after: &str,
    start: usize,
    end: usize,
    replace: &str,
    approximate: bool,
    changed: bool,
) -> (
    Vec<DiffRow>,
    Option<(usize, usize)>,
    Option<(usize, usize)>,
    bool,
    usize,
) {
    let old_start = line_index(source, start);
    let old_end = line_index(source, end.saturating_sub(1));
    let new_end = if replace.is_empty() {
        old_start.saturating_sub(1)
    } else {
        line_index(after, start + replace.len().saturating_sub(1))
    };
    let old_lines: Vec<&str> = source.split('\n').collect();
    let new_lines: Vec<&str> = after.split('\n').collect();
    let mut rows = Vec::new();

    let context_start = old_start.saturating_sub(EDIT_CONTEXT_LINES);
    for line in context_start..old_start {
        if let (Some(left), Some(right)) = (old_lines.get(line), new_lines.get(line)) {
            rows.push(DiffRow {
                left: Some(cell(line + 1, left, false)),
                right: Some(cell(line + 1, right, false)),
            });
        }
    }

    let old_count = old_end.saturating_sub(old_start) + 1;
    let new_count = if replace.is_empty() {
        0
    } else {
        new_end.saturating_sub(old_start) + 1
    };
    for offset in 0..old_count.max(new_count) {
        let left = if offset < old_count {
            old_lines
                .get(old_start + offset)
                .map(|text| cell(old_start + offset + 1, text, changed))
        } else {
            None
        };
        let right = if offset < new_count {
            new_lines
                .get(old_start + offset)
                .map(|text| cell(old_start + offset + 1, text, changed))
        } else {
            None
        };
        rows.push(DiffRow { left, right });
    }

    let context_end = (old_end + EDIT_CONTEXT_LINES).min(old_lines.len().saturating_sub(1));
    let mut new_context = if replace.is_empty() {
        old_start
    } else {
        new_end.saturating_add(1)
    };
    for line in (old_end + 1)..=context_end {
        let left = old_lines.get(line).map(|text| cell(line + 1, text, false));
        let right = new_lines
            .get(new_context)
            .map(|text| cell(new_context + 1, text, false));
        new_context = new_context.saturating_add(1);
        rows.push(DiffRow { left, right });
    }

    let omitted = rows.len().saturating_sub(EDIT_MAX_ROWS);
    if omitted > 0 {
        rows.truncate(EDIT_MAX_ROWS);
    }
    let new_range = if replace.is_empty() {
        None
    } else {
        Some((old_start + 1, new_end + 1))
    };
    (
        rows,
        Some((old_start + 1, old_end + 1)),
        new_range,
        approximate,
        omitted,
    )
}

#[allow(clippy::type_complexity)]
fn fallback_edit_box(
    search: &str,
    replace: &str,
    changed: bool,
) -> (
    Vec<DiffRow>,
    Option<(usize, usize)>,
    Option<(usize, usize)>,
    bool,
    usize,
) {
    let left: Vec<&str> = if search.is_empty() {
        Vec::new()
    } else {
        search.lines().collect()
    };
    let right: Vec<&str> = if replace.is_empty() {
        Vec::new()
    } else {
        replace.lines().collect()
    };
    let count = left.len().max(right.len()).max(1);
    let mut rows = Vec::new();
    for index in 0..count {
        let left = left.get(index).map(|text| Cell {
            number: None,
            text: display_line(text),
            changed,
        });
        let right = right.get(index).map(|text| Cell {
            number: None,
            text: display_line(text),
            changed,
        });
        rows.push(DiffRow { left, right });
    }
    let omitted = rows.len().saturating_sub(EDIT_MAX_ROWS);
    if omitted > 0 {
        rows.truncate(EDIT_MAX_ROWS);
    }
    (rows, None, None, false, omitted)
}

/// Reads the file an edit will touch, but only when it is a regular file inside
/// the current workspace and no larger than the preview limit. It may see a
/// path the model chose, so it must never read an arbitrary file.
fn read_edit_source(path: &str) -> Option<String> {
    let root = std::env::current_dir().ok()?.canonicalize().ok()?;
    let candidate = Path::new(path);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let candidate = candidate.canonicalize().ok()?;
    if !candidate.starts_with(&root) {
        return None;
    }
    // Check before opening: opening a FIFO can block, and the edit tool would
    // reject it anyway.
    let metadata = std::fs::metadata(&candidate).ok()?;
    if !metadata.is_file() || metadata.len() > EDIT_MAX_FILE_BYTES {
        return None;
    }
    let file = std::fs::File::open(candidate).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > EDIT_MAX_FILE_BYTES {
        return None;
    }
    let mut text = String::new();
    file.take(EDIT_MAX_FILE_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn line_index(text: &str, byte: usize) -> usize {
    text.as_bytes()
        .iter()
        .take(byte)
        .filter(|byte| **byte == b'\n')
        .count()
}

fn cell(number: usize, text: &str, changed: bool) -> Cell {
    Cell {
        number: Some(number),
        text: display_line(text),
        changed,
    }
}

fn display_line(text: &str) -> String {
    text.trim_end_matches('\r').replace('\t', "    ")
}

pub(crate) fn render_panel(
    cell: Option<&Cell>,
    side: Side,
    panel: usize,
    digits: usize,
    style: Style,
) -> String {
    let code_width = panel.saturating_sub(digits + 3).max(1);
    let (number, text, changed) = match cell {
        Some(cell) => (
            cell.number
                .map_or_else(|| " ".repeat(digits), |number| format!("{number:>digits$}")),
            clip_plain(&cell.text, code_width),
            cell.changed,
        ),
        None => (" ".repeat(digits), String::new(), false),
    };
    let sign = match (changed, side) {
        (true, Side::Left) => "-",
        (true, Side::Right) => "+",
        _ => " ",
    };
    let padded = format!("{text:<code_width$}");
    let code = match (changed, side) {
        (true, Side::Left) => style.red(padded),
        (true, Side::Right) => style.green(padded),
        _ => padded,
    };
    let number = style.faint(number);
    let sign = match (changed, side) {
        (true, Side::Left) => style.red(sign),
        (true, Side::Right) => style.green(sign),
        _ => style.faint(sign),
    };
    format!("{number} {sign} {code}")
}

/// One row of the unified view: the line, and what happened to it.
///
/// The side-by-side view keeps two cells per row because it shows two files.
/// This one shows one line at a time, so what it needs beside the text is the
/// mark a patch puts there — and the number of the line in the file the row
/// belongs to, which is the old file for a removal and the new one otherwise.
#[derive(Debug)]
pub(crate) struct UnifiedRow {
    /// Where the line stands: unchanged, gone, or new.
    pub(crate) kind: UnifiedKind,
    /// The number in the file it belongs to, when it has one.
    pub(crate) number: Option<usize>,
    /// The text, with tabs already expanded for the terminal.
    pub(crate) text: String,
}

/// What happened to a line in the unified view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnifiedKind {
    /// It is in both files.
    Context,
    /// It was in the old file only.
    Removed,
    /// It is in the new file only.
    Added,
}

impl UnifiedKind {
    /// The character a patch puts in the first column.
    pub(crate) fn mark(self) -> char {
        match self {
            Self::Context => ' ',
            Self::Removed => '-',
            Self::Added => '+',
        }
    }
}

/// Folds a side-by-side box into the unified view.
///
/// Both come from the same rows, so a removal and its replacement keep the
/// order they had: what the left panel held, then what the right one holds. The
/// side a panel left empty is the row the other panel added or removed, which
/// is exactly what a patch's `-` and `+` lines are.
///
/// It is a fold rather than a diff of its own on purpose: the boxes already
/// decided which lines changed and which are context, and a second diff would
/// be a second answer to the same question.
pub(crate) fn unify(rows: &[DiffRow]) -> Vec<UnifiedRow> {
    let mut unified = Vec::with_capacity(rows.len());
    for row in rows {
        match (&row.left, &row.right) {
            (Some(left), Some(right)) => {
                // Both sides hold a line. An unchanged row is the same line in
                // both files and is shown once; a changed one is shown twice —
                // the old above the new — so the reader sees what was there and
                // what is there now.
                if left.changed || right.changed {
                    unified.push(UnifiedRow {
                        kind: UnifiedKind::Removed,
                        number: left.number,
                        text: left.text.clone(),
                    });
                    unified.push(UnifiedRow {
                        kind: UnifiedKind::Added,
                        number: right.number,
                        text: right.text.clone(),
                    });
                } else {
                    unified.push(UnifiedRow {
                        kind: UnifiedKind::Context,
                        number: right.number.or(left.number),
                        text: right.text.clone(),
                    });
                }
            }
            (Some(left), None) => unified.push(UnifiedRow {
                kind: UnifiedKind::Removed,
                number: left.number,
                text: left.text.clone(),
            }),
            (None, Some(right)) => unified.push(UnifiedRow {
                kind: UnifiedKind::Added,
                number: right.number,
                text: right.text.clone(),
            }),
            (None, None) => {}
        }
    }
    unified
}

/// The line range a box covers, in the words both views use.
///
/// The two shapes answer the same question in their header, and that header was
/// written twice while this sat inline in one of them: a reader who switches
/// styles should not have to learn a second set of words for the same fact.
pub(crate) fn edit_range(edit: &EditBox) -> String {
    match (edit.old_range, edit.new_range) {
        (Some((start, end)), Some((new_start, new_end)))
            if (start, end) == (new_start, new_end) =>
        {
            line_range(start, end)
        }
        (Some((start, end)), Some((new_start, new_end))) => {
            format!(
                "{} → {}",
                line_range(start, end),
                line_range(new_start, new_end)
            )
        }
        (Some((start, end)), None) => format!("{} → deleted", line_range(start, end)),
        (None, _) => "location unavailable".to_owned(),
    }
}

pub(crate) fn clip_plain(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let escaped = crate::text::terminal(text);
    let escaped = escaped.replace('\t', "    ");
    if escaped.chars().count() <= max {
        return escaped;
    }
    let mut out: String = escaped.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub(crate) fn line_range(start: usize, end: usize) -> String {
    if start == end {
        format!("line {start}")
    } else {
        format!("lines {start}–{end}")
    }
}

/// Shows a path relative to the workspace when it is inside it, so the header
/// keeps the line range instead of spending its width on a long absolute path.
pub(crate) fn preview_path(path: &str) -> String {
    let root = std::env::current_dir()
        .ok()
        .and_then(|root| root.canonicalize().ok());
    let shown = Path::new(path).canonicalize().ok();
    let displayed = match (root, shown) {
        (Some(root), Some(shown)) => match shown.strip_prefix(&root) {
            Ok(relative) => relative.to_path_buf(),
            Err(_) => shown,
        },
        _ => PathBuf::from(path),
    };
    displayed.display().to_string().replace(['\n', '\r'], " ")
}

pub(crate) fn edit_box_width() -> usize {
    let columns = crossterm::terminal::size().map_or(100, |(columns, _)| usize::from(columns));
    let available = columns.saturating_sub(2);
    if available < 30 {
        available.max(20)
    } else {
        available.min(140)
    }
}

/// Shortens a command to one display line without splitting a character.
pub(crate) fn clip_command(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or_default().trim();
    let mut shortened: String = first.chars().take(max).collect();
    if first.chars().count() > max {
        shortened.push('…');
    } else if text.lines().count() > 1 {
        shortened.push_str(" …");
    }
    if shortened.is_empty() {
        "(empty)".to_owned()
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell with a number and text, changed or not.
    fn cell_at(number: usize, text: &str, changed: bool) -> Cell {
        Cell {
            number: Some(number),
            text: text.to_owned(),
            changed,
        }
    }

    #[test]
    fn a_replacement_reads_as_a_removal_then_an_addition() {
        // The order is what a patch does: what was there, then what is. A
        // unified view that put the addition first would read backwards.
        let rows = vec![DiffRow {
            left: Some(cell_at(4, "old line", true)),
            right: Some(cell_at(4, "new line", true)),
        }];
        let unified = unify(&rows);
        assert_eq!(unified.len(), 2, "a changed line is two rows: {unified:?}");
        assert_eq!(unified[0].kind, UnifiedKind::Removed);
        assert_eq!(unified[0].text, "old line");
        assert_eq!(unified[0].number, Some(4), "numbered by the old file");
        assert_eq!(unified[1].kind, UnifiedKind::Added);
        assert_eq!(unified[1].text, "new line");
    }

    #[test]
    fn an_unchanged_line_is_shown_once_and_only_one_row() {
        // Context is one line, not two: drawing it on both sides is what the
        // side-by-side view does and what this one exists not to do.
        let rows = vec![DiffRow {
            left: Some(cell_at(3, "same", false)),
            right: Some(cell_at(3, "same", false)),
        }];
        let unified = unify(&rows);
        assert_eq!(unified.len(), 1, "{unified:?}");
        assert_eq!(unified[0].kind, UnifiedKind::Context);
        assert_eq!(unified[0].text, "same");
    }

    #[test]
    fn a_side_that_is_missing_is_an_insertion_or_a_deletion() {
        // A pure deletion has no right cell and a pure insertion has no left
        // one; neither is a change of a line, and both must still be shown.
        let rows = vec![
            DiffRow {
                left: Some(cell_at(7, "gone", true)),
                right: None,
            },
            DiffRow {
                left: None,
                right: Some(cell_at(7, "arrived", true)),
            },
        ];
        let unified = unify(&rows);
        assert_eq!(unified.len(), 2, "{unified:?}");
        assert_eq!(unified[0].kind, UnifiedKind::Removed);
        assert_eq!(unified[0].text, "gone");
        assert_eq!(unified[1].kind, UnifiedKind::Added);
        assert_eq!(unified[1].text, "arrived");
    }

    #[test]
    fn the_marks_are_the_ones_a_patch_uses() {
        // A diff a person has read before should need no explanation, so the
        // marks are the two every tool already uses.
        assert_eq!(UnifiedKind::Context.mark(), ' ');
        assert_eq!(UnifiedKind::Removed.mark(), '-');
        assert_eq!(UnifiedKind::Added.mark(), '+');
    }

    #[test]
    fn an_empty_box_unifies_to_nothing() {
        // A box with no rows is not one row of blank: the call is what draws the
        // borders, and this must not invent a line for it.
        assert_eq!(unify(&[]).len(), 0);
    }

    #[test]
    fn the_range_says_what_both_views_say() {
        let boxed = |old: Option<(usize, usize)>, new: Option<(usize, usize)>| EditBox {
            index: 1,
            total: 1,
            old_range: old,
            new_range: new,
            approximate: false,
            rows: Vec::new(),
            omitted: 0,
        };
        // A line replaced by another line: the old range and the new one.
        assert!(edit_range(&boxed(Some((3, 3)), Some((3, 3)))).contains('3'));
        // A deletion: there is no new range.
        let deleted = edit_range(&boxed(Some((3, 5)), None));
        assert!(deleted.contains("deleted"), "{deleted}");
        // An insertion has no old range, which is what "unavailable" covers.
        assert_eq!(edit_range(&boxed(None, None)), "location unavailable");
    }
}
