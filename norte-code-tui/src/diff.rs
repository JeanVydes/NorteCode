//! The diff an edit made, in whichever layout the user prefers.

use serde::Deserialize;

/// The arguments an `edit` call carries, decoded at the wire boundary.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct EditArgs {
    /// The file the call touched.
    pub(crate) path: Option<String>,
    /// The replacements to apply.
    pub(crate) edits: Option<Vec<EditReplacement>>,
    /// A direct replacement, when the call did not use `edits`.
    pub(crate) old_string: Option<String>,
    /// The text replacing `old_string`.
    pub(crate) new_string: Option<String>,
}

/// One replacement inside an `edit` call.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct EditReplacement {
    /// The text before the change.
    pub(crate) search: Option<String>,
    /// The text after the change.
    pub(crate) replace: Option<String>,
}

/// How a code change is drawn.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum DiffMode {
    /// Changed lines one after another, with context.
    #[default]
    Unified,
    /// Removed and added lines side by side, with context.
    Split,
}

impl DiffMode {
    /// The label shown in the config page.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Unified => "unified",
            Self::Split => "split",
        }
    }

    /// The next mode in the cycle.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Unified => Self::Split,
            Self::Split => Self::Unified,
        }
    }
}

/// What one row of the diff contains.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) enum LineKind {
    /// A line around the change.
    Context,
    /// A line before the edit.
    Removed,
    /// A line after the edit.
    Added,
}

/// One row of the diff.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct Line {
    /// Whether this line is context, removed or added.
    pub(crate) kind: LineKind,
    /// The text of the line.
    pub(crate) text: String,
}

/// The change an edit made, with context when the host sent the source.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(crate) struct Diff {
    /// Lines in display order.
    pub(crate) lines: Vec<Line>,
}

const CONTEXT: usize = 3;
const MAX_LINES: usize = 400;

impl Diff {
    /// Build the diff from the decoded call and the file snapshot.
    pub(crate) fn from_call(source: Option<&str>, arguments: &EditArgs) -> Option<Self> {
        if let Some(source) = source {
            let mut after = source.to_owned();
            for edit in arguments.edits.iter().flatten() {
                if let (Some(search), Some(replace)) =
                    (edit.search.as_deref(), edit.replace.as_deref())
                {
                    after = after.replacen(search, replace, 1);
                }
            }
            if arguments.edits.is_none()
                && let (Some(old), Some(new)) = (
                    arguments.old_string.as_deref(),
                    arguments.new_string.as_deref(),
                )
            {
                after = after.replacen(old, new, 1);
            }
            if after != source {
                return Self::bounded(Self::between(source, &after).lines);
            }
        }
        let mut lines = Vec::new();
        for edit in arguments.edits.iter().flatten() {
            if let Some(old) = edit.search.as_deref() {
                lines.extend(old.lines().map(|line| Line {
                    kind: LineKind::Removed,
                    text: line.to_owned(),
                }));
            }
            if let Some(new) = edit.replace.as_deref() {
                lines.extend(new.lines().map(|line| Line {
                    kind: LineKind::Added,
                    text: line.to_owned(),
                }));
            }
        }
        if lines.is_empty() {
            if let Some(old) = arguments.old_string.as_deref() {
                lines.extend(old.lines().map(|line| Line {
                    kind: LineKind::Removed,
                    text: line.to_owned(),
                }));
            }
            if let Some(new) = arguments.new_string.as_deref() {
                lines.extend(new.lines().map(|line| Line {
                    kind: LineKind::Added,
                    text: line.to_owned(),
                }));
            }
        }
        Self::bounded(lines)
    }

    fn bounded(mut lines: Vec<Line>) -> Option<Self> {
        if lines.is_empty() {
            return None;
        }
        if lines.len() > MAX_LINES {
            lines.truncate(MAX_LINES - 1);
            lines.push(Line {
                kind: LineKind::Context,
                text: "... diff truncated".to_owned(),
            });
        }
        Some(Self { lines })
    }

    /// The changed lines and their surroundings.
    fn between(before: &str, after: &str) -> Self {
        let before: Vec<&str> = before.lines().collect();
        let after: Vec<&str> = after.lines().collect();
        let mut prefix = 0usize;
        while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
            prefix += 1;
        }
        let mut suffix = 0usize;
        while suffix < before.len() - prefix
            && suffix < after.len() - prefix
            && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
        {
            suffix += 1;
        }
        let context_before = prefix.saturating_sub(CONTEXT);
        let mut lines = Vec::new();
        for line in &before[context_before..prefix] {
            lines.push(Line {
                kind: LineKind::Context,
                text: (*line).to_owned(),
            });
        }
        for line in &before[prefix..before.len() - suffix] {
            lines.push(Line {
                kind: LineKind::Removed,
                text: (*line).to_owned(),
            });
        }
        for line in &after[prefix..after.len() - suffix] {
            lines.push(Line {
                kind: LineKind::Added,
                text: (*line).to_owned(),
            });
        }
        let tail_start = before.len() - suffix;
        for line in &before[tail_start..(tail_start + CONTEXT).min(before.len())] {
            lines.push(Line {
                kind: LineKind::Context,
                text: (*line).to_owned(),
            });
        }
        Self { lines }
    }

    /// Rows needed inside the code box for a layout.
    pub(crate) fn rows(&self, mode: DiffMode) -> usize {
        match mode {
            DiffMode::Unified => self.lines.len(),
            DiffMode::Split => {
                let mut rows = 0usize;
                let mut index = 0usize;
                while index < self.lines.len() {
                    if self.lines[index].kind == LineKind::Context {
                        rows += 1;
                        index += 1;
                    } else {
                        let start = index;
                        while index < self.lines.len()
                            && self.lines[index].kind != LineKind::Context
                        {
                            index += 1;
                        }
                        let removed = self.lines[start..index]
                            .iter()
                            .filter(|line| line.kind == LineKind::Removed)
                            .count();
                        let added = self.lines[start..index]
                            .iter()
                            .filter(|line| line.kind == LineKind::Added)
                            .count();
                        rows += removed.max(added);
                    }
                }
                rows
            }
        }
    }

    /// The count shown in the box title.
    pub(crate) fn counts(&self) -> (usize, usize) {
        (
            self.lines
                .iter()
                .filter(|line| line.kind == LineKind::Removed)
                .count(),
            self.lines
                .iter()
                .filter(|line| line.kind == LineKind::Added)
                .count(),
        )
    }
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
