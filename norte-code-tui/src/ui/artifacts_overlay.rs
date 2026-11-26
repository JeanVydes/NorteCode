use std::any::Any;

use crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use norte_code_wire::{ArtifactOrigin, SessionArtifact, SessionKey};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
    widgets::{Clear, Widget},
};
use serde::Serialize;
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius, draw_box};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

#[derive(Default, Serialize)]
pub(crate) struct ArtifactsOverlay {
    open: bool,
    session: Option<SessionKey>,
    #[serde(skip)]
    state: Inventory,
    selected: usize,
    offset: usize,
    #[serde(skip)]
    list: Rect,
}

#[derive(Default)]
enum Inventory {
    #[default]
    NoSession,
    Loading,
    Loaded(Vec<SessionArtifact>),
    Failed(String),
}

impl ArtifactsOverlay {
    pub(crate) fn open(&mut self, session: Option<SessionKey>) {
        let state = if session.is_some() {
            Inventory::Loading
        } else {
            Inventory::NoSession
        };
        *self = Self {
            open: true,
            session,
            state,
            ..Self::default()
        };
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn sync_session(&mut self, session: Option<&SessionKey>) {
        if self.open && self.session.as_ref() != session {
            *self = Self::default();
        }
    }

    pub(crate) fn finish(
        &mut self,
        session: &SessionKey,
        result: Result<Vec<SessionArtifact>, String>,
    ) {
        if self.open && self.session.as_ref() == Some(session) {
            self.state = match result {
                Ok(items) => Inventory::Loaded(items),
                Err(error) => Inventory::Failed(error),
            };
            self.selected = 0;
            self.offset = 0;
        }
    }

    fn count(&self) -> usize {
        match &self.state {
            Inventory::Loaded(items) => items.len(),
            _ => 0,
        }
    }

    fn advance(&mut self, rows: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(rows)
            .min(self.count().saturating_sub(1));
    }

    fn paint_body(&mut self, body: Rect, buf: &mut Buffer, theme: &Theme) {
        let Inventory::Loaded(items) = &self.state else {
            let message = match &self.state {
                Inventory::NoSession => "Open a session or send a message to see its artifacts.",
                Inventory::Loading => "Loading artifacts...",
                Inventory::Failed(error) => error,
                Inventory::Loaded(_) => return,
            };
            line(buf, body, 0, message, theme.muted);
            return;
        };
        if items.is_empty() {
            line(buf, body, 0, "This session has no artifacts.", theme.muted);
            return;
        }

        let details_height = 4.min(body.height.saturating_sub(1));
        self.list = Rect {
            height: body.height.saturating_sub(details_height),
            ..body
        };
        let visible = usize::from(self.list.height).max(1);
        self.offset = self
            .offset
            .min(self.selected)
            .max(self.selected.saturating_add(1).saturating_sub(visible));
        for (row, item) in (0..self.list.height).zip(items.iter().skip(self.offset)) {
            let selected = self.selected == self.offset + usize::from(row);
            let marker = if selected { ">" } else { " " };
            let color = if selected {
                theme.accent
            } else {
                theme.foreground
            };
            let source = origin_label(&item.origin);
            line(
                buf,
                self.list,
                row,
                &format!(
                    "{marker} {source:<16} {}",
                    item.name.as_deref().unwrap_or(&item.mime_type)
                ),
                color,
            );
        }
        let Some(item) = items.get(self.selected) else {
            return;
        };
        let detail = Rect::new(body.x, self.list.bottom(), body.width, details_height);
        let timestamp = item
            .created_at
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string());
        line(
            buf,
            detail,
            0,
            &format!(
                "{}  |  {} bytes  |  {}",
                item.mime_type,
                item.bytes,
                timestamp.as_deref().unwrap_or("Date unavailable")
            ),
            theme.foreground,
        );
        line(
            buf,
            detail,
            1,
            &format!("Blob  {}", item.blob_id),
            theme.muted,
        );
        let producer = match &item.origin {
            ArtifactOrigin::Plugin { plugin_id, call_id } => {
                format!("Plugin {plugin_id}  |  Call {call_id}")
            }
            ArtifactOrigin::Tool { call_id } => format!("Tool call {call_id}"),
            ArtifactOrigin::AssistantOutput {
                turn: Some(turn),
                part_index,
            } => part_index.map_or_else(
                || format!("Turn {turn}"),
                |part| format!("Turn {turn}  |  Part {}", part.saturating_add(1)),
            ),
            _ => origin_label(&item.origin),
        };
        line(buf, detail, 2, &producer, theme.muted);
    }
}

