//! Telemetry charts: series building, shared time windows, and painting.

use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use terminalui::components::candlechart::CandleChart;
use terminalui::components::linechart::LineChart;
use terminalui::{Context, Theme};

use crate::telemetry::{ChartInterval, short_label};
use crate::telemetry_series::{chart_window as compute_chart_window, series};

use super::{CHART_GAP, CHART_ROWS, CHART_SLOT, CHART_TITLES, TelemetryPage, TelemetryView};
use crate::ui::chart_renderer::{blit_rows, paint_slot_title, paint_time_labels};

#[derive(Clone, Copy, Debug)]
struct ChartSlotLayout {
    index: usize,
    title: &'static str,
    y: u16,
    window: (usize, usize),
    area: Rect,
    offset: u16,
}

impl TelemetryPage {
    #[cfg(test)]
    pub(crate) fn chart_scroll(&self) -> u16 {
        self.chart_scroll.offset()
    }

    #[cfg(test)]
    pub(in crate::ui) fn chart_rail(&self) -> Option<crate::ui::scroll_drag::Scrollbar> {
        self.chart_scroll.rail()
    }

    #[cfg(test)]
    pub(crate) fn chart_scroll_back(&self) -> usize {
        self.children
            .first()
            .and_then(|node| node.view.as_any_ref().downcast_ref::<CandleChart>())
            .map_or(0, |chart| chart.state.scroll_back)
    }

    #[cfg(test)]
    pub(crate) fn set_chart_scroll_back(&mut self, value: usize) {
        if let Some(chart) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<CandleChart>())
        {
            chart.state.scroll_back = value;
        }
    }

    #[cfg(test)]
    pub(crate) fn chart_page_capacity(&self) -> usize {
        self.chart_page.content.capacity()
    }

    #[cfg(test)]
    pub(crate) fn chart_area(&self, index: usize) -> Rect {
        self.chart_areas.get(index).copied().unwrap_or_default()
    }

    pub(super) fn rebuild_charts(&mut self) {
        let data = series(self.report.as_deref(), self.interval);
        let interval = self.interval;
        let labels = Arc::new(
            data.buckets
                .iter()
                .map(|(key, _)| short_label(key, interval))
                .collect(),
        );
        self.bucket_labels = Arc::clone(&labels);
        self.chart_data = data;
        self.line_window = None;
        let cost = std::mem::take(&mut self.chart_data.cost);
        if let Some(chart) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<CandleChart>())
        {
            chart.set_x_label(move |index| labels.get(index).cloned().unwrap_or_default());
            chart.set_series(cost);
        }
    }

    pub(super) fn chart_window(&self, width: u16) -> (usize, usize) {
        let Some(chart) = self
            .children
            .first()
            .and_then(|node| node.view.as_any_ref().downcast_ref::<CandleChart>())
        else {
            return (0, 0);
        };
        let total = self.bucket_labels.len();
        if total == 0 {
            return (0, 0);
        }
        compute_chart_window(total, chart.state.x_zoom, width, chart.state.scroll_back)
    }

    pub(super) fn sync_line_window(&mut self, window: (usize, usize)) {
        if self.line_window == Some(window) {
            return;
        }
        let (start, end) = window;
        let tokens = self
            .chart_data
            .tokens
            .get(start..end)
            .unwrap_or_default()
            .to_vec();
        let cache = self
            .chart_data
            .cache
            .get(start..end)
            .unwrap_or_default()
            .to_vec();
        if let Some(chart) = self
            .children
            .get_mut(1)
            .and_then(|node| node.view.as_any().downcast_mut::<LineChart>())
        {
            chart.set_series(tokens);
        }
        if let Some(chart) = self
            .children
            .get_mut(2)
            .and_then(|node| node.view.as_any().downcast_mut::<LineChart>())
        {
            chart.set_series(cache);
        }
        self.line_window = Some(window);
    }

    pub(super) fn scroll_chart_history(&mut self, delta: isize) {
        let Some(chart) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<CandleChart>())
        else {
            return;
        };
        let max = isize::try_from(self.bucket_labels.len().saturating_sub(1)).unwrap_or(isize::MAX);
        let current = isize::try_from(chart.state.scroll_back).unwrap_or(isize::MAX);
        chart.state.scroll_back = usize::try_from((current + delta).clamp(0, max)).unwrap_or(0);
    }

    pub(crate) fn scroll_chart_view(&mut self, delta: isize) {
        if self.view == TelemetryView::Charts {
            self.chart_scroll.scroll(delta, CHART_ROWS);
        }
    }

    pub(super) fn cycle_interval(&mut self, backwards: bool) {
        self.interval = if backwards {
            match self.interval {
                ChartInterval::Minute => ChartInterval::Day,
                ChartInterval::Hour => ChartInterval::Minute,
                ChartInterval::Day => ChartInterval::Hour,
            }
        } else {
            self.interval.next()
        };
        self.rebuild_charts();
    }

    fn paint_chart_slot(&mut self, layout: ChartSlotLayout, cx: &mut Context, theme: &Theme) {
        let page = &mut self.chart_page;
        let body = paint_slot_title(page, layout.y, layout.area.width, layout.title, theme);
        let body_y = layout.y + 1;
        if let Some(child) = self.children.get_mut(layout.index) {
            child.view.render(body, page, cx, theme);
        }
        if layout.index == 0
            && let Some(chart) = self
                .children
                .first_mut()
                .and_then(|node| node.view.as_any().downcast_mut::<CandleChart>())
        {
            chart.state.area = Rect {
                x: layout.area.x,
                y: layout.area.y + body_y.saturating_sub(layout.offset),
                width: layout.area.width,
                height: CHART_SLOT - 1,
            };
        }
        if layout.index > 0 {
            paint_time_labels(
                &self.bucket_labels,
                page,
                body_y + CHART_SLOT - 2,
                layout.area.width,
                layout.window,
                theme,
            );
        }
        self.chart_areas.push(Rect {
            x: layout.area.x,
            y: layout.area.y + body_y.saturating_sub(layout.offset),
            width: layout.area.width,
            height: CHART_SLOT - 1,
        });
    }

    pub(super) fn paint_charts(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
    ) {
        self.chart_scroll.set_viewport(area.height, CHART_ROWS);
        let offset = self.chart_scroll.offset();
        self.chart_areas.clear();
        let page_area = Rect::new(0, 0, area.width, CHART_ROWS);
        if self.chart_page.area == page_area {
            self.chart_page.reset();
        } else {
            self.chart_page.resize(page_area);
        }
        self.chart_page
            .set_style(page_area, Style::default().bg(theme.background));
        let window = self.chart_window(area.width);
        self.sync_line_window(window);
        for (index, title) in CHART_TITLES.iter().enumerate() {
            let layout = ChartSlotLayout {
                index,
                title,
                y: u16::try_from(index).unwrap_or(u16::MAX) * (CHART_SLOT + CHART_GAP),
                window,
                area,
                offset,
            };
            self.paint_chart_slot(layout, cx, theme);
        }
        blit_rows(&self.chart_page, buf, area, offset);
        self.chart_scroll.paint(area, buf, theme, CHART_ROWS);
    }
}
