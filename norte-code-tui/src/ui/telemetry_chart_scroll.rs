//! Vertical scrolling and the rail for the stacked telemetry charts.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::Theme;
use terminalui::components::scroll::{Scroll, ScrollbarStyle, render_scrollbar};

/// The stacked chart viewport, its offset, and the last painted rail.
#[derive(Debug, Default)]
pub(super) struct ChartScroll {
    offset: u16,
    viewport: u16,
    rail: Option<Rail>,
    dragging: bool,
}

/// Where the scrollbar rail and thumb landed in the last painted frame.
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct Rail {
    pub(in crate::ui) x: u16,
    pub(in crate::ui) y: u16,
    pub(in crate::ui) height: u16,
    pub(in crate::ui) top: u16,
    pub(in crate::ui) size: u16,
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
        self.rail = Some(Rail {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            height: area.height,
            top,
            size,
        });
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
        let on_rail = mouse.column == rail.x
            && mouse.row >= rail.y
            && mouse.row < rail.y.saturating_add(rail.height);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if on_rail => {
                let local = mouse.row.saturating_sub(rail.y);
                if (rail.top..rail.top.saturating_add(rail.size)).contains(&local) {
                    self.dragging = true;
                } else {
                    let page = isize::try_from(self.viewport).unwrap_or(isize::MAX);
                    self.scroll(if local < rail.top { -page } else { page }, total);
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

    fn drag_to(&mut self, rail: Rail, local: u16, total: u16) {
        let track = rail.height.saturating_sub(rail.size);
        let max = total.saturating_sub(self.viewport);
        if track == 0 {
            self.offset = 0;
            return;
        }
        let top = u32::from(local.saturating_sub(rail.size / 2)).min(u32::from(track));
        let offset = top * u32::from(max) / u32::from(track);
        self.offset = u16::try_from(offset).unwrap_or(u16::MAX).min(max);
    }

    #[cfg(test)]
    pub(in crate::ui) fn rail(&self) -> Option<Rail> {
        self.rail
    }
}
