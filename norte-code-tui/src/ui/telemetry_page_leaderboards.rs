//! Telemetry leaderboards: rebuild, paint, and key handling.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::{Context, Outcome, Theme};

use super::{MAX_RECORD_WINDOW, TelemetryPage};

impl TelemetryPage {
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

    pub(super) fn rebuild_leaderboard(&mut self) {
        let records: &[norte_code_wire::TelemetryRecord] =
            self.report.as_ref().map_or(&[], |report| {
                let start = report.records.len().saturating_sub(MAX_RECORD_WINDOW);
                &report.records[start..]
            });
        self.leaderboard.rebuild(records);
    }

    pub(super) fn paint_leaderboards(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        theme: &Theme,
    ) {
        self.leaderboard.render(area, buf, cx, theme);
    }

    pub(super) fn handle_leaderboard_key(
        &mut self,
        code: crossterm::event::KeyCode,
    ) -> Option<Outcome> {
        use crossterm::event::KeyCode;
        let consumed = match code {
            KeyCode::Char('1') => {
                self.leaderboard
                    .select_metric(crate::leaderboard::LeaderboardGroup::Usage);
                true
            }
            KeyCode::Char('2') => {
                self.leaderboard
                    .select_metric(crate::leaderboard::LeaderboardGroup::Input);
                true
            }
            KeyCode::Char('3') => {
                self.leaderboard
                    .select_metric(crate::leaderboard::LeaderboardGroup::Output);
                true
            }
            KeyCode::Char('4') => {
                self.leaderboard
                    .select_metric(crate::leaderboard::LeaderboardGroup::Cost);
                true
            }
            KeyCode::Char('5') => {
                self.leaderboard
                    .select_metric(crate::leaderboard::LeaderboardGroup::Latency);
                true
            }
            KeyCode::Char('6') => {
                self.leaderboard
                    .select_metric(crate::leaderboard::LeaderboardGroup::FirstOutput);
                true
            }
            KeyCode::Char('7') => {
                self.leaderboard
                    .select_metric(crate::leaderboard::LeaderboardGroup::FirstText);
                true
            }
            KeyCode::Char('8') => {
                self.leaderboard
                    .select_metric(crate::leaderboard::LeaderboardGroup::CacheRate);
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
