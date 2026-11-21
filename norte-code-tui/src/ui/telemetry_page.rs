//! The telemetry tab: charts, a session pager, and that session's requests.

use std::any::Any;
use std::collections::HashSet;
use std::fmt::Write;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Position as RatatuiPosition;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::components::candlechart::CandleChart;
use terminalui::components::linechart::LineChart;
use terminalui::components::pagination::Pagination;
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View, node};

use super::detail_panel::{DetailPanel, Field};
use super::leaderboard_section::{LeaderboardClick, LeaderboardSection};
use crate::session_stats::{SessionUsage, SharedUsage};
use crate::shared_telemetry::SharedTelemetry;
use crate::telemetry::{ChartInterval, short_label};
use crate::telemetry_series::{Series, series};
use chart_scroll::ChartScroll;

#[path = "telemetry_chart_scroll.rs"]
mod chart_scroll;

/// Sessions and requests shown per page.
const SESSION_PAGE: usize = 4;
const REQUEST_PAGE: usize = 6;
const CHART_SLOT: u16 = 18;
const CHART_GAP: u16 = 2;
const CHART_ROWS: u16 = CHART_SLOT * 3 + CHART_GAP * 2;
const LINE_AXIS_WIDTH: u16 = 9;

/// Which view Tab currently shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
enum TelemetryView {
    /// Three full-height time charts.
    #[default]
    Charts,
    /// The paginated session list and its detail.
    Sessions,
    /// The selected session's requests and their detail.
    Requests,
    /// Rankings of models, providers and sessions.
    Leaderboards,
}

/// The charts share one interval so their x axes line up.
#[derive(Serialize)]
pub(crate) struct TelemetryPage {
    interval: ChartInterval,
    #[serde(skip)]
    report: Option<SharedTelemetry>,
    /// Indices into `report.records`, newest first.
    #[serde(skip)]
    pub(crate) record_indices: Vec<usize>,
    /// Rollup computed once per report, not on every frame.
    #[serde(skip)]
    usage: SharedUsage,
    /// Indices of the selected session's records, newest first.
    selected_records: Vec<usize>,
    /// One representative record index per distinct session, sorted by id.
    sessions: Vec<usize>,
    /// Session summaries from the host, for the shared detail panel.
    session_data: Vec<norte_code_wire::SessionSummary>,
    /// The session whose requests are shown.
    selected_session: usize,
    /// Which panel the detail shows.
    pub(crate) detail_is_request: bool,
    /// Which view is on screen.
    view: TelemetryView,
    /// Scroll offset and rail of the tall stacked charts.
    #[serde(skip)]
    chart_scroll: ChartScroll,
    /// Screen areas of the three chart bodies, for wheel hit-testing.
    #[serde(skip)]
    chart_areas: Vec<Rect>,
    /// Full series behind the charts, sliced for the aligned time window.
    #[serde(skip)]
    chart_data: Series,
    /// Short time labels shared by all three x axes.
    #[serde(skip)]
    bucket_labels: Arc<Vec<String>>,
    /// Window last pushed into the two line charts.
    #[serde(skip)]
    line_window: Option<(usize, usize)>,
    /// Pages the request table was last rebuilt for.
    last_session_page: usize,
    last_request_page: usize,
    /// Last highlighted request row, to switch the panel on selection.
    last_request_row: usize,
    /// The rankings view and its shared report.
    #[serde(skip)]
    leaderboard: LeaderboardSection,
    pub(crate) children: Vec<Node>,
}