fn origin_label(origin: &ArtifactOrigin) -> String {
    match origin {
        ArtifactOrigin::UserUpload => "Upload".to_owned(),
        ArtifactOrigin::AssistantOutput { .. } => "Assistant".to_owned(),
        ArtifactOrigin::Plugin { plugin_id, .. } => plugin_id.to_string(),
        ArtifactOrigin::Tool { .. } => "Tool".to_owned(),
        ArtifactOrigin::Message => "Message".to_owned(),
    }
}

fn line(buf: &mut Buffer, area: Rect, row: u16, text: &str, color: ratatui::style::Color) {
    if row < area.height {
        buf.set_stringn(
            area.x,
            area.y + row,
            text,
            usize::from(area.width),
            Style::default().fg(color),
        );
    }
}

impl View for ArtifactsOverlay {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "artifacts_overlay"
    }
    fn focusable(&self) -> bool {
        self.open
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        self.list = Rect::default();
        if !self.open || area.width < 10 || area.height < 5 {
            return;
        }
        let width = area.width.saturating_sub(2).min(94);
        let height = area.height.saturating_sub(2).min(26);
        let frame = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        Clear.render(frame, buf);
        let chrome = Chrome {
            border: Some(Border {
                set: BorderSet::Rounded,
                color: Some(theme.border),
                radius: Radius::Small,
            }),
            background: Some(theme.secondary),
            radius: Radius::Small,
            padding: Padding {
                horizontal: 1,
                vertical: 0,
            },
            border_gradient: None,
            fill_gradient: None,
        };
        let inner = draw_box(buf, frame, &chrome, theme);
        let title = format!("Session artifacts  |  {}", self.count());
        line(buf, inner, 0, &title, theme.accent);
        if inner.height >= 3 {
            self.paint_body(
                Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 2),
                buf,
                theme,
            );
            line(
                buf,
                inner,
                inner.height - 1,
                "Up/Down select  |  PgUp/PgDn scroll  |  Esc close",
                theme.muted,
            );
        }
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        if !self.open {
            return Outcome::Ignored;
        }
        let page = isize::try_from(self.list.height)
            .unwrap_or(isize::MAX)
            .max(1);
        match event {
            InputEvent::Key(key) => match key.code {
                KeyCode::Esc => *self = Self::default(),
                KeyCode::Up | KeyCode::Char('k') => self.advance(-1),
                KeyCode::Down | KeyCode::Char('j') => self.advance(1),
                KeyCode::PageUp => self.advance(-page),
                KeyCode::PageDown => self.advance(page),
                KeyCode::Home => self.selected = 0,
                KeyCode::End => self.selected = self.count().saturating_sub(1),
                _ => {}
            },
            InputEvent::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.advance(-3),
                MouseEventKind::ScrollDown => self.advance(3),
                MouseEventKind::Down(MouseButton::Left)
                    if self.list.contains(Position::new(mouse.column, mouse.row)) =>
                {
                    self.selected = (self.offset + usize::from(mouse.row - self.list.y))
                        .min(self.count().saturating_sub(1));
                }
                _ => {}
            },
            _ => {}
        }
        Outcome::Consumed
    }
}
