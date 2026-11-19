//! The sessions page: every known session, attachable, renamable, removable.
//!
//! Enter on a session attaches to it; `r` renames it in place; `d` deletes
//! it after a confirmation. Both actions go out as pending requests the
//! host drains; the host re-lists the sessions afterwards. The attached
//! session is marked and cannot be deleted from here.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// The interaction mode of the page.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
enum Mode {
    /// Browsing the list.
    #[default]
    Listing,
    /// Typing the new title; carries the session.
    Rename {
        /// The session to rename.
        session: String,
        /// The title so far.
        title: String,
    },
    /// Confirming the removal; carries the session.
    ConfirmDelete {
        /// The session to delete.
        session: String,
    },
}

/// What the page is doing, as far as the host is concerned.
#[derive(Serialize)]
pub(crate) enum SessionsAction {
    /// Attach to this session.
    Attach {
        /// The session to attach to.
        session: Box<norte_code_wire::SessionSummary>,
    },
    /// Rename this session.
    Rename {
        /// The session to rename.
        session: norte_code_wire::SessionKey,
        /// The new title.
        title: String,
    },
    /// Delete this session.
    Delete {
        /// The session to delete.
        session: norte_code_wire::SessionKey,
    },
}

/// The sessions page.
#[derive(Serialize)]
pub(crate) struct SessionsPage {
    /// Every known session, newest first.
    #[serde(skip)]
    sessions: Vec<norte_code_wire::SessionSummary>,
    /// The attached session, when one is.
    #[serde(skip)]
    active: Option<String>,
    /// The selected row.
    selected: usize,
    /// The first visible row.
    offset: usize,
    /// What the page is doing over the list.
    #[serde(default)]
    mode: Mode,
    /// The last drawn area.
    #[serde(skip)]
    area: Rect,
    /// Requests waiting for the host.
    #[serde(skip)]
    pending: Vec<SessionsAction>,
}

impl SessionsPage {
    /// Creates an empty sessions page.
    pub(crate) fn new() -> Self {
        Self {
            sessions: Vec::new(),
            active: None,
            selected: 0,
            offset: 0,
            mode: Mode::Listing,
            area: Rect::new(0, 0, 0, 0),
            pending: Vec::new(),
        }
    }

    /// Replace the session list; selection is preserved when possible.
    pub(crate) fn set_data(
        &mut self,
        sessions: Vec<norte_code_wire::SessionSummary>,
        active: Option<String>,
    ) {
        self.sessions = sessions;
        self.active = active;
        self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
        self.clamp_view();
    }

    /// Take any pending requests the host should action.
    pub(crate) fn take_pending(&mut self) -> Vec<SessionsAction> {
        std::mem::take(&mut self.pending)
    }

    fn selected_session(&self) -> Option<&norte_code_wire::SessionSummary> {
        self.sessions.get(self.selected)
    }

    fn body_rows(&self) -> usize {
        self.area.height.saturating_sub(2) as usize
    }

    fn clamp_view(&mut self) {
        let body = self.body_rows();
        if self.sessions.len() <= body {
            self.offset = 0;
            return;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + body {
            self.offset = self.selected + 1 - body;
        }
    }

    fn attach(&mut self) {
        if let Some(session) = self.selected_session() {
            self.pending.push(SessionsAction::Attach {
                session: Box::new(session.clone()),
            });
        }
    }

    fn type_title(&mut self, key: &crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        if !matches!(
            key.modifiers,
            crossterm::event::KeyModifiers::NONE | crossterm::event::KeyModifiers::SHIFT
        ) {
            return;
        }
        let Mode::Rename { session, title } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::Listing,
            KeyCode::Backspace => {
                title.pop();
            }
            KeyCode::Char(character) => title.push(character),
            KeyCode::Enter => {
                let title = std::mem::take(title).trim().to_owned();
                if !title.is_empty() {
                    self.pending.push(SessionsAction::Rename {
                        session: norte_code_wire::SessionKey::new(session.clone()),
                        title,
                    });
                }
                self.mode = Mode::Listing;
            }
            _ => {}
        }
    }

