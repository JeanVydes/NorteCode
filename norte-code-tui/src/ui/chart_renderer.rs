//! Pure painting for telemetry chart slots and shared time labels.
//!
//! Split out of `TelemetryPage` so the pixel math (title, labels, scroll
//! blit) can be unit tested without a full page, report or child tree.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use terminalui::Theme;

use crate::ui::telemetry_page::{CHART_ROWS, CHART_SLOT, LINE_AXIS_WIDTH};

/// Paint one chart slot's title at `layout_y`, returning the body rect the
/// caller should render the chart into.
pub(super) fn paint_slot_title(
    page: &mut Buffer,
    layout_y: u16,
    width: u16,
    title: &str,
    theme: &Theme,
) -> Rect {
    let title_area = Rect::new(0, layout_y, width, 1);
    page.set_style(title_area, Style::default().bg(theme.secondary));
    page.set_stringn(
        1,
        layout_y,
        title,
        usize::from(width.saturating_sub(2)),
        Style::default()
            .fg(theme.accent)
            .bg(theme.secondary)
            .add_modifier(Modifier::BOLD),
    );
    let body_y = layout_y + 1;
    Rect {
        x: 0,
        y: body_y,
        width,
        height: CHART_SLOT - 1,
    }
}

/// Draw every tenth time label across the plot area.
pub(super) fn paint_time_labels(
    labels: &[String],
    page: &mut Buffer,
    y: u16,
    width: u16,
    window: (usize, usize),
    theme: &Theme,
) {
    paint_time_labels_at(labels, page, Rect::new(0, y, width, 1), window, theme);
}

pub(super) fn paint_time_labels_at(
    labels: &[String],
    page: &mut Buffer,
    area: Rect,
    window: (usize, usize),
    theme: &Theme,
) {
    let (start, end) = window;
    if start == end || area.width == 0 || area.height == 0 {
        return;
    }
    let plot_width = area.width.saturating_sub(LINE_AXIS_WIDTH).max(1);
    let last = end.saturating_sub(start).saturating_sub(1);
    let label_capacity = usize::from(plot_width / 8).max(1);
    let step = end.saturating_sub(start).div_ceil(label_capacity).max(1);
    let style = Style::default().fg(theme.muted);
    for index in (start..end).step_by(step) {
        let Some(label) = labels.get(index) else {
            continue;
        };
        let relative = index.saturating_sub(start);
        let column = relative
            .checked_mul(usize::from(plot_width.saturating_sub(1)))
            .and_then(|value| value.checked_div(last))
            .map_or(0, |value| u16::try_from(value).unwrap_or(0));
        page.set_stringn(area.x.saturating_add(column), area.y, label, 5, style);
    }
}

/// Copy `area` from the composed `page` buffer into the real frame at
/// vertical scroll `offset`.
pub(super) fn blit_rows(page: &Buffer, buf: &mut Buffer, area: Rect, offset: u16) {
    let inside = u32::from(area.x) >= u32::from(buf.area.x)
        && u32::from(area.y) >= u32::from(buf.area.y)
        && u32::from(area.x) + u32::from(area.width)
            <= u32::from(buf.area.x) + u32::from(buf.area.width)
        && u32::from(area.y) + u32::from(area.height)
            <= u32::from(buf.area.y) + u32::from(buf.area.height);
    if !inside {
        return;
    }
    let stride = usize::from(buf.area.width);
    let left = usize::from(area.x - buf.area.x);
    let top = usize::from(area.y - buf.area.y);
    let width = usize::from(area.width);
    for row in 0..area.height {
        let source = usize::from(row.saturating_add(offset));
        if source >= usize::from(CHART_ROWS) {
            break;
        }
        let target = (top + usize::from(row)) * stride + left;
        let origin = source * width;
        buf.content[target..target + width].clone_from_slice(&page.content[origin..origin + width]);
    }
}
