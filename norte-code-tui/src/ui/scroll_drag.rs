//! Shared scrollbar geometry and mouse math for the app's custom rails.
//!
//! The library paints the thumb; these helpers decide where a click landed
//! and what offset a drag means, so every rail behaves the same way.

#[cfg(test)]
#[path = "scroll_drag_tests.rs"]
mod tests;

/// Where a one-cell vertical rail and its thumb landed in the last frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Scrollbar {
    /// Absolute column of the rail.
    pub(crate) x: u16,
    /// Absolute row of the rail's first cell.
    pub(crate) y: u16,
    /// Number of cells in the rail.
    pub(crate) height: u16,
    /// Thumb top, relative to the rail.
    pub(crate) top: u16,
    /// Thumb height in cells.
    pub(crate) size: u16,
}

impl Scrollbar {
    /// Build the geometry from a painted thumb.
    pub(crate) fn new(x: u16, y: u16, height: u16, top: u16, size: u16) -> Self {
        Self {
            x,
            y,
            height,
            top,
            size,
        }
    }

    /// Whether a screen cell is on this rail.
    pub(crate) fn on_rail(&self, column: u16, row: u16) -> bool {
        column == self.x && (self.y..self.y.saturating_add(self.height)).contains(&row)
    }

    /// Whether a rail-relative row is on the thumb.
    pub(crate) fn thumb_contains(&self, local: u16) -> bool {
        (self.top..self.top.saturating_add(self.size)).contains(&local)
    }

    /// Direction of a track click: up above the thumb, down below it.
    pub(crate) fn page(&self, local: u16) -> isize {
        if local < self.top { -1 } else { 1 }
    }

    /// Offset produced by dragging the thumb to a rail-relative row.
    pub(crate) fn offset_for_drag(&self, local: u16, max_offset: u16) -> u16 {
        let track = self.height.saturating_sub(self.size);
        if track == 0 {
            return 0;
        }
        let top = u32::from(local.saturating_sub(self.size / 2)).min(u32::from(track));
        let offset = top * u32::from(max_offset) / u32::from(track);
        u16::try_from(offset).unwrap_or(u16::MAX).min(max_offset)
    }
}
