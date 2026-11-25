//! The rankings view inside the telemetry tab.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Context, Theme, View, node};

use crate::leaderboard::{Leaderboard, LeaderboardGroup, LeaderboardSubject};
use norte_code_wire::TelemetryRecord;

/// What a click changed in the rankings view.
pub(crate) enum LeaderboardClick {
    None,
    Tab,
    Row,
}

/// Owns the six rankings, their tabs and their selection.
pub(crate) struct LeaderboardSection {
    group: LeaderboardGroup,
    subject: LeaderboardSubject,
    board: Leaderboard,
    selected: usize,
    table: Table,
    area: Rect,
}

impl LeaderboardSection {
    pub(crate) fn new() -> Self {
        Self {
            group: LeaderboardGroup::Usage,
            subject: LeaderboardSubject::Model,
            board: Leaderboard::default(),
            selected: 0,
            area: Rect::new(0, 0, 0, 0),
            table: Table::new().with_header(node(
                "header",
                TableRow::header(vec![
                    node("h-name", TableCell::text("name").with_weight(8)),
                    node("h-req", TableCell::text("req").with_weight(2)),
                    node("h-turns", TableCell::text("turns").with_weight(2)),
                    node("h-tokens", TableCell::text("in/out").with_weight(3)),
                    node("h-value", TableCell::text("value").with_weight(3)),
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
                            "value",
                            TableCell::text(metric_value_label(self.group, row)).with_weight(3),
                        ),
                    ]),
                )
            })
            .collect();
        self.table.set_rows(rows);
        self.table.state.selected = 0;
    }

    #[cfg(test)]
    pub(crate) fn group(&self) -> LeaderboardGroup {
        self.group
    }

    #[cfg(test)]
    pub(crate) fn subject(&self) -> LeaderboardSubject {
        self.subject
    }

    #[cfg(test)]
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    #[cfg(test)]
    pub(crate) fn area(&self) -> Rect {
        self.area
    }

    pub(crate) fn cycle_subject(&mut self, backwards: bool) {
        self.subject = if backwards {
            self.subject.previous()
        } else {
            self.subject.next()
        };
    }

    pub(crate) fn cycle_group(&mut self, backwards: bool) {
        self.group = if backwards {
            self.group.previous()
        } else {
            self.group.next()
        };
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let last = isize::try_from(self.board.rows.len().saturating_sub(1)).unwrap_or(isize::MAX);
        let current = isize::try_from(self.selected).unwrap_or(isize::MAX);
        self.selected = usize::try_from((current + delta).clamp(0, last)).unwrap_or(0);
        self.table.state.selected = self.selected;
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> LeaderboardClick {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return LeaderboardClick::None;
        }
        let row = mouse.row.saturating_sub(self.area.y);
        let column = mouse.column.saturating_sub(self.area.x);
        match row {
            1 => metric_at_column(column)
                .map_or(LeaderboardClick::None, |group| self.select_group(group)),
            2 if (1..=6).contains(&column) => self.select_subject(LeaderboardSubject::Model),
            2 if (10..=18).contains(&column) => self.select_subject(LeaderboardSubject::Provider),
            2 if (22..=29).contains(&column) => self.select_subject(LeaderboardSubject::Session),
            _ if row >= 6 => {
                let index = usize::from(row - 6);
                if index < self.board.rows.len() {
                    self.selected = index;
                    self.table.state.selected = index;
                    LeaderboardClick::Row
                } else {
                    LeaderboardClick::None
                }
            }
            _ => LeaderboardClick::None,
        }
    }

    pub(crate) fn select_group(&mut self, group: LeaderboardGroup) -> LeaderboardClick {
        let changed = self.group != group;
        self.group = group;
        if changed {
            LeaderboardClick::Tab
        } else {
            LeaderboardClick::None
        }
    }

    pub(crate) fn select_metric(&mut self, group: LeaderboardGroup) {
        self.group = group;
    }

    fn select_subject(&mut self, subject: LeaderboardSubject) -> LeaderboardClick {
        let changed = self.subject != subject;
        self.subject = subject;
        if changed {
            LeaderboardClick::Tab
        } else {
            LeaderboardClick::None
        }
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        self.area = area;
        let active = Style::default()
            .fg(theme.foreground)
            .bg(theme.secondary)
            .add_modifier(Modifier::BOLD);
        let idle = Style::default().fg(theme.muted);
        let title = format!(
            "Rankings · {} by {} · {} entries",
            self.group.label(),
            self.subject.label(),
            self.board.rows.len()
        );
        buf.set_stringn(
            area.x + 1,
            area.y,
            &title,
            usize::from(area.width.saturating_sub(2)),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        let selected = self.board.rows.get(self.selected).map_or_else(
            || "No activity to rank yet".to_owned(),
            |row| {
                format!(
                    "Selected: {} · {} requests · ${:.4}",
                    row.name, row.requests, row.cost_usd
                )
            },
        );
        buf.set_stringn(
            area.x + 1,
            area.y + 3,
            terminalui::clip(&selected, usize::from(area.width.saturating_sub(2))),
            usize::from(area.width.saturating_sub(2)),
            idle,
        );
        for (column, label, group) in metric_tabs() {
            let style = if group == self.group { active } else { idle };
            buf.set_stringn(area.x + column, area.y + 1, label, label.len(), style);
        }

        let (models, providers, sessions) = match self.subject {
            LeaderboardSubject::Model => (active, idle, idle),
            LeaderboardSubject::Provider => (idle, active, idle),
            LeaderboardSubject::Session => (idle, idle, active),
        };
        buf.set_stringn(area.x + 1, area.y + 2, "Models", 6, models);
        buf.set_stringn(area.x + 10, area.y + 2, "Providers", 9, providers);
        buf.set_stringn(area.x + 22, area.y + 2, "Sessions", 8, sessions);

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

fn metric_tabs() -> [(u16, &'static str, LeaderboardGroup); 8] {
    [
        (1, "1 req", LeaderboardGroup::Usage),
        (7, "2 in", LeaderboardGroup::Input),
        (12, "3 out", LeaderboardGroup::Output),
        (18, "4 cost", LeaderboardGroup::Cost),
        (25, "5 latency", LeaderboardGroup::Latency),
        (35, "6 first", LeaderboardGroup::FirstOutput),
        (43, "7 text", LeaderboardGroup::FirstText),
        (50, "8 cache", LeaderboardGroup::CacheRate),
    ]
}

fn metric_at_column(column: u16) -> Option<LeaderboardGroup> {
    metric_tabs().into_iter().find_map(|(start, label, group)| {
        (column >= start && column < start + u16::try_from(label.len()).unwrap_or(u16::MAX))
            .then_some(group)
    })
}

fn metric_value_label(group: LeaderboardGroup, row: &crate::leaderboard::LeaderboardRow) -> String {
    if row.metric_samples == 0 {
        return "n/a".to_owned();
    }
    match group {
        LeaderboardGroup::Cost => format!("${:.4}", row.metric_value),
        LeaderboardGroup::Latency | LeaderboardGroup::FirstOutput | LeaderboardGroup::FirstText => {
            format!("{:.0} ms", row.metric_value)
        }
        LeaderboardGroup::CacheRate => format!("{:.1}%", row.metric_value * 100.0),
        _ => format!("{:.0}", row.metric_value),
    }
}
