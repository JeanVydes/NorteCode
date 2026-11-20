//! The telemetry tab: charts, a session pager, and that session's requests.

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

/// Sessions and requests shown per page.
const SESSION_PAGE: usize = 4;
const REQUEST_PAGE: usize = 6;

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
}

/// The charts share one interval so their x axes line up.
#[derive(Serialize)]
pub(crate) struct TelemetryPage {
    interval: ChartInterval,
    report: Option<norte_code_wire::TelemetryReport>,
    /// All records, newest first.
    pub(crate) records: Vec<norte_code_wire::TelemetryRecord>,
    /// Rollup computed once per report, not on every frame.
    usage: std::collections::BTreeMap<String, crate::session_stats::SessionUsage>,
    /// The selected session's records, rebuilt only when the session changes.
    selected_records: Vec<norte_code_wire::TelemetryRecord>,
    /// Distinct session ids, newest activity first.
    sessions: Vec<String>,
    /// Session summaries from the host, for the shared detail panel.
    session_data: Vec<norte_code_wire::SessionSummary>,
    /// The session whose requests are shown.
    selected_session: usize,
    /// Which panel the detail shows.
    pub(crate) detail_is_request: bool,
    /// Which view is on screen.
    view: TelemetryView,
    /// Pages the request table was last rebuilt for.
    last_session_page: usize,
    last_request_page: usize,
    /// Last highlighted request row, to switch the panel on selection.
    last_request_row: usize,
    pub(crate) children: Vec<Node>,
}

impl TelemetryPage {
    /// A page with empty charts, filled by the host when data arrives.
    pub(crate) fn new() -> Self {
        Self {
            interval: ChartInterval::Hour,
            report: None,
            records: Vec::new(),
            usage: std::collections::BTreeMap::new(),
            selected_records: Vec::new(),
            sessions: Vec::new(),
            session_data: Vec::new(),
            selected_session: 0,
            detail_is_request: false,
            view: TelemetryView::Charts,
            last_session_page: 0,
            last_request_page: 0,
            last_request_row: 0,
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
                        ]),
                    )),
                ),
                node("request_pager", Pagination::new(1)),
                node("request_detail", DetailPanel::new()),
            ],
        }
    }

    /// Tab cycles charts, then the two paginated lists.
    pub(crate) fn toggle_focus(&mut self) {
        self.view = match self.view {
            TelemetryView::Charts => TelemetryView::Sessions,
            TelemetryView::Sessions => TelemetryView::Requests,
            TelemetryView::Requests => TelemetryView::Charts,
        };
    }

    /// Replace the session summaries used by the shared detail panel.
    pub(crate) fn set_sessions(&mut self, sessions: Vec<norte_code_wire::SessionSummary>) {
        self.session_data = sessions;
        self.sync_detail();
    }

    /// Replace the report the charts, session list and request table use.
    pub(crate) fn set_report(&mut self, report: Option<norte_code_wire::TelemetryReport>) {
        self.report = report;
        self.records = self
            .report
            .as_ref()
            .map(|report| report.records.iter().rev().cloned().collect())
            .unwrap_or_default();
        let mut sessions: Vec<String> = self
            .records
            .iter()
            .map(|record| record.session.clone())
            .collect();
        sessions.sort();
        sessions.dedup();
        self.sessions = sessions;
        self.usage = crate::session_stats::by_session(&self.records);
        self.selected_session = 0;
        self.detail_is_request = false;
        self.rebuild_selected();
        self.rebuild_charts();
        self.rebuild_session_pager();
        self.rebuild_request_table();
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
        self.sessions.get(index).map(String::as_str)
    }

    fn rebuild_selected(&mut self) {
        let id = self.selected_id().map(str::to_owned);
        self.selected_records = self
            .records
            .iter()
            .filter(|record| Some(record.session.as_str()) == id.as_deref())
            .cloned()
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
            .selected_records
            .iter()
            .skip(start)
            .take(REQUEST_PAGE)
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
            .get_mut(4)
            .and_then(|node| node.view.as_any().downcast_mut::<Table>())
        {
            table.set_rows(rows);
            table.state.selected = 0;
        }
        self.last_request_row = 0;
    }

    pub(crate) fn sync_detail(&mut self) {
        let fields = if self.detail_is_request {
            let selected = self
                .children
                .get(4)
                .and_then(|node| node.view.as_any_ref().downcast_ref::<Table>())
                .map_or(0, Table::selected);
            let index = self.request_page() * REQUEST_PAGE + selected;
            self.selected_records.get(index).map_or_else(
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
            let empty = crate::session_stats::SessionUsage::default();
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
                " · {} req · {} in / {} out",
                summary.requests, summary.input_tokens, summary.output_tokens
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
        let width = area.width / 3;
        for (index, title) in titles.iter().enumerate() {
            let step = u16::try_from(index).unwrap_or(0);
            let x = area.x.saturating_add(width.saturating_mul(step));
            let w = if index == 2 {
                area.width.saturating_sub(width * 2)
            } else {
                width
            };
            buf.set_stringn(
                x + 1,
                area.y,
                title,
                title.len(),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            );
            let plot = Rect {
                x,
                y: area.y + 1,
                width: w,
                height: area.height.saturating_sub(1),
            };
            if let Some(child) = self.children.get_mut(index) {
                child.view.render(plot, buf, cx, theme);
            }
        }
    }

    fn paint_sessions(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let start = self.session_page() * SESSION_PAGE;
        for (offset, index) in (start..start + SESSION_PAGE)
            .take(area.height as usize)
            .enumerate()
        {
            if let Some(id) = self.sessions.get(index) {
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
        let hint = "Tab charts/sessions/requests · j/k navigate · [ ] interval";
        if area.width > 56 {
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
        if self.view == TelemetryView::Charts {
            self.paint_charts(body, buf, cx, theme);
        } else {
            self.paint_lower(area, area.y + 1, buf, cx, theme);
        }
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        use crossterm::event::KeyCode;
        if let InputEvent::Key(key) = event
            && key.kind == crossterm::event::KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('[') => {
                    self.cycle_interval(true);
                    return Outcome::Consumed;
                }
                KeyCode::Char(']') => {
                    self.cycle_interval(false);
                    return Outcome::Consumed;
                }
                KeyCode::Char('j') | KeyCode::Down if self.view != TelemetryView::Charts => {
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
                KeyCode::Char('k') | KeyCode::Up if self.view != TelemetryView::Charts => {
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