    fn confirm_delete(&mut self, key: &crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        if !matches!(
            key.modifiers,
            crossterm::event::KeyModifiers::NONE | crossterm::event::KeyModifiers::SHIFT
        ) {
            return;
        }
        match key.code {
            KeyCode::Esc => self.mode = Mode::Listing,
            KeyCode::Enter => {
                if let Mode::ConfirmDelete { session } = std::mem::take(&mut self.mode) {
                    self.pending.push(SessionsAction::Delete {
                        session: norte_code_wire::SessionKey::new(session),
                    });
                    self.mode = Mode::Listing;
                }
            }
            _ => {}
        }
    }

    fn handle_listing_key(&mut self, key: &crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        if !matches!(
            key.modifiers,
            crossterm::event::KeyModifiers::NONE | crossterm::event::KeyModifiers::SHIFT
        ) {
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.sessions.len().saturating_sub(1));
            }
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(self.body_rows().max(1));
            }
            KeyCode::PageDown => {
                self.selected = (self.selected + self.body_rows().max(1))
                    .min(self.sessions.len().saturating_sub(1));
            }
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.sessions.len().saturating_sub(1),
            KeyCode::Enter => self.attach(),
            KeyCode::Char('r') => {
                if let Some(session) = self.selected_session() {
                    let title = session
                        .title
                        .clone()
                        .unwrap_or_else(|| session.session_id.clone());
                    self.mode = Mode::Rename {
                        session: session.session_id.clone(),
                        title,
                    };
                }
            }
            KeyCode::Char('d') => {
                if let Some(session) = self.selected_session() {
                    if self.active.as_deref() == Some(session.session_id.as_str()) {
                        return;
                    }
                    self.mode = Mode::ConfirmDelete {
                        session: session.session_id.clone(),
                    };
                }
            }
            _ => return,
        }
        self.clamp_view();
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Outcome {
        use crossterm::event::{MouseButton, MouseEventKind};
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.selected = self.selected.saturating_sub(3);
                self.clamp_view();
                Outcome::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.selected = (self.selected + 3).min(self.sessions.len().saturating_sub(1));
                self.clamp_view();
                Outcome::Consumed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.mode != Mode::Listing {
                    return Outcome::Ignored;
                }
                let y = mouse.row;
                if y > self.area.y && y < self.area.y + self.area.height {
                    let index = self.offset + (y - self.area.y - 1) as usize;
                    if index < self.sessions.len() {
                        self.selected = index;
                        self.clamp_view();
                        return Outcome::Consumed;
                    }
                }
                Outcome::Ignored
            }
            _ => Outcome::Ignored,
        }
    }
}

impl View for SessionsPage {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "sessions_page"
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

    fn focusable(&self) -> bool {
        true
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        self.area = area;
        buf.set_style(area, Style::default().bg(theme.background));
        Self::paint_caption(area, buf, theme);
        match &self.mode {
            Mode::Rename { title, .. } => {
                Self::paint_rename(area, buf, theme, title);
                return;
            }
            Mode::ConfirmDelete { session } => {
                let title = self
                    .sessions
                    .iter()
                    .find(|candidate| candidate.session_id == *session)
                    .and_then(|session| session.title.clone())
                    .unwrap_or_else(|| clip(session, 48));
                Self::paint_delete(area, buf, theme, &title);
                return;
            }
            Mode::Listing => {}
        }
        self.paint_list(area, buf, theme);
        Self::paint_hint(area, buf, theme);
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        use crossterm::event::KeyEventKind;
        match event {
            InputEvent::Mouse(mouse) => self.handle_mouse(*mouse),
            InputEvent::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Outcome::Ignored;
                }
                match &self.mode {
                    Mode::Rename { .. } => self.type_title(key),
                    Mode::ConfirmDelete { .. } => self.confirm_delete(key),
                    Mode::Listing => self.handle_listing_key(key),
                }
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }
}

