//! The telemetry tab: charts, a session pager, and that session's requests.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde::Serialize;
use terminalui::components::candlechart::CandleChart;
use terminalui::components::linechart::LineChart;
use terminalui::components::pagination::Pagination;
use terminalui::components::piechart::PieChart;
use terminalui::components::presentation::{DotSize, RenderStyle, StyleTuning};
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Node, node};

use super::detail_panel::DetailPanel;
use super::leaderboard_section::LeaderboardSection;
use crate::session_stats::SharedUsage;
use crate::shared_telemetry::SharedTelemetry;
use crate::telemetry::ChartInterval;
use crate::telemetry_series::Series;
use crate::usage_calendar::{CALENDAR_WEEKS, UsageCalendar};
use chart_scroll::ChartScroll;

#[path = "telemetry_chart_scroll.rs"]
mod chart_scroll;
#[path = "telemetry_page_charts.rs"]
mod charts;
#[path = "telemetry_page_heatmap.rs"]
mod heatmap;
#[cfg(test)]
#[path = "telemetry_page_heatmap_tests.rs"]
mod heatmap_tests;
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
pub(crate) const CHART_SLOT: u16 = 18;
pub(crate) const CHART_GAP: u16 = 2;
pub(crate) const CHART_ROWS: u16 = CHART_SLOT * 9 + CHART_GAP * 8;
/// Month labels plus the seven weekday rows of the usage heatmap.
const HEATMAP_ROWS: u16 = 8;
pub(crate) const LINE_AXIS_WIDTH: u16 = 9;
const CHART_TITLES: [&str; 9] = [
    "cost · cumulative usd",
    "input · tokens",
    "output · tokens",
    "cache · hit rate %",
    "requests · per bucket",
    "latency · average ms",
    "first output · average ms",
    "first visible text · average ms",
    "cost mix · provider + model",
];

/// Which view Tab currently shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(super) enum TelemetryView {
    /// Time series and spend composition.
    #[default]
    Charts,
    /// The paginated session list and its detail.
    Sessions,
    /// The selected session's requests and their detail.
    Requests,
    /// Rankings of models, providers and sessions.
    Leaderboards,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ChartGlyphs {
    #[default]
    Braille,
    Halftone,
    Blocks,
    Lines,
}

impl ChartGlyphs {
    fn render_style(self) -> RenderStyle {
        match self {
            Self::Braille => RenderStyle::Braille,
            Self::Halftone => RenderStyle::Halftone,
            Self::Blocks => RenderStyle::Blocks,
            Self::Lines => RenderStyle::Lines,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Braille => "braille",
            Self::Halftone => "dots",
            Self::Blocks => "blocks",
            Self::Lines => "lines",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Braille => Self::Halftone,
            Self::Halftone => Self::Blocks,
            Self::Blocks => Self::Lines,
            Self::Lines => Self::Braille,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct TelemetryPage {
    interval: ChartInterval,
    chart_glyphs: ChartGlyphs,
    #[serde(skip)]
    chart_tuning: StyleTuning,
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
    pub(super) view: TelemetryView,
    /// Scroll offset and rail of the tall stacked charts.
    #[serde(skip)]
    chart_scroll: ChartScroll,
    #[serde(skip)]
    transition: Option<terminalui::Animation>,
    #[serde(skip)]
    chart_grid: bool,
    /// Screen areas of the three chart bodies, for wheel hit-testing.
    #[serde(skip)]
    chart_areas: Vec<Rect>,
    /// Full series behind the charts, sliced for the aligned time window.
    #[serde(skip)]
    chart_data: Series,
    #[serde(skip)]
    additional_charts: Vec<LineChart>,
    #[serde(skip)]
    has_cost_slices: bool,
    provider_cost_chart: PieChart,
    #[serde(skip)]
    model_cost_chart: PieChart,
    /// Short time labels shared by all three x axes.
    #[serde(skip)]
    bucket_labels: Arc<Vec<String>>,
    /// Window last pushed into the two line charts.
    #[serde(skip)]
    line_window: Option<(usize, usize)>,
    /// Reused composition surface for the stacked charts.
    ///
    /// The charts are painted into this off-screen buffer first and then
    /// blitted row by row into the real frame, because the three charts
    /// share one vertical scroll offset and are taller than the visible
    /// area. Composing off-screen avoids redrawing and keeps the scroll
    /// blit a single slice copy.
    #[serde(skip)]
    chart_page: Buffer,
    /// Trailing year of burned tokens, bucketed by calendar day.
    #[serde(skip)]
    usage_calendar: UsageCalendar,
    /// Screen area and visible width of the heatmap drawn above the charts.
    #[serde(skip)]
    heatmap_area: Rect,
    #[serde(skip)]
    heatmap_weeks: usize,
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
            chart_glyphs: ChartGlyphs::default(),
            chart_tuning: StyleTuning {
                density: 3,
                contrast: 1.25,
                dotsize: DotSize::Small,
            },
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
            transition: None,
            chart_grid: false,
            chart_areas: Vec::new(),
            chart_data: Series::default(),
            additional_charts: vec![
                LineChart::new(),
                LineChart::new(),
                LineChart::new(),
                LineChart::new(),
                LineChart::new(),
            ],
            has_cost_slices: false,
            provider_cost_chart: PieChart::new(),
            model_cost_chart: PieChart::new(),
            bucket_labels: Arc::new(Vec::new()),
            line_window: None,
            chart_page: Buffer::empty(Rect::new(0, 0, 0, CHART_ROWS)),
            usage_calendar: UsageCalendar::from_report(None, Utc::now().date_naive()),
            heatmap_area: Rect::default(),
            heatmap_weeks: CALENDAR_WEEKS,
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
        self.detail_is_request = self.view == TelemetryView::Requests;
        self.sync_detail();
        self.start_transition();
    }

    fn start_transition(&mut self) {
        self.transition = Some(terminalui::Animation::new(
            Duration::from_millis(180),
            terminalui::Easing::EaseOutCubic,
        ));
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
        self.usage_calendar =
            UsageCalendar::from_report(self.report.as_deref(), Utc::now().date_naive());
        let start = self.report.as_ref().map_or(0, |report| {
            report.records.len().saturating_sub(MAX_RECORD_WINDOW)
        });
        self.record_indices = self.report.as_ref().map_or_else(Vec::new, |report| {
            (start..report.records.len()).rev().collect()
        });
        self.sessions = if let Some(report) = self.report.as_ref() {
            let mut seen = HashSet::new();
            let sessions: Vec<usize> = report
                .records
                .iter()
                .enumerate()
                .skip(start)
                .rev()
                .filter(|(_, record)| seen.insert(record.session.as_str()))
                .map(|(index, _)| index)
                .collect();
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
