//! Telemetry lists: sessions, requests, detail, captions, and row painting.

use std::fmt::Write;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use terminalui::components::pagination::Pagination;
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Node, Theme, node};

use crate::session_stats::SessionUsage;
use crate::ui::detail_panel::{DetailPanel, Field};

use super::{REQUEST_PAGE, SESSION_PAGE, TelemetryPage, TelemetryView};

impl TelemetryPage {
    pub(super) fn record(&self, index: usize) -> Option<&norte_code_wire::TelemetryRecord> {
        self.report.as_ref()?.records.get(index)
    }

    pub(super) fn selected_record(
        &self,
        position: usize,
    ) -> Option<&norte_code_wire::TelemetryRecord> {
        self.selected_records
            .get(position)
            .copied()
            .and_then(|index| self.record(index))
    }

    pub(super) fn session_id(&self, position: usize) -> Option<&str> {
        self.sessions
            .get(position)
            .copied()
            .and_then(|index| self.record(index))
            .map(|record| record.session.as_str())
    }

    pub(super) fn pager_page(&self, index: usize) -> usize {
        self.children
            .get(index)
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Pagination>())
            .map_or(0, Pagination::page)
    }

    pub(super) fn session_page(&self) -> usize {
        self.pager_page(3)
    }

    pub(super) fn request_page(&self) -> usize {
        self.pager_page(5)
    }

    /// The session the request table is filtered to.
    pub(super) fn selected_id(&self) -> Option<&str> {
        let index = self.session_page() * SESSION_PAGE + self.selected_session;
        self.session_id(index)
    }

    pub(super) fn rebuild_selected(&mut self) {
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

    pub(super) fn rebuild_session_pager(&mut self) {
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

    pub(super) fn rebuild_request_table(&mut self) {
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
                                    TableCell::text(crate::ui::clip(&record.model, 20))
                                        .with_weight(5),
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
                        Field::new("id", crate::ui::clip(&record.id, 34)),
                        Field::new("model", crate::ui::clip(&record.model, 34)),
                        Field::new("provider", crate::ui::clip(&record.provider, 34)),
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
                        crate::ui::session_detail::fields(
                            session,
                            self.usage.get(id).unwrap_or(&empty),
                        )
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
    pub(super) fn select_session(&mut self, absolute: usize) {
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
    pub(super) fn move_request(&mut self, delta: isize) {
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

    pub(super) fn caption(&self) -> String {
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

    pub(super) fn paint_sessions(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
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
                    crate::ui::clip(id, usize::from(area.width.saturating_sub(2))),
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
}

fn status_label(status: norte_code_wire::RequestStatus) -> &'static str {
    match status {
        norte_code_wire::RequestStatus::Ok => "ok",
        norte_code_wire::RequestStatus::Error => "failed",
        norte_code_wire::RequestStatus::Cancelled => "cancelled",
    }
}
