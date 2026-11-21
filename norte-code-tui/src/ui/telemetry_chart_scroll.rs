//! Vertical scrolling and the rail for the stacked telemetry charts.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::Theme;
use terminalui::components::scroll::{Scroll, ScrollbarStyle, render_scrollbar};

use crate::ui::scroll_drag::Scrollbar;

/// The stacked chart viewport, its offset, and the last painted rail.
#[derive(Debug, Default)]
pub(super) struct ChartScroll {
    offset: u16,
    viewport: u16,
    rail: Option<Scrollbar>,
    dragging: bool,
}

impl ChartScroll {
    /// First visible row of the stacked charts.
    pub(super) fn offset(&self) -> u16 {
        self.offset
    }

    /// Resize the viewport and keep the offset legal.
    pub(super) fn set_viewport(&mut self, viewport: u16, total: u16) {
        self.viewport = viewport;
        self.offset = self.offset.min(total.saturating_sub(viewport));
        if total <= viewport {
            self.rail = None;
            self.dragging = false;
        }
    }

    /// Paint the rail over the chart viewport and remember its geometry.
    pub(super) fn paint(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, total: u16) {
        if total <= self.viewport || area.height == 0 {
            self.rail = None;
            return;
        }
        let scroll = Scroll {
            offset: usize::from(self.offset),
            viewport: usize::from(self.viewport),
            total: usize::from(total),
        };
        let style = ScrollbarStyle {
            track_color: Some(theme.background),
            ..ScrollbarStyle::default()
        };
        render_scrollbar(buf, area, &style, theme, &scroll);
        let (top, size) = scroll.thumb();
        self.rail = Some(Scrollbar::new(
            area.x + area.width.saturating_sub(1),
            area.y,
            area.height,
            top,
            size,
        ));
    }

    /// Move by `delta` rows, clamped to the content.
    pub(super) fn scroll(&mut self, delta: isize, total: u16) {
        let max = i32::from(total.saturating_sub(self.viewport));
        let delta = i32::try_from(delta).unwrap_or(i32::MAX);
        let next = i32::from(self.offset).saturating_add(delta).clamp(0, max);
        self.offset = u16::try_from(next).unwrap_or(0);
    }

    /// Handle clicks and drags on the rail. Returns whether the event was used.
    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent, total: u16) -> bool {
        let Some(rail) = self.rail else {
            return false;
        };
        let on_rail = rail.on_rail(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if on_rail => {
                let local = mouse.row.saturating_sub(rail.y);
                if rail.thumb_contains(local) {
                    self.dragging = true;
                } else {
                    let page = isize::try_from(self.viewport).unwrap_or(isize::MAX);
                    self.scroll(rail.page(local) * page, total);
                }
                true
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.drag_to(rail, mouse.row.saturating_sub(rail.y), total);
                true
            }
            MouseEventKind::Up(MouseButton::Left | MouseButton::Right) if self.dragging => {
                self.dragging = false;
                true
            }
            _ => false,
        }
    }

    fn drag_to(&mut self, rail: Scrollbar, local: u16, total: u16) {
        let max = total.saturating_sub(self.viewport);
        self.offset = rail.offset_for_drag(local, max);
    }

    #[cfg(test)]
    pub(in crate::ui) fn rail(&self) -> Option<Scrollbar> {
        self.rail
    }
}
