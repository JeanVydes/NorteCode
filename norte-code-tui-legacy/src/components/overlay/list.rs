//! A modal that lists rows and lets the reader filter them.
//!
//! It owns the selection and the filter so every menu behaves the same: type to
//! filter, `j`/`k` or the arrows to move, `Enter` to run, `Esc` to close.

use crossterm::event::{KeyCode, KeyEvent};

use crate::components::{MenuEntry, overlay::Modal, palette};
use crate::keys;
use crate::ui_model::ThemeMode;

/// A filtered, scrollable list inside a modal.
pub(crate) struct ModalList {
    entries: Vec<MenuEntry>,
    filter: String,
    selected: usize,
}

/// What a key did to the list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ListOutcome {
    /// The key moved the selection or the filter.
    Handled,
    /// The reader confirmed the selected row.
    Submitted(usize),
    /// The reader asked to close the list.
    Closed,
    /// The key is not the list's.
    Ignored,
}

impl ModalList {
    /// A list over the rows, with nothing filtered.
    pub(crate) fn new(entries: Vec<MenuEntry>) -> Self {
        Self {
            entries,
            filter: String::new(),
            selected: 0,
        }
    }

    /// The rows the filter lets through, with their index in the whole list.
    fn visible(&self) -> Vec<(usize, &MenuEntry)> {
        let filter = self.filter.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| filter.is_empty() || entry.label.to_lowercase().contains(&filter))
            .collect()
    }

    /// The index in the whole list of the selected row.
    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.visible().get(self.selected).map(|(index, _)| *index)
    }

    /// Handles one key.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> ListOutcome {
        match key.code {
            keys::CANCEL => return ListOutcome::Closed,
            keys::SUBMIT => {
                return self
                    .selected_index()
                    .map_or(ListOutcome::Handled, ListOutcome::Submitted);
            }
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                let len = self.visible().len();
                self.selected = self.selected.saturating_add(1).min(len.saturating_sub(1));
                return ListOutcome::Handled;
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                self.selected = self.selected.saturating_sub(1);
                return ListOutcome::Handled;
            }
            keys::ERASE => {
                self.filter.pop();
                self.selected = 0;
                return ListOutcome::Handled;
            }
            KeyCode::Char(character) => {
                self.filter.push(character);
                self.selected = 0;
                return ListOutcome::Handled;
            }
            _ => {}
        }
        ListOutcome::Ignored
    }

    /// Empties the filter, so `Esc` inside a filtered list first drops it.
    pub(crate) fn clear_filter(&mut self) -> bool {
        if self.filter.is_empty() {
            return false;
        }
        self.filter.clear();
        self.selected = 0;
        true
    }

    /// The labels the filter lets through, for a test to read.
    #[cfg(test)]
    pub(crate) fn visible_labels(&self) -> Vec<String> {
        self.visible()
            .into_iter()
            .map(|(_, entry)| entry.label.clone())
            .collect()
    }

    /// The modal body: one row per visible entry, with its section caption.
    fn body(&self) -> (Vec<ratatui::text::Line<'static>>, Option<usize>) {
        let mut lines = Vec::new();
        let mut selected_line = None;
        let mut section: Option<String> = None;
        for (visible_index, (_, entry)) in self.visible().into_iter().enumerate() {
            if entry.section != section {
                if !lines.is_empty() {
                    lines.push(ratatui::text::Line::default());
                }
                if let Some(name) = &entry.section {
                    lines.push(ratatui::text::Line::from(ratatui::text::Span::raw(
                        name.clone(),
                    )));
                }
                section.clone_from(&entry.section);
            }
            if visible_index == self.selected {
                selected_line = Some(lines.len());
            }
            lines.push(Self::row(entry));
        }
        (lines, selected_line)
    }

    fn row(entry: &MenuEntry) -> ratatui::text::Line<'static> {
        use ratatui::text::{Line, Span};
        let marker = if entry.current { "● " } else { "  " };
        let mut spans = vec![Span::raw(marker), Span::raw(entry.label.clone())];
        if let Some(hint) = &entry.hint {
            let pad = 34_usize.saturating_sub(entry.label.chars().count());
            spans.push(Span::raw(" ".repeat(pad.max(1))));
            spans.push(Span::raw(hint.clone()));
        }
        Line::from(spans)
    }

    /// Draws the list as a modal.
    pub(crate) fn render(&self, frame: &mut ratatui::Frame<'_>, title: &str, theme: ThemeMode) {
        let (body, selected) = self.body();
        let mut body = body;
        if !self.filter.is_empty() {
            body.insert(
                0,
                ratatui::text::Line::from(ratatui::text::Span::styled(
                    format!("/{}", self.filter),
                    palette::muted_style(theme),
                )),
            );
            body.insert(1, ratatui::text::Line::default());
        }
        let selected = selected.map(|index| index + usize::from(!self.filter.is_empty()) * 2);
        Modal {
            title,
            controls: "Enter run, j/k move, type to filter, Esc close",
            body,
            theme,
            selected,
        }
        .render(frame);
    }
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
