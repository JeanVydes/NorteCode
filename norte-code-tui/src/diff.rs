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
    /// Removed lines above added lines.
    #[default]
    Unified,
    /// Removed and added lines side by side.
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

/// The lines an edit removed and added.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(crate) struct Diff {
    /// Lines that were there before.
    pub(crate) removed: Vec<String>,
    /// Lines that are there now.
    pub(crate) added: Vec<String>,
}

impl Diff {
    /// Build the diff from decoded edit arguments, when they carry one.
    pub(crate) fn from_edit(arguments: &EditArgs) -> Option<Self> {
        let mut diff = Self::default();
        for edit in arguments.edits.iter().flatten() {
            if let Some(old) = edit.search.as_deref() {
                diff.removed.extend(old.lines().map(str::to_owned));
            }
            if let Some(new) = edit.replace.as_deref() {
                diff.added.extend(new.lines().map(str::to_owned));
            }
        }
        if diff.removed.is_empty() && diff.added.is_empty() {
            if let Some(old) = arguments.old_string.as_deref() {
                diff.removed.extend(old.lines().map(str::to_owned));
            }
            if let Some(new) = arguments.new_string.as_deref() {
                diff.added.extend(new.lines().map(str::to_owned));
            }
        }
        if diff.removed.is_empty() && diff.added.is_empty() {
            return None;
        }
        Some(diff)
    }

    /// Rows needed to draw the diff in `mode`.
    pub(crate) fn rows(&self, mode: DiffMode) -> usize {
        match mode {
            DiffMode::Unified => self.removed.len() + self.added.len() + 1,
            DiffMode::Split => self.removed.len().max(self.added.len()) + 1,
        }
    }
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
