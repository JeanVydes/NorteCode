//! The rankings view inside the telemetry tab.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Context, Theme, View, node};

use crate::leaderboard::{Leaderboard, LeaderboardGroup, LeaderboardSubject};
use norte_code_wire::TelemetryRecord;

/// Owns the six rankings, their tabs and their selection.
pub(crate) struct LeaderboardSection {
    group: LeaderboardGroup,
    subject: LeaderboardSubject,
    board: Leaderboard,
    selected: usize,
    table: Table,
}

impl LeaderboardSection {
    pub(crate) fn new() -> Self {
        Self {
            group: LeaderboardGroup::Usage,
            subject: LeaderboardSubject::Model,
            board: Leaderboard::default(),
            selected: 0,
            table: Table::new().with_header(node(
                "header",
                TableRow::header(vec![
                    node("h-name", TableCell::text("name").with_weight(8)),
                    node("h-req", TableCell::text("req").with_weight(2)),
                    node("h-turns", TableCell::text("turns").with_weight(2)),
                    node("h-tokens", TableCell::text("in/out").with_weight(3)),
                    node("h-cost", TableCell::text("usd").with_weight(3)),
                ]),
            )),
        }
    }

    pub(crate) fn rebuild(&mut self, records: &[TelemetryRecord]) {
        self.board = Leaderboard::build(records, self.group, self.subject);
        self.selected = 0;
        let rows = self
            .board
            .rows
            .iter()
            .map(|row| {
                node(
                    "row",
                    TableRow::body(vec![
                        node("name", TableCell::text(row.name.clone()).with_weight(8)),
                        node(
                            "req",
                            TableCell::text(row.requests.to_string()).with_weight(2),
                        ),
                        node(
                            "turns",
                            TableCell::text(row.turns.to_string()).with_weight(2),
                        ),
                        node(
                            "tokens",
                            TableCell::text(format!(
                                "{} / {}",
                                row.input_tokens, row.output_tokens
                            ))
                            .with_weight(3),
                        ),
                        node(
                            "cost",
                            TableCell::text(format!("{:.4}", row.cost_usd)).with_weight(3),
                        ),
                    ]),
                )
            })
            .collect();
        self.table.set_rows(rows);
        self.table.state.selected = 0;
    }

    pub(crate) fn select_usage(&mut self) {
        self.group = LeaderboardGroup::Usage;
    }

    pub(crate) fn select_cost(&mut self) {
        self.group = LeaderboardGroup::Cost;
    }

    pub(crate) fn cycle_subject(&mut self, backwards: bool) {
        self.subject = if backwards {
            self.subject.previous()
        } else {
            self.subject.next()
        };
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let last = isize::try_from(self.board.rows.len().saturating_sub(1)).unwrap_or(isize::MAX);
        let current = isize::try_from(self.selected).unwrap_or(isize::MAX);
        self.selected = usize::try_from((current + delta).clamp(0, last)).unwrap_or(0);
        self.table.state.selected = self.selected;
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let active = Style::default()
            .fg(theme.foreground)
            .add_modifier(Modifier::BOLD);
        let idle = Style::default().fg(theme.muted);
        let title = format!(
            "rankings: {} / {}",
            self.group.label(),
            self.subject.label()
        );
        buf.set_stringn(
            area.x + 1,
            area.y,
            &title,
            usize::from(area.width.saturating_sub(2)),
            active,
        );
        let (usage, cost) = match self.group {
            LeaderboardGroup::Usage => (active, idle),
            LeaderboardGroup::Cost => (idle, active),
        };
        buf.set_stringn(area.x + 1, area.y + 1, "1 usage", 7, usage);
        buf.set_stringn(area.x + 10, area.y + 1, "2 cost", 6, cost);

        let (models, providers, sessions) = match self.subject {
            LeaderboardSubject::Model => (active, idle, idle),
            LeaderboardSubject::Provider => (idle, active, idle),
            LeaderboardSubject::Session => (idle, idle, active),
        };
        buf.set_stringn(area.x + 1, area.y + 2, "models", 6, models);
        buf.set_stringn(area.x + 10, area.y + 2, "providers", 9, providers);
        buf.set_stringn(area.x + 22, area.y + 2, "sessions", 8, sessions);

        let table_rect = Rect {
            x: area.x,
            y: area.y + 4,
            width: area.width,
            height: area.height.saturating_sub(4),
        };
        self.table.render(table_rect, buf, cx, theme);
        if self.board.rows.is_empty() {
            buf.set_stringn(
                area.x + 1,
                table_rect.y + 2,
                "no telemetry in this ranking",
                29,
                idle,
            );
        }
    }
}
