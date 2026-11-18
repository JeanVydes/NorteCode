//! The left rail: what the session is, and whether it is attached.
//!
//! Sections, top to bottom: SESSION (identity) and CONNECTION
//! (the attachment state, in plain words). No placeholder content is
//! shown; a section appears only when the client has the data for it.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::components::chrome::{draw_box, Border, BorderSet, Chrome, Padding, Radius};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

use crate::ui_model::SessionAttachmentState;

const RAIL_WIDTH: u16 = 26;

/// The fixed rail.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Rail {
    /// Where the connection stands right now.
    pub(crate) attachment: SessionAttachmentState,
    /// The session id, when one exists.
    pub(crate) session: Option<String>,
    /// The provider id, when the host reported one.
    pub(crate) provider: Option<String>,
    /// The model id, when the host reported one.
    pub(crate) model: Option<String>,
}

impl Rail {
    /// Creates a rail for a never-attached session.
    pub(crate) fn detached() -> Self {
        Self {
            attachment: SessionAttachmentState::Disconnected,
            session: None,
            provider: None,
            model: None,
        }
    }

    /// Replace the session shown; `None` clears the id line.
    pub(crate) fn set_session(&mut self, session: Option<String>) {
        self.session = session;
    }
}

impl View for Rail {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "rail"
    }

    fn measure(&self) -> Option<(u16, u16)> {
        Some((RAIL_WIDTH, 1))
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        let chrome = Chrome {
            border: Some(Border {
                set: BorderSet::Rounded,
                color: Some(theme.border),
                radius: Radius::Small,
            }),
            background: None,
            radius: Radius::Small,
            padding: Padding {
                horizontal: 0,
                vertical: 0,
            },
            border_gradient: None,
            fill_gradient: None,
        };
        let inner = draw_box(buf, area, &chrome, theme);
        if inner.width < 10 {
            return;
        }
        let dim = Style::default().fg(theme.muted).bg(theme.background);
        let text = Style::default().fg(theme.foreground).bg(theme.background);
        let mut y = inner.y + 1;

        buf.set_stringn(inner.x + 1, y, "SESSION", 7, dim.add_modifier(Modifier::BOLD));
        y += 1;
        if let Some(session) = &self.session {
            buf.set_stringn(inner.x + 1, y, session, session.len(), text.clone().add_modifier(Modifier::BOLD));
            y += 1;
        } else {
            buf.set_stringn(inner.x + 1, y, "no session", 10, dim);
            y += 2;
        }
        if let Some(model) = &self.model {
            buf.set_stringn(inner.x + 1, y, model, model.len(), Style::default().fg(theme.accent).bg(theme.background));
        } else {
            buf.set_stringn(inner.x + 1, y, "—", 1, dim);
        }
        y += 1;
        if let Some(provider) = &self.provider {
            buf.set_stringn(inner.x + 1, y, provider, provider.len(), dim);
        } else {
            buf.set_stringn(inner.x + 1, y, "—", 1, dim);
        }
        y += 2;

        buf.set_stringn(inner.x + 1, y, "CONNECTION", 10, dim.add_modifier(Modifier::BOLD));
        y += 1;
        match &self.attachment {
            SessionAttachmentState::Disconnected => {
                buf.set_stringn(inner.x + 1, y, "detached", 8, dim);
            }
            SessionAttachmentState::Connecting => {
                buf.set_stringn(inner.x + 1, y, "● connecting…", 14, Style::default().fg(theme.warning).bg(theme.background));
            }
            SessionAttachmentState::Attached => {
                buf.set_stringn(inner.x + 1, y, "● attached", 10, Style::default().fg(theme.success).bg(theme.background));
            }
            SessionAttachmentState::Failed { reason } => {
                buf.set_stringn(inner.x + 1, y, "✗ failed", 9, Style::default().fg(theme.destructive).bg(theme.background).add_modifier(Modifier::BOLD));
                y += 1;
                let detail: String = reason.chars().take(inner.width.saturating_sub(3) as usize).collect();
                buf.set_stringn(inner.x + 1, y, &detail, detail.len(), dim);
            }
        }
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}