impl TelemetryPage {
    /// A page with empty charts, filled by the host when data arrives.
    pub(crate) fn new() -> Self {
        Self {
            interval: ChartInterval::Hour,
            report: None,
            record_indices: Vec::new(),
            usage: SharedUsage::default(),
            selected_records: Vec::new(),
            sessions: Vec::new(),
            session_data: Vec::new(),
            selected_session: 0,
            detail_is_request: false,
            view: TelemetryView::Charts,
            chart_scroll: ChartScroll::default(),
            chart_areas: Vec::new(),
            chart_data: Series::default(),
            bucket_labels: Arc::new(Vec::new()),
            line_window: None,
            last_session_page: 0,
            last_request_page: 0,
            last_request_row: 0,
            leaderboard: LeaderboardSection::new(),
            children: vec![
                node("cost_chart", CandleChart::new()),
                node("token_chart", LineChart::new()),
                node("cache_chart", LineChart::new().with_domain(0.0, 100.0)),
                node("session_pager", Pagination::new(1)),
                node(
                    "request_table",
                    Table::new().with_header(node(
                        "header",
                        TableRow::header(vec![
                            node("h-time", TableCell::text("time").with_weight(3)),
                            node("h-model", TableCell::text("model").with_weight(5)),
                            node("h-status", TableCell::text("status").with_weight(2)),
                            node("h-tokens", TableCell::text("tokens").with_weight(2)),
                            node("h-cost", TableCell::text("usd").with_weight(3)),
                        ]),
                    )),
                ),
                node("request_pager", Pagination::new(1)),
                node("request_detail", DetailPanel::new()),
            ],
        }
    }

    /// Whether Tab currently shows the rankings.
    pub(crate) fn is_leaderboard_focused(&self) -> bool {
        self.view == TelemetryView::Leaderboards
    }

    #[cfg(test)]
    pub(crate) fn leaderboard_group(&self) -> crate::leaderboard::LeaderboardGroup {
        self.leaderboard.group()
    }

    #[cfg(test)]
    pub(crate) fn leaderboard_subject(&self) -> crate::leaderboard::LeaderboardSubject {
        self.leaderboard.subject()
    }

    #[cfg(test)]
    pub(crate) fn leaderboard_selected(&self) -> usize {
        self.leaderboard.selected()
    }

    #[cfg(test)]
    pub(crate) fn leaderboard_area(&self) -> ratatui::layout::Rect {
        self.leaderboard.area()
    }

    #[cfg(test)]
    pub(crate) fn chart_scroll(&self) -> u16 {
        self.chart_scroll.offset()
    }

