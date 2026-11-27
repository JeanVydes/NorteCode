//! The annual usage heatmap painted above the telemetry charts.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use terminalui::Theme;

use crate::usage_calendar::CALENDAR_WEEKS;

use super::TelemetryPage;

const LABEL_WIDTH: u16 = 3;
const WEEKDAY_LABELS: [(u16, &str); 3] = [(1, "Mon"), (3, "Wed"), (5, "Fri")];
const LEGEND: [u8; 5] = [0, 1, 2, 3, 4];
const LEGEND_WIDTH: u16 = 15;

impl TelemetryPage {
    #[cfg(test)]
    pub(crate) fn heatmap_area(&self) -> Rect {
        self.heatmap_area
    }

    #[cfg(test)]
    pub(crate) fn heatmap_weeks(&self) -> usize {
        self.heatmap_weeks
    }

    pub(super) fn paint_heatmap(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.heatmap_area = area;
        let Some(grid_width) = area.width.checked_sub(LABEL_WIDTH) else {
            return;
        };
        let weeks = usize::from(grid_width).min(CALENDAR_WEEKS);
        self.heatmap_weeks = weeks;
        if area.height == 0 || weeks == 0 {
            return;
        }
        buf.set_style(area, Style::default().bg(theme.background));
        let grid_x = area.x + LABEL_WIDTH;
        let first_week = CALENDAR_WEEKS - weeks;
        self.paint_heatmap_labels(area, grid_x, first_week, weeks, buf, theme);
        self.paint_heatmap_cells(area, grid_x, first_week, weeks, buf, theme);
        Self::paint_heatmap_legend(area, grid_x, weeks, buf, theme);
    }

    fn paint_heatmap_labels(
        &self,
        area: Rect,
        grid_x: u16,
        first_week: usize,
        weeks: usize,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let muted = Style::default().fg(theme.muted);
        for (weekday, label) in WEEKDAY_LABELS {
            let y = area.y + 1 + weekday;
            if y < area.y + area.height {
                buf.set_stringn(area.x, y, label, usize::from(LABEL_WIDTH), muted);
            }
        }
        for (week, name) in self.usage_calendar.month_labels() {
            let Some(display) = week.checked_sub(first_week) else {
                continue;
            };
            if display >= weeks {
                continue;
            }
            let x = grid_x.saturating_add(u16::try_from(display).unwrap_or(u16::MAX));
            buf.set_stringn(x, area.y, name, name.len(), muted);
        }
    }

    fn paint_heatmap_cells(
        &self,
        area: Rect,
        grid_x: u16,
        first_week: usize,
        weeks: usize,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        for display in 0..weeks {
            let x = grid_x.saturating_add(u16::try_from(display).unwrap_or(u16::MAX));
            let source = first_week + display;
            for weekday in 0..7u16 {
                let y = area.y + 1 + weekday;
                if y >= area.y + area.height {
                    break;
                }
                let cell = &mut buf[(x, y)];
                if self.usage_calendar.is_future(source, usize::from(weekday)) {
                    cell.set_symbol(" ")
                        .set_style(Style::default().bg(theme.background));
                    continue;
                }
                let level = self.usage_calendar.level(source, usize::from(weekday));
                let symbol = match level {
                    1 => "▪",
                    2 => "■",
                    3..=4 => "●",
                    _ => "·",
                };
                cell.set_symbol(symbol)
                    .set_style(Style::default().fg(level_color(theme, level)));
            }
        }
    }

    fn paint_heatmap_legend(
        area: Rect,
        grid_x: u16,
        weeks: usize,
        buf: &mut Buffer,
        theme: &Theme,
    ) {
        let legend_x = grid_x
            .saturating_add(u16::try_from(weeks).unwrap_or(u16::MAX))
            .saturating_add(1);
        if legend_x.saturating_add(LEGEND_WIDTH) > area.x.saturating_add(area.width) {
            return;
        }
        let muted = Style::default().fg(theme.muted);
        buf.set_stringn(legend_x, area.y, "less", 4, muted);
        for (index, level) in LEGEND.iter().enumerate() {
            let x = legend_x
                .saturating_add(5)
                .saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
            let symbol = match level {
                1 => "▪",
                2 => "■",
                3..=4 => "●",
                _ => "·",
            };
            buf[(x, area.y)]
                .set_symbol(symbol)
                .set_fg(level_color(theme, *level));
        }
        buf.set_stringn(legend_x + 11, area.y, "more", 4, muted);
    }
}

fn level_color(theme: &Theme, level: u8) -> Color {
    match level {
        0 => theme.border,
        1 => blend(theme.background, theme.primary, 44),
        2 => blend(theme.background, theme.primary, 66),
        3 => blend(theme.background, theme.primary, 84),
        _ => theme.primary,
    }
}

fn blend(from: Color, to: Color, percent: u16) -> Color {
    let (Color::Rgb(from_red, from_green, from_blue), Color::Rgb(to_red, to_green, to_blue)) =
        (from, to)
    else {
        return to;
    };
    Color::Rgb(
        mix(from_red, to_red, percent),
        mix(from_green, to_green, percent),
        mix(from_blue, to_blue, percent),
    )
}

fn mix(from: u8, to: u8, percent: u16) -> u8 {
    let delta = i32::from(to) - i32::from(from);
    let value = i32::from(from) + delta * i32::from(percent) / 100;
    u8::try_from(value).unwrap_or(255)
}
