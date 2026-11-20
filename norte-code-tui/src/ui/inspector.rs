//! The inspector: a scrollable, selectable list painted as a plain view.
//!
//! Every catalogue view in this client — sessions, models, plans, jobs,
//! plugins, costs, host, telemetry — renders the same shape: a muted caption
//! line, a list of rows, one row highlighted. One view implements the
//! pattern; each page feeds it rows.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// The selection semantics of the inspector.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Selection {
    /// The rows are read-only; nothing activates on Enter.
    Browse,
    /// The rows activate on Enter; the host maps the index to an action.
    Activate,
}

/// A row of the inspector.
#[derive(Clone, Serialize)]
pub(crate) struct Row {
    /// The primary text; bold when selected.
    pub(crate) primary: String,
    /// Optional secondary text, right-aligned.
    pub(crate) secondary: Option<String>,
    /// Optional tag, left of the secondary text.
    pub(crate) tag: Option<String>,
}

impl Row {
    pub(crate) fn new(primary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            secondary: None,
            tag: None,
        }
    }

    pub(crate) fn with_secondary(mut self, secondary: impl Into<String>) -> Self {
        self.secondary = Some(secondary.into());
        self
    }
}

/// The caption + list pattern every catalogue page shares.
#[derive(Serialize)]
pub(crate) struct Inspector {
    /// The caption line (muted, top row).
    pub(crate) caption: String,
    rows: Vec<Row>,
    /// The selected index, if any.
    pub(crate) selected: Option<usize>,
    offset: usize,
    selection: Selection,
    area: Rect,
}

impl Inspector {
    /// Builds an inspector with the given caption.
    pub(crate) fn new(caption: impl Into<String>, selection: Selection) -> Self {
        Self {
            caption: caption.into(),
            rows: Vec::new(),
            selected: None,
            offset: 0,
            selection,
            area: Rect::new(0, 0, 0, 0),
        }
    }

    /// Replace the rows; selection starts at the first row when the page had none.
    pub(crate) fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        match (&self.selected, self.rows.is_empty()) {
            (_, true) => self.selected = None,
            (Some(index), false) if *index >= self.rows.len() => {
                if let Some(index) = self.selected.as_mut() {
                    *index = self.rows.len().saturating_sub(1);
                }
            }
            (None, false) => self.selected = Some(0),
            _ => {}
        }
    }

    /// Move the selection by one; returns whether it moved.
    pub(crate) fn move_up(&mut self) -> bool {
        self.move_by(-1)
    }

    /// Move the selection by one; returns whether it moved.
    pub(crate) fn move_down(&mut self) -> bool {
        self.move_by(1)
    }

    fn move_by(&mut self, delta: isize) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        let current = match self.selected {
            Some(index) => index,
            None if delta >= 0 => {
                self.selected = Some(0);
                self.clamp_view();
                return true;
            }
            None => {
                self.selected = self.rows.len().checked_sub(1);
                self.clamp_view();
                return true;
            }
        };
        let next = current
            .saturating_add_signed(delta)
            .clamp(0, self.rows.len().saturating_sub(1));
        if next == current {
            return false;
        }
        self.selected = Some(next);
        self.clamp_view();
        true
    }

    fn clamp_view(&mut self) {
        if self.rows.len() <= self.body_rows() {
            self.offset = 0;
            return;
        }
        let selected = self.selected.unwrap_or(0);
        if selected < self.offset {
            self.offset = selected;
        } else if selected >= self.offset + self.body_rows() {
            self.offset = selected + 1 - self.body_rows();
        }
    }

    fn body_rows(&self) -> usize {
        self.area.height.saturating_sub(1) as usize
    }
}

impl View for Inspector {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "inspector"
    }

    fn children(&self) -> &[terminalui::Node] {
        &[]
    }

    fn children_mut(&mut self) -> &mut [terminalui::Node] {
        &mut []
    }

    fn measure(&self) -> Option<(u16, u16)> {
        None
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        self.area = area;
        let caption_style = Style::default().fg(theme.muted);
        buf.set_stringn(
            area.x + 1,
            area.y,
            &self.caption,
            self.caption.chars().count(),
            caption_style,
        );

        let body = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        if body.height == 0 {
            return;
        }
        self.clamp_view();
        let visible = body.height as usize;
        for offset in 0..visible {
            let index = self.offset + offset;
            let Some(row) = self.rows.get(index) else {
                break;
            };
            let y = body.y + u16::try_from(offset).unwrap_or(u16::MAX);
            let selected = self.selected == Some(index);
            let row_style = if selected {
                Style::default()
                    .fg(theme.foreground)
                    .bg(theme.secondary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            if selected {
                buf.set_style(
                    Rect {
                        x: body.x,
                        y,
                        width: body.width,
                        height: 1,
                    },
                    Style::default().bg(theme.secondary),
                );
            }
            let marker = if selected { "▌" } else { " " };
            buf.set_stringn(body.x, y, marker, 1, Style::default().fg(theme.accent));
            let primary = format!(" {}", row.primary);
            let primary_max = body.width.saturating_sub(4) as usize;
            let primary_text: String = primary.chars().take(primary_max).collect();
            buf.set_stringn(
                body.x + 2,
                y,
                &primary_text,
                primary_text.chars().count(),
                row_style,
            );
            if let Some(tag) = &row.tag {
                let tag_text = format!(" {tag}");
                let tag_x = body.x
                    + body.width.saturating_sub(
                        u16::try_from(tag_text.chars().count()).unwrap_or(u16::MAX) + 2,
                    );
                let tag_style = Style::default().fg(theme.muted).bg(if selected {
                    theme.secondary
                } else {
                    theme.background
                });
                buf.set_stringn(tag_x, y, &tag_text, tag_text.chars().count(), tag_style);
            }
        }
        // Scrollbar is intentionally omitted: the inspector is short by design.
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        match event {
            InputEvent::Key(key) => {
                use crossterm::event::{KeyCode, KeyEventKind};
                if key.kind != KeyEventKind::Press {
                    return Outcome::Ignored;
                }
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.move_up();
                        Outcome::Consumed
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.move_down();
                        Outcome::Consumed
                    }
                    KeyCode::Home => {
                        self.selected = (!self.rows.is_empty()).then_some(0);
                        self.clamp_view();
                        Outcome::Consumed
                    }
                    KeyCode::End => {
                        self.selected = self.rows.len().checked_sub(1);
                        self.clamp_view();
                        Outcome::Consumed
                    }
                    KeyCode::Enter if self.selection == Selection::Activate => Outcome::Consumed,
                    _ => Outcome::Ignored,
                }
            }
            InputEvent::Mouse(mouse) => {
                use crossterm::event::{MouseButton, MouseEventKind};
                if mouse.kind == MouseEventKind::ScrollUp {
                    self.move_by(-3);
                    return Outcome::Consumed;
                }
                if mouse.kind == MouseEventKind::ScrollDown {
                    self.move_by(3);
                    return Outcome::Consumed;
                }
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    let y = mouse.row;
                    if y > self.area.y && y < self.area.y + self.area.height {
                        let index = self.offset + (y - self.area.y - 1) as usize;
                        if index < self.rows.len() {
                            self.selected = Some(index);
                            self.clamp_view();
                            return Outcome::Consumed;
                        }
                    }
                }
                Outcome::Ignored
            }
            _ => Outcome::Ignored,
        }
    }

    fn focusable(&self) -> bool {
        true
    }
}