    #[cfg(test)]
    pub(super) fn chart_rail(&self) -> Option<crate::ui::scroll_drag::Scrollbar> {
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
    pub(crate) fn chart_area(&self, index: usize) -> Rect {
        self.chart_areas.get(index).copied().unwrap_or_default()
    }

    /// Tab cycles charts, the two paginated lists, then the rankings.
    pub(crate) fn toggle_focus(&mut self) {
        self.view = match self.view {
            TelemetryView::Charts => TelemetryView::Sessions,
            TelemetryView::Sessions => TelemetryView::Requests,
            TelemetryView::Requests => TelemetryView::Leaderboards,
            TelemetryView::Leaderboards => TelemetryView::Charts,
        };
    }

    /// Replace the session summaries used by the shared detail panel.
    pub(crate) fn set_sessions(&mut self, sessions: Vec<norte_code_wire::SessionSummary>) {
        self.session_data = sessions;
        self.sync_detail();
    }

    /// Replace the report the charts, session list and request table use.
    pub(crate) fn set_report(&mut self, report: Option<SharedTelemetry>, usage: SharedUsage) {
        self.report = report;
        self.usage = usage;
        self.record_indices = self
            .report
            .as_ref()
            .map_or_else(Vec::new, |report| (0..report.records.len()).rev().collect());
        self.sessions = self.report.as_ref().map_or_else(Vec::new, |report| {
            let mut seen = HashSet::new();
            let mut sessions: Vec<usize> = report
                .records
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, record)| seen.insert(record.session.as_str()))
                .map(|(index, _)| index)
                .collect();
            sessions.sort_by(|left, right| {
                report.records[*left]
                    .session
                    .cmp(&report.records[*right].session)
            });
            sessions
        });
        self.selected_session = 0;
        self.detail_is_request = false;
        self.rebuild_selected();
        self.rebuild_charts();
        self.rebuild_session_pager();
        self.rebuild_request_table();
        self.rebuild_leaderboard();
        self.sync_detail();
    }

    fn record(&self, index: usize) -> Option<&norte_code_wire::TelemetryRecord> {
        self.report.as_ref()?.records.get(index)
    }

    fn selected_record(&self, position: usize) -> Option<&norte_code_wire::TelemetryRecord> {
        self.selected_records
            .get(position)
            .copied()
            .and_then(|index| self.record(index))
    }

    fn session_id(&self, position: usize) -> Option<&str> {
        self.sessions
            .get(position)
            .copied()
            .and_then(|index| self.record(index))
            .map(|record| record.session.as_str())
    }

    fn rebuild_charts(&mut self) {
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

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn chart_window(&self) -> (usize, usize) {
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
        let cell_width = (2.0 * chart.state.x_zoom).round().clamp(1.0, 16.0) as u16;
        let plot_width = chart.state.area.width.saturating_sub(11).max(1);
        let visible = usize::from(plot_width / cell_width).max(1);
        let end = total.saturating_sub(chart.state.scroll_back).max(1);
        (end.saturating_sub(visible), end)
    }

    fn sync_line_window(&mut self, window: (usize, usize)) {
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

    fn paint_time_labels(
        &mut self,
        page: &mut Buffer,
        y: u16,
        width: u16,
        window: (usize, usize),
        theme: &Theme,
    ) {
        let (start, end) = window;
        if start == end {
            return;
        }
        let plot_width = width.saturating_sub(LINE_AXIS_WIDTH).max(1);
        let last = end.saturating_sub(start).saturating_sub(1);
        let style = Style::default().fg(theme.muted);
        for index in (start..end).step_by(10) {
            let Some(label) = self.bucket_labels.get(index) else {
                continue;
            };
            let relative = index.saturating_sub(start);
            let column = relative
                .checked_mul(usize::from(plot_width.saturating_sub(1)))
                .and_then(|value| value.checked_div(last))
                .map_or(0, |value| u16::try_from(value).unwrap_or(0));
            page.set_stringn(column, y, label, 5, style);
        }
    }

    fn scroll_chart_history(&mut self, delta: isize) {
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

    fn pager_page(&self, index: usize) -> usize {
        self.children
            .get(index)
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Pagination>())
            .map_or(0, Pagination::page)
    }

    fn session_page(&self) -> usize {
        self.pager_page(3)
    }

    fn request_page(&self) -> usize {
        self.pager_page(5)
    }

    /// The session the request table is filtered to.
    fn selected_id(&self) -> Option<&str> {
        let index = self.session_page() * SESSION_PAGE + self.selected_session;
        self.session_id(index)
    }

    fn rebuild_selected(&mut self) {
        let Some(report) = self.report.as_ref() else {
            self.selected_records.clear();
            return;
        };
        let id = self.selected_id().map(str::to_owned);
        self.selected_records = self
            .record_indices
            .iter()
            .copied()
            .filter(|index| {
                report
                    .records
                    .get(*index)
                    .is_some_and(|record| Some(record.session.as_str()) == id.as_deref())
            })
            .collect();
    }

    fn rebuild_session_pager(&mut self) {
        let total = self.sessions.len().div_ceil(SESSION_PAGE).max(1);
        if let Some(pager) = self
            .children
            .get_mut(3)
            .and_then(|node| node.view.as_any().downcast_mut::<Pagination>())
        {
            if pager.total_pages != total {
                pager.set_total_pages(total);
            }
            pager.go_to(pager.page().min(total - 1));
        }
    }

    fn rebuild_request_table(&mut self) {
        let total = self.selected_records.len().div_ceil(REQUEST_PAGE).max(1);
        if let Some(pager) = self
            .children
            .get_mut(5)
            .and_then(|node| node.view.as_any().downcast_mut::<Pagination>())
        {
            if pager.total_pages != total {
                pager.set_total_pages(total);
            }
            pager.go_to(pager.page().min(total - 1));
        }
        let start = self.request_page() * REQUEST_PAGE;
        let rows: Vec<Node> = self
            .report
            .as_ref()
            .map(|report| {
                self.selected_records
                    .iter()
                    .copied()
                    .skip(start)
                    .take(REQUEST_PAGE)
                    .enumerate()
                    .filter_map(|(index, record_index)| {
                        let record = report.records.get(record_index)?;
                        let time = record.started_at.get(11..16).unwrap_or(&record.started_at);
                        let tokens =
                            record.input_tokens.unwrap_or(0) + record.output_tokens.unwrap_or(0);
                        Some(node(
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
                                node(
                                    "cost",
                                    TableCell::text(record.cost_usd.map_or_else(
                                        || "n/a".to_owned(),
                                        |cost| format!("{cost:.4}"),
                                    ))
                                    .with_weight(3),
                                ),
                            ]),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(table) = self
            .children
            .get_mut(4)
            .and_then(|node| node.view.as_any().downcast_mut::<Table>())
        {
            table.set_rows(rows);
            table.state.selected = 0;
        }
        self.last_request_row = 0;
    }

    fn rebuild_leaderboard(&mut self) {
        let records: &[norte_code_wire::TelemetryRecord] = self
            .report
            .as_ref()
            .map_or(&[], |report| report.records.as_slice());
        self.leaderboard.rebuild(records);
    }

    pub(crate) fn sync_detail(&mut self) {
        let fields = if self.detail_is_request {
            let selected = self
                .children
                .get(4)
                .and_then(|node| node.view.as_any_ref().downcast_ref::<Table>())
                .map_or(0, Table::selected);
            let index = self.request_page() * REQUEST_PAGE + selected;
            self.selected_record(index).map_or_else(
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
                        Field::new("tools", record.tool_calls.len().to_string()),
                    ]
                },
            )
        } else {
            let id = self.selected_id().unwrap_or("");
            let empty = SessionUsage::default();
            self.session_data
                .iter()
                .find(|session| session.session_id == id)
                .map_or_else(
                    || vec![Field::new("no session", "no summary")],
                    |session| {
                        super::session_detail::fields(session, self.usage.get(id).unwrap_or(&empty))
                    },
                )
        };
        let title = if self.detail_is_request {
            "request"
        } else {
            "session"
        };
        if let Some(panel) = self
            .children
            .get_mut(6)
            .and_then(|node| node.view.as_any().downcast_mut::<DetailPanel>())
        {
            panel.set(title, fields);
        }
    }

    /// Select a session by its absolute index, moving both the pager and the
    /// highlighted row so they never disagree.
    fn select_session(&mut self, absolute: usize) {
        let page = absolute / SESSION_PAGE;
        self.selected_session = absolute.saturating_sub(page * SESSION_PAGE);
        self.detail_is_request = false;
        self.view = TelemetryView::Sessions;
        if let Some(pager) = self
            .children
            .get_mut(3)
            .and_then(|node| node.view.as_any().downcast_mut::<Pagination>())
        {
            pager.go_to(page);
        }
        self.last_session_page = page;
        if let Some(pager) = self
            .children
            .get_mut(5)
            .and_then(|node| node.view.as_any().downcast_mut::<Pagination>())
        {
            pager.go_to(0);
        }
        self.last_request_page = 0;
        self.rebuild_selected();
        self.rebuild_request_table();
        self.sync_detail();
    }

    /// Move the highlighted request row within the selected session.
    fn move_request(&mut self, delta: isize) {
        let total = self.selected_records.len();
        let max = total
            .saturating_sub(self.request_page() * REQUEST_PAGE)
            .min(REQUEST_PAGE);
        if max == 0 {
            return;
        }
        let Some(table) = self
            .children
            .get_mut(4)
            .and_then(|node| node.view.as_any().downcast_mut::<Table>())
        else {
            return;
        };
        let current = isize::try_from(table.state.selected).unwrap_or(isize::MAX);
        let last = isize::try_from(max.saturating_sub(1)).unwrap_or(isize::MAX);
        let next = usize::try_from((current + delta).clamp(0, last)).unwrap_or(0);
        table.state.selected = next;
        self.detail_is_request = true;
        self.view = TelemetryView::Requests;
        self.last_request_row = next;
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
                " · {} req · {} in / {} out · ${:.4}",
                summary.requests, summary.input_tokens, summary.output_tokens, summary.cost_usd
            );
        }
        caption
    }

    fn paint_charts(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let titles = [
            "cost · cumulative usd",
            "tokens · in + out",
            "cache · hit rate %",
        ];
        self.chart_scroll.set_viewport(area.height, CHART_ROWS);
        let offset = self.chart_scroll.offset();
        self.chart_areas.clear();
        let mut page = Buffer::empty(Rect::new(0, 0, area.width, CHART_ROWS));
        let bold = Style::default()
            .fg(theme.foreground)
            .bg(theme.background)
            .add_modifier(Modifier::BOLD);
        let body_y = 1u16;
        page.set_stringn(0, 0, titles[0], titles[0].len(), bold);
        let body = Rect {
            x: 0,
            y: body_y,
            width: area.width,
            height: CHART_SLOT - 1,
        };
        if let Some(child) = self.children.first_mut() {
            child.view.render(body, &mut page, cx, theme);
            if let Some(chart) = child.view.as_any().downcast_mut::<CandleChart>() {
                chart.state.area = Rect {
                    x: area.x,
                    y: area.y + body_y.saturating_sub(offset),
                    width: area.width,
                    height: CHART_SLOT - 1,
                };
            }
        }
        self.chart_areas.push(Rect {
            x: area.x,
            y: area.y + body_y.saturating_sub(offset),
            width: area.width,
            height: CHART_SLOT - 1,
        });
        let window = self.chart_window();
        self.sync_line_window(window);
        for (index, title) in titles.iter().enumerate().skip(1) {
            let y = u16::try_from(index).unwrap_or(u16::MAX) * (CHART_SLOT + CHART_GAP);
            page.set_stringn(0, y, title, title.len(), bold);
            let body_y = y + 1;
            let body = Rect {
                x: 0,
                y: body_y,
                width: area.width,
                height: CHART_SLOT - 1,
            };
            if let Some(child) = self.children.get_mut(index) {
                child.view.render(body, &mut page, cx, theme);
            }
            self.paint_time_labels(
                &mut page,
                body_y + CHART_SLOT - 2,
                area.width,
                window,
                theme,
            );
            self.chart_areas.push(Rect {
                x: area.x,
                y: area.y + body_y.saturating_sub(offset),
                width: area.width,
                height: CHART_SLOT - 1,
            });
        }
        for row in 0..area.height {
            let source = row.saturating_add(offset);
            if source >= CHART_ROWS {
                break;
            }
            for column in 0..area.width {
                if let (Some(cell), Some(source_cell)) = (
                    buf.cell_mut(RatatuiPosition::new(area.x + column, area.y + row)),
                    page.cell(RatatuiPosition::new(column, source)),
                ) {
                    *cell = source_cell.clone();
                }
            }
        }
        self.chart_scroll.paint(area, buf, theme, CHART_ROWS);
    }

    fn paint_sessions(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let start = self.session_page() * SESSION_PAGE;
        for (offset, index) in (start..start + SESSION_PAGE)
            .take(area.height as usize)
            .enumerate()
        {
            if let Some(id) = self.session_id(index) {
                let y = area
                    .y
                    .saturating_add(u16::try_from(offset).unwrap_or(u16::MAX));
                let selected = index == self.session_page() * SESSION_PAGE + self.selected_session;
                if selected {
                    buf.set_style(
                        Rect {
                            x: area.x,
                            y,
                            width: area.width,
                            height: 1,
                        },
                        Style::default().bg(theme.secondary),
                    );
                }
                buf.set_stringn(
                    area.x + 1,
                    y,
                    clip(id, usize::from(area.width.saturating_sub(2))),
                    usize::from(area.width.saturating_sub(2)),
                    Style::default().fg(theme.foreground),
                );
            }
        }
        if self.sessions.is_empty() {
            buf.set_stringn(
                area.x + 1,
                area.y,
                "no sessions in telemetry",
                24,
                Style::default().fg(theme.muted),
            );
        }
    }

    fn paint_leaderboards(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
    ) {
        self.leaderboard.render(area, buf, cx, theme);
    }

    fn paint_lower(
        &mut self,
        area: Rect,
        lower_y: u16,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
    ) {
        let detail_width = (area.width / 2)
            .clamp(30, 52)
            .min(area.width.saturating_sub(32));
        let left_width = area.width.saturating_sub(detail_width + 1);
        let lower_h = area.y + area.height - lower_y;

        let mut sessions_style = Style::default().fg(theme.muted);
        let mut requests_style = Style::default().fg(theme.muted);
        let active = if self.view == TelemetryView::Sessions {
            &mut sessions_style
        } else {
            &mut requests_style
        };
        *active = Style::default()
            .fg(theme.foreground)
            .add_modifier(Modifier::BOLD);
        buf.set_stringn(area.x + 1, lower_y, "sessions", 8, sessions_style);
        let sessions_rect = Rect {
            x: area.x,
            y: lower_y + 1,
            width: left_width,
            height: 4,
        };
        self.paint_sessions(sessions_rect, buf, theme);
        let session_pager = Rect {
            x: area.x,
            y: lower_y + 5,
            width: left_width,
            height: 1,
        };
        if let Some(child) = self.children.get_mut(3) {
            child.view.render(session_pager, buf, cx, theme);
        }

        let sess_page = self.session_page();
        if sess_page != self.last_session_page {
            self.last_session_page = sess_page;
            self.selected_session = 0;
            self.detail_is_request = false;
            self.rebuild_selected();
            self.rebuild_request_table();
        }

        let req_title_y = lower_y + 6;
        buf.set_stringn(
            area.x + 1,
            req_title_y,
            "requests · selected session",
            27,
            requests_style,
        );
        let table_rect = Rect {
            x: area.x,
            y: req_title_y + 1,
            width: left_width,
            height: lower_h.saturating_sub(8),
        };
        let request_pager = Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(1),
            width: left_width,
            height: 1,
        };
        let detail_rect = Rect {
            x: area.x + left_width + 1,
            y: lower_y,
            width: detail_width,
            height: lower_h,
        };

        let req_page = self.request_page();
        if req_page != self.last_request_page {
            self.last_request_page = req_page;
            self.rebuild_request_table();
        }
        let request_row = self
            .children
            .get(4)
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Table>())
            .map_or(0, Table::selected);
        if request_row != self.last_request_row {
            self.last_request_row = request_row;
            self.detail_is_request = true;
        }
        self.sync_detail();
        if let Some(child) = self.children.get_mut(4) {
            child.view.render(table_rect, buf, cx, theme);
        }
        if let Some(child) = self.children.get_mut(5) {
            child.view.render(request_pager, buf, cx, theme);
        }
        if let Some(child) = self.children.get_mut(6) {
            child.view.render(detail_rect, buf, cx, theme);
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Option<Outcome> {
        use crossterm::event::MouseEventKind;
        match self.view {
            TelemetryView::Charts => {
                if self.chart_scroll.handle_mouse(mouse, CHART_ROWS) {
                    return Some(Outcome::Consumed);
                }
                if !matches!(
                    mouse.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                ) {
                    return None;
                }
                let chart = self.chart_areas.iter().position(|area| {
                    mouse.column >= area.x
                        && mouse.column < area.x.saturating_add(area.width)
                        && mouse.row >= area.y
                        && mouse.row < area.y.saturating_add(area.height)
                });
                if chart.is_none_or(|index| index == 0) {
                    return None;
                }
                let delta = if mouse.kind == MouseEventKind::ScrollUp {
                    -3
                } else {
                    3
                };
                self.scroll_chart_history(delta);
                Some(Outcome::Consumed)
            }
            TelemetryView::Leaderboards => match self.leaderboard.handle_mouse(mouse) {
                LeaderboardClick::Tab => {
                    self.rebuild_leaderboard();
                    Some(Outcome::Consumed)
                }
                LeaderboardClick::Row => Some(Outcome::Consumed),
                LeaderboardClick::None => None,
            },
            TelemetryView::Sessions | TelemetryView::Requests => None,
        }
    }

    fn handle_leaderboard_key(&mut self, code: crossterm::event::KeyCode) -> Option<Outcome> {
        use crossterm::event::KeyCode;
        let consumed = match code {
            KeyCode::Char('1') => {
                self.leaderboard.select_usage();
                true
            }
            KeyCode::Char('2') => {
                self.leaderboard.select_cost();
                true
            }
            KeyCode::Up => {
                self.leaderboard.cycle_group(true);
                true
            }
            KeyCode::Down => {
                self.leaderboard.cycle_group(false);
                true
            }
            KeyCode::Left => {
                self.leaderboard.cycle_subject(true);
                true
            }
            KeyCode::Right => {
                self.leaderboard.cycle_subject(false);
                true
            }
            KeyCode::Char('j') => {
                self.leaderboard.move_selection(1);
                return Some(Outcome::Consumed);
            }
            KeyCode::Char('k') => {
                self.leaderboard.move_selection(-1);
                return Some(Outcome::Consumed);
            }
            _ => false,
        };
        if consumed {
            self.rebuild_leaderboard();
            Some(Outcome::Consumed)
        } else {
            None
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
        let hint = match self.view {
            TelemetryView::Charts => {
                "wheel charts history | up/down stack | click bar to scroll | [ ] interval"
            }
            TelemetryView::Leaderboards => {
                "up/down group, left/right entity, j/k rows | [ ] interval"
            }
            TelemetryView::Sessions | TelemetryView::Requests => "j/k select | [ ] interval",
        };
        if area.width > u16::try_from(hint.len()).unwrap_or(u16::MAX) {
            let w = u16::try_from(hint.len()).unwrap_or(u16::MAX);
            buf.set_stringn(
                area.x + area.width.saturating_sub(w + 1),
                area.y,
                hint,
                hint.len(),
                Style::default().fg(theme.muted),
            );
        }
        let body = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        match self.view {
            TelemetryView::Charts => self.paint_charts(body, buf, cx, theme),
            TelemetryView::Leaderboards => self.paint_leaderboards(body, buf, cx, theme),
            TelemetryView::Sessions | TelemetryView::Requests => {
                self.paint_lower(area, area.y + 1, buf, cx, theme);
            }
        }
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        use crossterm::event::KeyCode;
        if let InputEvent::Mouse(mouse) = event
            && let Some(outcome) = self.handle_mouse(*mouse)
        {
            return outcome;
        }
        if let InputEvent::Key(key) = event
            && key.kind == crossterm::event::KeyEventKind::Press
        {
            if self.view == TelemetryView::Leaderboards
                && let Some(outcome) = self.handle_leaderboard_key(key.code)
            {
                return outcome;
            }
            match key.code {
                KeyCode::Up | KeyCode::PageUp if self.view == TelemetryView::Charts => {
                    self.scroll_chart_view(if key.code == KeyCode::Up { -3 } else { -6 });
                    return Outcome::Consumed;
                }
                KeyCode::Down | KeyCode::PageDown if self.view == TelemetryView::Charts => {
                    self.scroll_chart_view(if key.code == KeyCode::Down { 3 } else { 6 });
                    return Outcome::Consumed;
                }
                KeyCode::Char('[') => {
                    self.cycle_interval(true);
                    return Outcome::Consumed;
                }
                KeyCode::Char(']') => {
                    self.cycle_interval(false);
                    return Outcome::Consumed;
                }
                KeyCode::Char('j') | KeyCode::Down
                    if self.view != TelemetryView::Charts
                        && self.view != TelemetryView::Leaderboards =>
                {
                    if self.view == TelemetryView::Sessions {
                        let current = self.session_page() * SESSION_PAGE + self.selected_session;
                        let next = (current + 1).min(self.sessions.len().saturating_sub(1));
                        self.select_session(next);
                    } else {
                        self.move_request(1);
                        self.sync_detail();
                    }
                    return Outcome::Consumed;
                }
                KeyCode::Char('k') | KeyCode::Up
                    if self.view != TelemetryView::Charts
                        && self.view != TelemetryView::Leaderboards =>
                {
                    if self.view == TelemetryView::Sessions {
                        let current = self.session_page() * SESSION_PAGE + self.selected_session;
                        self.select_session(current.saturating_sub(1));
                    } else {
                        self.move_request(-1);
                        self.sync_detail();
                    }
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
