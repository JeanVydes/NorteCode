//! One paginated cursor.
//!
//! Both the session list and the request list page at ten rows, so the cursor
//! has the same behaviour in both: `]` and `[` turn the page, and the selection
//! never leaves the list.

use super::telemetry::PAGE_SIZE;

/// A selection and the page it sits on.
#[derive(Default)]
pub(crate) struct Pages {
    selected: usize,
    page: usize,
    /// How many pages the list had at the last clamp.
    count: usize,
}

impl Pages {
    /// The index of the selected row in the whole list.
    pub(crate) const fn selected(&self) -> usize {
        self.selected
    }

    /// Where the selection sits inside its page.
    pub(crate) const fn row(&self) -> usize {
        self.selected % PAGE_SIZE
    }

    /// The rows of the current page, cloned out of the list.
    pub(super) fn page_items<T: Clone>(&self, items: &[T]) -> Vec<T> {
        let start = self.page * PAGE_SIZE;
        items.iter().skip(start).take(PAGE_SIZE).cloned().collect()
    }

    /// Puts the selection on an absolute index.
    pub(crate) fn set_selected(&mut self, index: usize) {
        self.selected = index;
        self.page = index / PAGE_SIZE;
    }

    /// Moves the selection down one row.
    pub(crate) fn down(&mut self, length: usize) {
        if length == 0 {
            self.selected = 0;
            return;
        }
        self.selected = self.selected.saturating_add(1).min(length - 1);
        self.page = self.selected / PAGE_SIZE;
    }

    /// Moves the selection up one row.
    pub(crate) fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.page = self.selected / PAGE_SIZE;
    }

    /// Turns to the next page, where the selection starts again.
    pub(crate) fn next_page(&mut self, length: usize) {
        let pages = page_count(length);
        if self.page + 1 < pages {
            self.page += 1;
            self.selected = self.page * PAGE_SIZE;
        }
    }

    /// Turns to the previous page.
    pub(crate) fn previous_page(&mut self) {
        if self.page > 0 {
            self.page -= 1;
            self.selected = self.page * PAGE_SIZE;
        }
    }

    /// Keeps the selection inside the list after it shrank.
    pub(crate) fn clamp(&mut self, length: usize) {
        self.count = page_count(length);
        if length == 0 {
            self.selected = 0;
            self.page = 0;
            return;
        }
        self.selected = self.selected.min(length - 1);
        self.page = self.selected / PAGE_SIZE;
    }

    /// Starts the request cursor at the first row, on opening a session.
    pub(crate) fn reset(&mut self) {
        self.selected = 0;
        self.page = 0;
    }

    /// The page the cursor is on.
    pub(crate) const fn page(&self) -> usize {
        self.page
    }

    /// How many pages the list has.
    pub(crate) fn total(&self) -> usize {
        self.count
    }
}

/// How many pages a list needs, at least one.
pub(crate) const fn page_count(length: usize) -> usize {
    let pages = length.saturating_add(PAGE_SIZE - 1) / PAGE_SIZE;
    if pages == 0 { 1 } else { pages }
}
