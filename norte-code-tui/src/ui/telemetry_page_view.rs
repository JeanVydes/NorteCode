//! Telemetry view implementation: rendering and event routing.

use std::any::Any;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use terminalui::components::table::Table;
use terminalui::{Context, Direction, Effect, InputEvent, Node, Outcome, Theme, View};
use unicode_width::UnicodeWidthStr;

use crate::ui::leaderboard_section::LeaderboardClick;

use super::{CHART_ROWS, HEATMAP_ROWS, SESSION_PAGE, TelemetryPage, TelemetryView};

impl TelemetryPage {
    pub(super) fn paint_lower(
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
        let lower_h = area.y.saturating_add(area.height).saturating_sub(lower_y);

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

    fn paint_body(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        match self.view {
            TelemetryView::Charts => {
                let heatmap_height = HEATMAP_ROWS.min(area.height);
                let chart_gap = u16::from(heatmap_height > 0 && area.height > heatmap_height);
                self.paint_heatmap(
                    Rect {
                        height: heatmap_height,
                        ..area
                    },
                    buf,
                    theme,
                );
                self.paint_charts(
                    Rect {
                        y: area.y + heatmap_height + chart_gap,
                        height: area.height.saturating_sub(heatmap_height + chart_gap),
                        ..area
                    },
                    buf,
                    cx,
                    theme,
                );
            }
            TelemetryView::Leaderboards => self.paint_leaderboards(area, buf, cx, theme),
            TelemetryView::Sessions | TelemetryView::Requests => {
                self.paint_lower(area, area.y, buf, cx, theme);
            }
        }
    }

    pub(super) fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Option<Outcome> {
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

    fn handle_chart_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Outcome> {
        use crossterm::event::{KeyCode, KeyModifiers};
        if self.view != TelemetryView::Charts {
            return None;
        }
        match key.code {
            KeyCode::Up | KeyCode::PageUp => {
                self.scroll_chart_view(if key.code == KeyCode::Up { -3 } else { -6 });
            }
            KeyCode::Down | KeyCode::PageDown => {
                self.scroll_chart_view(if key.code == KeyCode::Down { 3 } else { 6 });
            }
            KeyCode::Char('[') => self.cycle_interval(true),
            KeyCode::Char(']') => self.cycle_interval(false),
            KeyCode::Char('d') if key.modifiers == KeyModifiers::NONE => {
                self.cycle_chart_density();
            }
            KeyCode::Char('c') if key.modifiers == KeyModifiers::NONE => {
                self.cycle_chart_contrast();
            }
            KeyCode::Char('v') if key.modifiers == KeyModifiers::NONE => {
                self.cycle_chart_dot_size();
            }
            KeyCode::Char('g') if key.modifiers == KeyModifiers::NONE => self.cycle_chart_glyphs(),
            KeyCode::Char('+' | '=')
                if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
            {
                self.zoom_chart_window(1.25);
            }
            KeyCode::Char('-' | '_')
                if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
            {
                self.zoom_chart_window(0.8);
            }
            _ => return None,
        }
        Some(Outcome::Consumed)
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
        if area.height == 0 || area.width == 0 {
            return;
        }
        buf.set_style(area, Style::default().bg(theme.background));
        let caption = self.caption();
        let hint = match self.view {
            TelemetryView::Charts => {
                "Tab view · wheel/↑↓ history · [ ] interval · d density · c contrast · v size · g glyphs"
            }
            TelemetryView::Leaderboards => "Tab view · 1-8 metric · ←→ type · j/k rows",
            TelemetryView::Sessions => "Tab view · j/k sessions · [ ] interval",
            TelemetryView::Requests => "Tab view · j/k requests · [ ] interval",
        };
        let hint_width = u16::try_from(UnicodeWidthStr::width(hint)).unwrap_or(u16::MAX);
        let show_hint = area.width > hint_width.saturating_add(10);
        let caption_width = if show_hint {
            area.width.saturating_sub(hint_width.saturating_add(3))
        } else {
            area.width.saturating_sub(2)
        };
        buf.set_stringn(
            area.x + 1,
            area.y,
            &caption,
            usize::from(caption_width),
            Style::default().fg(theme.muted),
        );
        if show_hint {
            buf.set_stringn(
                area.x + area.width.saturating_sub(hint_width + 1),
                area.y,
                hint,
                usize::from(hint_width),
                Style::default().fg(theme.muted),
            );
        }
        let body = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        let progress = self
            .transition
            .as_ref()
            .map(terminalui::Animation::progress);
        if let Some(progress) = progress {
            terminalui::render_with(
                &Effect::SlideIn {
                    direction: Direction::Right,
                    distance: 3,
                    easing: terminalui::Easing::EaseOutCubic,
                },
                progress,
                buf,
                body,
                theme,
                |area, buf, theme| self.paint_body(area, buf, cx, theme),
            );
        } else {
            self.paint_body(body, buf, cx, theme);
        }
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        use crossterm::event::KeyCode;
        if let InputEvent::Mouse(mouse) = event
            && let Some(outcome) = self.handle_mouse(*mouse)
        {
            return outcome;
        }
        if self.handle_chart_hover(event, cx) == Outcome::Consumed {
            return Outcome::Consumed;
        }
        if let InputEvent::Key(key) = event
            && key.kind == crossterm::event::KeyEventKind::Press
        {
            if self.view == TelemetryView::Leaderboards
                && let Some(outcome) = self.handle_leaderboard_key(key.code)
            {
                return outcome;
            }
            if let Some(outcome) = self.handle_chart_key(*key) {
                return outcome;
            }
            match key.code {
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

    fn tick(&mut self, delta: Duration, _cx: &mut Context) {
        if self
            .transition
            .as_mut()
            .is_some_and(|animation| !animation.tick(delta))
        {
            self.transition = None;
        }
    }
}
