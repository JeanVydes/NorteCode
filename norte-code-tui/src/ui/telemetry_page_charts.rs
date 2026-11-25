//! Telemetry charts: series building, shared time windows, and painting.

use std::collections::BTreeMap;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use terminalui::components::candlechart::{CandleChart, CandleChartStyle};
use terminalui::components::linechart::{LineChart, LineChartStyle};
use terminalui::components::piechart::{PieChart, PieSlice};
use terminalui::components::presentation::{DotSize, StyleTuning};
use terminalui::{Context, Theme, View};

use crate::telemetry::{ChartInterval, short_label};
use crate::telemetry_series::{chart_window as compute_chart_window, series};

use super::{CHART_GAP, CHART_ROWS, CHART_SLOT, CHART_TITLES, TelemetryPage, TelemetryView};
use crate::ui::chart_renderer::{
    blit_rows, paint_slot_title, paint_time_labels, paint_time_labels_at,
};

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
    pub(crate) fn set_chart_preferences(
        &mut self,
        density: u8,
        contrast_quarters: u8,
        dot_size: u8,
    ) {
        let tuning = StyleTuning {
            density: density.clamp(2, 6),
            contrast: f32::from(contrast_quarters.clamp(1, 16)) / 4.0,
            dotsize: match dot_size.min(2) {
                0 => DotSize::Small,
                2 => DotSize::Large,
                _ => DotSize::Medium,
            },
        };
        self.chart_tuning = tuning;
        let presentation = self.chart_glyphs.render_style();
        for node in &mut self.children {
            if let Some(chart) = node.view.as_any().downcast_mut::<CandleChart>() {
                chart.tuning = tuning;
                chart.presentation = presentation;
            } else if let Some(chart) = node.view.as_any().downcast_mut::<LineChart>() {
                chart.tuning = tuning;
                chart.presentation = presentation;
            }
        }
        for chart in &mut self.additional_charts {
            chart.tuning = tuning;
            chart.presentation = presentation;
        }
        self.provider_cost_chart.set_tuning(tuning);
        self.model_cost_chart.set_tuning(tuning);
        self.provider_cost_chart.set_presentation(presentation);
        self.model_cost_chart.set_presentation(presentation);
    }

    pub(super) fn cycle_chart_glyphs(&mut self) {
        self.chart_glyphs = self.chart_glyphs.next();
    }

    pub(super) fn cycle_chart_density(&mut self) {
        self.chart_tuning.density = if self.chart_tuning.density >= 6 {
            2
        } else {
            self.chart_tuning.density + 1
        };
    }

    pub(super) fn cycle_chart_contrast(&mut self) {
        self.chart_tuning.contrast = match self.chart_tuning.contrast {
            value if value < 0.9 => 1.0,
            value if value < 1.1 => 1.25,
            value if value < 1.4 => 1.5,
            value if value < 1.8 => 2.0,
            _ => 0.75,
        };
    }

    pub(super) fn cycle_chart_dot_size(&mut self) {
        self.chart_tuning.dotsize = match self.chart_tuning.dotsize {
            DotSize::Small => DotSize::Medium,
            DotSize::Medium => DotSize::Large,
            _ => DotSize::Small,
        };
    }

    pub(super) fn zoom_chart_window(&mut self, factor: f64) {
        if let Some(chart) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<CandleChart>())
        {
            chart.zoom_x(factor);
        }
    }

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
        if let Some(chart) = self
            .children
            .get_mut(1)
            .and_then(|node| node.view.as_any().downcast_mut::<LineChart>())
        {
            chart.set_series(self.chart_data.input.clone());
        }
        if let Some(chart) = self
            .children
            .get_mut(2)
            .and_then(|node| node.view.as_any().downcast_mut::<LineChart>())
        {
            chart.set_gapped_series(self.chart_data.cache.clone());
        }
        let line_series = [
            (
                0,
                self.chart_data.output.iter().copied().map(Some).collect(),
            ),
            (
                1,
                self.chart_data.requests.iter().copied().map(Some).collect(),
            ),
            (2, self.chart_data.latency.clone()),
            (3, self.chart_data.first_output.clone()),
            (4, self.chart_data.first_text.clone()),
        ];
        for (index, values) in line_series {
            if let Some(chart) = self.additional_charts.get_mut(index) {
                chart.set_gapped_series(values);
            }
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
        let input = self
            .chart_data
            .input
            .get(start..end)
            .unwrap_or_default()
            .to_vec();
        let cache = self
            .chart_data
            .cache
            .get(start..end)
            .unwrap_or_default()
            .to_vec();
        let output = self
            .chart_data
            .output
            .get(start..end)
            .unwrap_or_default()
            .to_vec();
        let requests = self
            .chart_data
            .requests
            .get(start..end)
            .unwrap_or_default()
            .to_vec();
        let latency = self
            .chart_data
            .latency
            .get(start..end)
            .unwrap_or_default()
            .to_vec();
        let first_output = self
            .chart_data
            .first_output
            .get(start..end)
            .unwrap_or_default()
            .to_vec();
        let first_text = self
            .chart_data
            .first_text
            .get(start..end)
            .unwrap_or_default()
            .to_vec();
        if let Some(chart) = self
            .children
            .get_mut(1)
            .and_then(|node| node.view.as_any().downcast_mut::<LineChart>())
        {
            chart.set_series(input);
        }
        if let Some(chart) = self
            .children
            .get_mut(2)
            .and_then(|node| node.view.as_any().downcast_mut::<LineChart>())
        {
            chart.set_gapped_series(cache);
        }
        let line_series = [
            (0, output.into_iter().map(Some).collect()),
            (1, requests.into_iter().map(Some).collect()),
            (2, latency),
            (3, first_output),
            (4, first_text),
        ];
        for (index, values) in line_series {
            if let Some(chart) = self.additional_charts.get_mut(index) {
                chart.set_gapped_series(values);
            }
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
        if self.view != TelemetryView::Charts {
            return;
        }
        if self.chart_grid {
            self.scroll_chart_history(-delta);
        } else {
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
        match layout.index {
            0 | 1 | 3 => {
                let child_index = match layout.index {
                    0 => 0,
                    1 => 1,
                    _ => 2,
                };
                if let Some(child) = self.children.get_mut(child_index) {
                    child.view.render(body, page, cx, theme);
                }
            }
            2 | 4..=7 => {
                let chart_index = match layout.index {
                    2 => 0,
                    4 => 1,
                    5 => 2,
                    6 => 3,
                    _ => 4,
                };
                if let Some(chart) = self.additional_charts.get_mut(chart_index) {
                    chart.render(body, page, cx, theme);
                }
            }
            8 => paint_cost_mix(
                &mut self.provider_cost_chart,
                &mut self.model_cost_chart,
                body,
                page,
                cx,
                theme,
            ),
            _ => {}
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
        if layout.index > 0 && layout.index < 8 {
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

    fn apply_chart_appearance(&mut self, theme: &Theme) {
        let presentation = self.chart_glyphs.render_style();
        let tuning = self.chart_tuning;
        for (index, child) in self.children.iter_mut().take(3).enumerate() {
            if let Some(chart) = child.view.as_any().downcast_mut::<CandleChart>() {
                chart.presentation = presentation;
                chart.tuning = tuning;
                chart.style = CandleChartStyle {
                    grid: Some(theme.border),
                    label: Some(theme.muted),
                    live: Some(theme.accent),
                    ..CandleChartStyle::default()
                };
            } else if let Some(chart) = child.view.as_any().downcast_mut::<LineChart>() {
                chart.presentation = presentation;
                chart.tuning = tuning;
                let line = if index == 1 {
                    theme.accent
                } else {
                    theme.success
                };
                chart.style = LineChartStyle {
                    line: Some(line),
                    grid: Some(theme.border),
                    label: Some(theme.muted),
                    latest: Some(line),
                };
            }
        }
        for (index, chart) in self.additional_charts.iter_mut().enumerate() {
            let line = [
                theme.accent,
                theme.success,
                theme.warning,
                theme.destructive,
                theme.primary,
            ][index % 5];
            chart.presentation = presentation;
            chart.tuning = tuning;
            chart.style = LineChartStyle {
                line: Some(line),
                grid: Some(theme.border),
                label: Some(theme.muted),
                latest: Some(line),
            };
        }
        self.provider_cost_chart.set_slices(cost_slices(
            self.report.as_deref(),
            CostBreakdown::Provider,
            theme,
        ));
        self.model_cost_chart.set_slices(cost_slices(
            self.report.as_deref(),
            CostBreakdown::Model,
            theme,
        ));
    }

    fn paint_chart_grid(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        const GAP: u16 = 2;
        const GRID: usize = 3;
        let available_width = area.width.saturating_sub(GAP * 2);
        let available_height = area.height.saturating_sub(GAP * 2);
        let base_width = available_width / 3;
        let extra_width = available_width % 3;
        let base_height = available_height / 3;
        let extra_height = available_height % 3;
        if base_width < 24 || base_height < 8 {
            return;
        }
        self.chart_grid = true;
        self.chart_scroll.set_viewport(area.height, area.height);
        self.chart_areas.clear();
        buf.set_style(area, Style::default().bg(theme.background));
        let window = self.line_window.unwrap_or((0, 0));
        for (index, title_text) in CHART_TITLES.iter().enumerate() {
            let column = index % GRID;
            let row = index / GRID;
            let width = base_width + u16::from(column < usize::from(extra_width));
            let height = base_height + u16::from(row < usize::from(extra_height));
            let x = area.x
                + (base_width * u16::try_from(column).unwrap_or(0))
                + GAP * u16::try_from(column).unwrap_or(0)
                + u16::try_from(column.min(usize::from(extra_width))).unwrap_or(0);
            let y = area.y
                + (base_height * u16::try_from(row).unwrap_or(0))
                + GAP * u16::try_from(row).unwrap_or(0)
                + u16::try_from(row.min(usize::from(extra_height))).unwrap_or(0);
            let title = Rect::new(x, y, width, 1);
            buf.set_style(title, Style::default().bg(theme.secondary));
            buf.set_stringn(
                x.saturating_add(1),
                y,
                title_text,
                usize::from(width.saturating_sub(2)),
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.secondary)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            );
            let chart_height = height.saturating_sub(2);
            let chart_area = Rect::new(x, y.saturating_add(1), width, chart_height);
            if index == 8 {
                paint_cost_mix(
                    &mut self.provider_cost_chart,
                    &mut self.model_cost_chart,
                    chart_area,
                    buf,
                    cx,
                    theme,
                );
            } else if index == 0 {
                if let Some(child) = self.children.first_mut() {
                    child.view.render(chart_area, buf, cx, theme);
                }
            } else if let Some(chart) = self.chart_by_index_mut(index) {
                chart.render(chart_area, buf, cx, theme);
            }
            self.chart_areas.push(chart_area);
            if index > 0 && index < 8 {
                paint_time_labels_at(
                    &self.bucket_labels,
                    buf,
                    Rect::new(x, y.saturating_add(height.saturating_sub(1)), width, 1),
                    window,
                    theme,
                );
            }
        }
    }

    pub(super) fn paint_charts(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
    ) {
        if area.width >= 84 && area.height >= 27 {
            let window_width = area.width.saturating_sub(4) / 3;
            let window = self.chart_window(window_width);
            self.sync_line_window(window);
            self.apply_chart_appearance(theme);
            self.paint_chart_grid(area, buf, cx, theme);
            return;
        }
        self.chart_grid = false;
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
        self.apply_chart_appearance(theme);
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

fn paint_cost_mix(
    provider_chart: &mut PieChart,
    model_chart: &mut PieChart,
    area: Rect,
    buf: &mut Buffer,
    cx: &mut Context,
    theme: &Theme,
) {
    if area.width < 24 || area.height < 2 {
        return;
    }
    let half = area.width / 2;
    let chart_width = half.saturating_sub(1);
    let header_style = Style::default().fg(theme.muted);
    buf.set_stringn(
        area.x,
        area.y,
        "provider",
        usize::from(chart_width),
        header_style,
    );
    buf.set_stringn(
        area.x + half,
        area.y,
        "model",
        usize::from(area.width.saturating_sub(half)),
        header_style,
    );
    let chart_area = |x, width| Rect::new(x, area.y + 1, width, area.height.saturating_sub(1));
    provider_chart.render(chart_area(area.x, chart_width), buf, cx, theme);
    model_chart.render(
        chart_area(area.x + half, area.width.saturating_sub(half)),
        buf,
        cx,
        theme,
    );
}

impl TelemetryPage {
    fn chart_by_index_mut(&mut self, index: usize) -> Option<&mut LineChart> {
        match index {
            1 => self
                .children
                .get_mut(1)?
                .view
                .as_any()
                .downcast_mut::<LineChart>(),
            2 | 4..=7 => self.additional_charts.get_mut(match index {
                2 => 0,
                4 => 1,
                5 => 2,
                6 => 3,
                _ => 4,
            }),
            3 => self
                .children
                .get_mut(2)?
                .view
                .as_any()
                .downcast_mut::<LineChart>(),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum CostBreakdown {
    Provider,
    Model,
}

fn cost_slices(
    report: Option<&norte_code_wire::TelemetryReport>,
    breakdown: CostBreakdown,
    theme: &Theme,
) -> Vec<PieSlice> {
    let mut grouped = BTreeMap::<String, f64>::new();
    if let Some(report) = report {
        for record in &report.records {
            let cost = record.cost_usd.unwrap_or(0.0);
            if cost > 0.0 {
                let name = match breakdown {
                    CostBreakdown::Provider if record.provider.is_empty() => "Unknown provider",
                    CostBreakdown::Provider => record.provider.as_str(),
                    CostBreakdown::Model => {
                        record.resolved_model.as_deref().unwrap_or(&record.model)
                    }
                };
                let name = if name.is_empty() {
                    match breakdown {
                        CostBreakdown::Provider => "Unknown provider",
                        CostBreakdown::Model => "Unknown model",
                    }
                } else {
                    name
                };
                *grouped.entry(name.to_owned()).or_default() += cost;
            }
        }
    }
    let mut values: Vec<_> = grouped.into_iter().collect();
    values.sort_by(|left, right| right.1.total_cmp(&left.1));
    let colors = [
        theme.accent,
        theme.success,
        theme.warning,
        theme.destructive,
        theme.foreground,
    ];
    let mut slices: Vec<_> = values
        .drain(..)
        .take(colors.len())
        .enumerate()
        .map(|(index, (label, cost))| PieSlice::new(label, cost, colors[index]))
        .collect();
    let other: f64 = values.into_iter().map(|(_, cost)| cost).sum();
    if other > 0.0 {
        slices.push(PieSlice::new("Other", other, theme.muted));
    }
    slices
}