impl SessionsPage {
    fn paint_caption(area: Rect, buf: &mut Buffer, theme: &Theme) {
        let caption = "sessions · newest first";
        buf.set_stringn(
            area.x + 1,
            area.y,
            caption,
            caption.chars().count(),
            Style::default().fg(theme.muted),
        );
    }

    fn paint_hint(area: Rect, buf: &mut Buffer, theme: &Theme) {
        let y = area.y + area.height.saturating_sub(1);
        let hint = "⏎ attach · r rename · d delete";
        buf.set_stringn(
            area.x + 1,
            y,
            hint,
            hint.chars().count(),
            Style::default().fg(theme.muted),
        );
    }

    fn paint_rename(area: Rect, buf: &mut Buffer, theme: &Theme, title: &str) {
        let y = area.y + 2;
        buf.set_stringn(
            area.x + 1,
            y,
            "rename session",
            15,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        let input = format!("{title}▏");
        let budget = usize::from(area.width.saturating_sub(2)).min(input.chars().count() + 2);
        buf.set_stringn(
            area.x + 1,
            y + 2,
            &input,
            budget,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        let hint = "Enter renames, Esc cancels";
        buf.set_stringn(
            area.x + 1,
            y + 4,
            hint,
            hint.chars().count(),
            Style::default().fg(theme.muted),
        );
    }

    fn paint_delete(area: Rect, buf: &mut Buffer, theme: &Theme, title: &str) {
        let y = area.y + 2;
        buf.set_stringn(
            area.x + 1,
            y,
            "delete session",
            14,
            Style::default()
                .fg(theme.destructive)
                .add_modifier(Modifier::BOLD),
        );
        let question = format!("Delete {title}?");
        let budget = usize::from(area.width.saturating_sub(2));
        buf.set_stringn(
            area.x + 1,
            y + 2,
            &question,
            budget,
            Style::default().fg(theme.foreground),
        );
        let hint = "Enter deletes, Esc cancels";
        buf.set_stringn(
            area.x + 1,
            y + 4,
            hint,
            hint.chars().count(),
            Style::default().fg(theme.muted),
        );
    }

    fn paint_list(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let rows = self.body_rows();
        let end = (self.offset + rows).min(self.sessions.len());
        for (offset, index) in (self.offset..end).enumerate() {
            let y = area.y + 1 + u16::try_from(offset).unwrap_or(0);
            let session = &self.sessions[index];
            let selected = self.selected == index;
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
            let is_active = self.active.as_deref() == Some(session.session_id.as_str());
            let marker = if is_active {
                "▌"
            } else if session.pinned {
                "◆"
            } else {
                " "
            };
            let marker_style = if is_active {
                Style::default().fg(theme.accent)
            } else {
                Style::default().fg(theme.muted)
            };
            buf.set_stringn(area.x, y, marker, 2usize, marker_style);
            let title = session
                .title
                .as_deref()
                .unwrap_or(session.session_id.as_str());
            let title_style = if selected {
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            let clipped = clip(title, 44);
            buf.set_stringn(area.x + 2, y, &clipped, 44, title_style);
            let provider = session.provider.as_deref().unwrap_or("unknown");
            let model = session
                .model
                .as_ref()
                .map_or_else(|| "unknown".to_string(), |model| clip(&model.id, 24));
            let mut secondary = format!("{provider} · {model}");
            if is_active {
                secondary.push_str(" · current");
            }
            let secondary_style = Style::default().fg(theme.muted);
            let secondary_x = area.x + 46;
            let secondary_budget = usize::from(area.width.saturating_sub(46 + 1));
            buf.set_stringn(
                secondary_x,
                y,
                &secondary,
                secondary_budget,
                secondary_style,
            );
        }
        if self.sessions.is_empty() {
            let text = "no sessions yet";
            buf.set_stringn(
                area.x + 1,
                area.y + 1,
                text,
                text.chars().count(),
                Style::default().fg(theme.muted),
            );
        }
    }
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
#[path = "sessions_page_tests.rs"]
mod tests;
