//! The telemetry tab: time charts for spend, tokens and cache hits.

use std::any::Any;
use std::fmt::Write;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::components::candlechart::CandleChart;
use terminalui::components::linechart::LineChart;
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View, node};

use crate::telemetry::{ChartInterval, short_label};
use crate::telemetry_series::series;

/// The charts share one interval so their x axes line up.
#[derive(Serialize)]
pub(crate) struct TelemetryPage {
    interval: ChartInterval,
    report: Option<norte_code_wire::TelemetryReport>,
    children: Vec<Node>,
}

impl TelemetryPage {
    /// A page with empty charts, filled by the host when data arrives.
    pub(crate) fn new() -> Self {
        Self {
            interval: ChartInterval::Hour,
            report: None,
            children: vec![
                node("cost_chart", CandleChart::new()),
                node("token_chart", LineChart::new()),
                node("cache_chart", LineChart::new().with_domain(0.0, 100.0)),
            ],
        }
    }

    /// Replace the report the charts are built from.
    pub(crate) fn set_report(&mut self, report: Option<norte_code_wire::TelemetryReport>) {
        self.report = report;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let data = series(self.report.as_ref(), self.interval);
        if let Some(chart) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<CandleChart>())
        {
            let interval = self.interval;
            let labels: Vec<String> = data
                .buckets
                .iter()
                .map(|(key, _)| short_label(key, interval))
                .collect();
            chart.set_x_label(move |index| labels.get(index).cloned().unwrap_or_default());
            chart.set_series(data.cost);
        }
        if let Some(chart) = self
            .children
            .get_mut(1)
            .and_then(|node| node.view.as_any().downcast_mut::<LineChart>())
        {
            chart.set_series(data.tokens);
        }
        if let Some(chart) = self
            .children
            .get_mut(2)
            .and_then(|node| node.view.as_any().downcast_mut::<LineChart>())
        {
            chart.set_series(data.cache);
        }
    }

    fn cycle_interval(&mut self, backwards: bool) {
        self.interval = if backwards {
            match self.interval {
                ChartInterval::Minute => ChartInterval::Day,
                ChartInterval::Hour => ChartInterval::Minute,
                ChartInterval::Day => ChartInterval::Hour,
            }
        } else {
            self.interval.next()
        };
        self.rebuild();
    }

    fn caption(&self) -> String {
        let summary = self.report.as_ref().map(|report| &report.summary);
        let mut caption = format!("telemetry · {} buckets", self.interval.label());
        if let Some(summary) = summary {
            let _ = write!(
                caption,
                " · {} req · {} in / {} out",
                summary.requests, summary.input_tokens, summary.output_tokens
            );
        }
        caption
    }
}

impl View for TelemetryPage {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "telemetry_page"
    }

    fn children(&self) -> &[Node] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn focusable(&self) -> bool {
        false
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        buf.set_style(area, Style::default().bg(theme.background));
        let caption = self.caption();
        buf.set_stringn(
            area.x + 1,
            area.y,
            &caption,
            usize::from(area.width.saturating_sub(2)),
            Style::default().fg(theme.muted),
        );
        let hint = "[ ] interval · arrows scroll cost · wheel zooms";
        if area.width > 50 {
            let hint_width = u16::try_from(hint.len()).unwrap_or(u16::MAX);
            buf.set_stringn(
                area.x + area.width.saturating_sub(hint_width.saturating_add(1)),
                area.y,
                hint,
                hint.len(),
                Style::default().fg(theme.muted),
            );
        }

        let titles = [
            "cost · cumulative usd",
            "tokens · in + out",
            "cache · hit rate %",
        ];
        let chart_height = area.height.saturating_sub(7) / 3;
        let mut y = area.y + 1;
        for (index, title) in titles.iter().enumerate() {
            if y >= area.y + area.height {
                break;
            }
            buf.set_stringn(
                area.x + 1,
                y,
                title,
                title.len(),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            );
            let rect = Rect {
                x: area.x,
                y: y + 1,
                width: area.width,
                height: chart_height,
            };
            if let Some(child) = self.children.get_mut(index) {
                child.view.render(rect, buf, cx, theme);
            }
            y = y + 1 + chart_height + 1;
        }
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        if let InputEvent::Key(key) = event
            && key.kind == crossterm::event::KeyEventKind::Press
        {
            use crossterm::event::KeyCode;
            match key.code {
                KeyCode::Char('[') => {
                    self.cycle_interval(true);
                    return Outcome::Consumed;
                }
                KeyCode::Char(']') => {
                    self.cycle_interval(false);
                    return Outcome::Consumed;
                }
                _ => {}
            }
        }
        Outcome::Ignored
    }
}
