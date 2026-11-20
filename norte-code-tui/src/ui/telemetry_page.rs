//! The telemetry tab: time charts above, a paged request table and its detail.

use std::any::Any;
use std::fmt::Write;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::components::candlechart::CandleChart;
use terminalui::components::linechart::LineChart;
use terminalui::components::pagination::Pagination;
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View, node};

use super::detail_panel::{DetailPanel, Field};
use crate::telemetry::{ChartInterval, short_label};
use crate::telemetry_series::series;

/// Requests shown per page in the lower table.
const PAGE_SIZE: usize = 6;

/// The charts share one interval so their x axes line up.
#[derive(Serialize)]
pub(crate) struct TelemetryPage {
    interval: ChartInterval,
    report: Option<norte_code_wire::TelemetryReport>,
    /// Newest first.
    pub(crate) records: Vec<norte_code_wire::TelemetryRecord>,
    /// The page the table was last rebuilt for.
    last_page: usize,
    pub(crate) children: Vec<Node>,
}

impl TelemetryPage {
    /// A page with empty charts, filled by the host when data arrives.
    pub(crate) fn new() -> Self {
        Self {
            interval: ChartInterval::Hour,
            report: None,
            records: Vec::new(),
            last_page: 0,
            children: vec![
                node("cost_chart", CandleChart::new()),
                node("token_chart", LineChart::new()),
                node("cache_chart", LineChart::new().with_domain(0.0, 100.0)),
                node(
                    "request_table",
                    Table::new().with_header(node(
                        "header",
                        TableRow::header(vec![
                            node("h-time", TableCell::text("time").with_weight(3)),
                            node("h-model", TableCell::text("model").with_weight(5)),
                            node("h-status", TableCell::text("status").with_weight(2)),
                            node("h-tokens", TableCell::text("tokens").with_weight(2)),
                        ]),
                    )),
                ),
                node("request_pager", Pagination::new(1)),
                node("request_detail", DetailPanel::new()),
            ],
        }
    }

    /// Replace the report the charts and table are built from.
    pub(crate) fn set_report(&mut self, report: Option<norte_code_wire::TelemetryReport>) {
        self.report = report;
        self.records = self
            .report
            .as_ref()
            .map(|report| report.records.iter().rev().cloned().collect())
            .unwrap_or_default();
        self.rebuild_charts();
        self.rebuild_table();
        self.sync_detail();
    }

    fn rebuild_charts(&mut self) {
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

    fn page(&self) -> usize {
        self.children
            .get(4)
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Pagination>())
            .map_or(0, Pagination::page)
    }

    fn page_records(&self) -> &[norte_code_wire::TelemetryRecord] {
        let start = self.page() * PAGE_SIZE;
        self.records.get(start..).unwrap_or_default()
    }

    fn rebuild_table(&mut self) {
        let total = self.records.len().div_ceil(PAGE_SIZE).max(1);
        if let Some(pager) = self
            .children
            .get_mut(4)
            .and_then(|node| node.view.as_any().downcast_mut::<Pagination>())
        {
            if pager.total_pages != total {
                pager.set_total_pages(total);
            }
            pager.go_to(pager.page().min(total - 1));
        }
        let rows: Vec<Node> = self
            .page_records()
            .iter()
            .take(PAGE_SIZE)
            .enumerate()
            .map(|(index, record)| {
                let time = record.started_at.get(11..16).unwrap_or(&record.started_at);
                let tokens = record.input_tokens.unwrap_or(0) + record.output_tokens.unwrap_or(0);
                node(
                    format!("row-{index}"),
                    TableRow::body(vec![
                        node("time", TableCell::text(time.to_owned()).with_weight(3)),
                        node(
                            "model",
                            TableCell::text(clip(&record.model, 20)).with_weight(5),
                        ),
                        node(
                            "status",
                            TableCell::text(status_label(record.status)).with_weight(2),
                        ),
                        node("tokens", TableCell::text(tokens.to_string()).with_weight(2)),
                    ]),
                )
            })
            .collect();
        if let Some(table) = self
            .children
            .get_mut(3)
            .and_then(|node| node.view.as_any().downcast_mut::<Table>())
        {
            table.set_rows(rows);
            table.state.selected = 0;
        }
    }

    fn sync_detail(&mut self) {
        let selected = self
            .children
            .get(3)
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Table>())
            .map_or(0, Table::selected);
        let page = self.page() * PAGE_SIZE;
        let fields = self.records.get(page + selected).map_or_else(
            || vec![Field::new("no request", "select a row")],
            |record| {
                vec![
                    Field::new("id", clip(&record.id, 34)),
                    Field::new("model", clip(&record.model, 34)),
                    Field::new("provider", clip(&record.provider, 34)),
                    Field::new("status", status_label(record.status)),
                    Field::new("started", record.started_at.clone()),
                    Field::new("duration", format!("{} ms", record.duration_ms)),
                    Field::new(
                        "tokens",
                        format!(
                            "{} in / {} out",
                            record.input_tokens.unwrap_or(0),
                            record.output_tokens.unwrap_or(0)
                        ),
                    ),
                    Field::new(
                        "cache",
                        format!("{} read", record.cache_read_tokens.unwrap_or(0)),
                    ),
                    Field::new(
                        "cost",
                        record.cost_usd.map_or_else(
                            || "not reported".to_owned(),
                            |cost| format!("${cost:.4}"),
                        ),
                    ),
                ]
            },
        );
        if let Some(panel) = self
            .children
            .get_mut(5)
            .and_then(|node| node.view.as_any().downcast_mut::<DetailPanel>())
        {
            panel.set("request", fields);
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
        self.rebuild_charts();
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

    fn paint_charts(
        &mut self,
        area: Rect,
        chart_area: u16,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
    ) {
        let titles = [
            "cost · cumulative usd",
            "tokens · in + out",
            "cache · hit rate %",
        ];
        let chart_height = chart_area.saturating_sub(4) / 3;
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

        let chart_area = area.height.saturating_mul(2) / 5;
        self.paint_charts(area, chart_area, buf, cx, theme);

        let detail_width = (area.width / 2)
            .clamp(30, 48)
            .min(area.width.saturating_sub(30));
        let table_width = area.width.saturating_sub(detail_width + 1);
        let section_y = area.y + chart_area;
        if section_y >= area.y + area.height {
            return;
        }
        let section_height = area.y + area.height - section_y;
        buf.set_stringn(
            area.x + 1,
            section_y,
            "requests · newest first",
            23,
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        );
        let table_rect = Rect {
            x: area.x,
            y: section_y + 1,
            width: table_width,
            height: section_height.saturating_sub(2),
        };
        let pager_rect = Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(1),
            width: table_width,
            height: 1,
        };
        let detail_rect = Rect {
            x: area.x + table_width + 1,
            y: section_y,
            width: detail_width,
            height: section_height,
        };

        if let Some(child) = self.children.get_mut(4) {
            child.view.render(pager_rect, buf, cx, theme);
        }
        if self.page() != self.last_page {
            self.last_page = self.page();
            self.rebuild_table();
        }
        self.sync_detail();
        if let Some(child) = self.children.get_mut(3) {
            child.view.render(table_rect, buf, cx, theme);
        }
        if let Some(child) = self.children.get_mut(5) {
            child.view.render(detail_rect, buf, cx, theme);
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

fn status_label(status: norte_code_wire::RequestStatus) -> &'static str {
    match status {
        norte_code_wire::RequestStatus::Ok => "ok",
        norte_code_wire::RequestStatus::Error => "failed",
        norte_code_wire::RequestStatus::Cancelled => "cancelled",
    }
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}
