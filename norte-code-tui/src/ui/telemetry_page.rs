//! The telemetry tab: charts, a session pager, and that session's requests.

use std::collections::HashSet;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde::Serialize;
use terminalui::components::candlechart::CandleChart;
use terminalui::components::linechart::LineChart;
use terminalui::components::pagination::Pagination;
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Node, node};

use super::detail_panel::DetailPanel;
use super::leaderboard_section::LeaderboardSection;
use crate::session_stats::SharedUsage;
use crate::shared_telemetry::SharedTelemetry;
use crate::telemetry::ChartInterval;
use crate::telemetry_series::Series;
use chart_scroll::ChartScroll;

#[path = "telemetry_chart_scroll.rs"]
mod chart_scroll;
#[path = "telemetry_page_charts.rs"]
mod charts;
#[path = "telemetry_page_leaderboards.rs"]
mod leaderboards;
#[path = "telemetry_page_lists.rs"]
mod lists;
#[path = "telemetry_page_view.rs"]
mod view;

/// Sessions and requests shown per page.
const SESSION_PAGE: usize = 4;
const REQUEST_PAGE: usize = 6;
/// Newest records exposed to the paginated lists and rankings.
pub(crate) const MAX_RECORD_WINDOW: usize = 1_000;
const CHART_SLOT: u16 = 18;
const CHART_GAP: u16 = 2;
const CHART_ROWS: u16 = CHART_SLOT * 3 + CHART_GAP * 2;
const LINE_AXIS_WIDTH: u16 = 9;
const CHART_TITLES: [&str; 3] = [
    "cost · cumulative usd",
    "tokens · in + out",
    "cache · hit rate %",
];

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
    /// Reused composition surface for the stacked charts.
    #[serde(skip)]
    chart_page: Buffer,
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
            chart_page: Buffer::empty(Rect::new(0, 0, 0, CHART_ROWS)),
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
        if self.session_data == sessions {
            return;
        }
        self.session_data = sessions;
        self.sync_detail();
    }

    /// Replace the report the charts, session list and request table use.
    pub(crate) fn set_report(&mut self, report: Option<SharedTelemetry>, usage: SharedUsage) {
        let unchanged = match (&self.report, &report) {
            (Some(current), Some(next)) => Arc::ptr_eq(current, next),
            (None, None) => true,
            _ => false,
        } && Arc::ptr_eq(&self.usage, &usage);
        if unchanged {
            return;
        }
        self.report = report;
        self.usage = usage;
        let start = self.report.as_ref().map_or(0, |report| {
            report.records.len().saturating_sub(MAX_RECORD_WINDOW)
        });
        self.record_indices = self.report.as_ref().map_or_else(Vec::new, |report| {
            (start..report.records.len()).rev().collect()
        });
        self.sessions = if let Some(report) = self.report.as_ref() {
            let mut seen = HashSet::new();
            let mut sessions: Vec<usize> = report
                .records
                .iter()
                .enumerate()
                .skip(start)
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
        } else {
            Vec::new()
        };
        self.selected_session = 0;
        self.detail_is_request = false;
        self.rebuild_selected();
        self.rebuild_charts();
        self.rebuild_session_pager();
        self.rebuild_request_table();
        self.rebuild_leaderboard();
        self.sync_detail();
    }
}
