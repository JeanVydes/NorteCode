//! The sessions page: every known session, attachable, renamable, removable.
//!
//! Enter on a session attaches to it; `r` renames it in place; `d` deletes
//! it after a confirmation. Both actions go out as pending requests the
//! host drains; the host re-lists the sessions afterwards. The attached
//! session is marked and cannot be deleted from here.

use std::any::Any;
use std::fmt::Write;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::components::button::{Button, ButtonVariant};
use terminalui::components::dialog::Dialog;
use terminalui::components::form::FormField;
use terminalui::components::input::Input;
use terminalui::components::pagination::Pagination;
use terminalui::{Context, InputEvent, Node, Outcome, Stack, Theme, View, node};

use super::copy::{
    CANCEL, CONFIRM, NO_SESSIONS, SESSIONS_CAPTION, SESSIONS_HINT, TITLE_REQUIRED, delete_question,
};
use super::detail_panel::{DetailPanel, Field};
use super::node_lookup::find_node_mut;

/// Rows shown per page; keeps the pager meaningful on tall terminals.
const PAGE_ROWS: usize = 10;
const PAGER_INDEX: usize = 0;
const DETAIL_INDEX: usize = 1;
const NEW_INDEX: usize = 2;
const RENAME_DIALOG_INDEX: usize = 3;
const DELETE_DIALOG_INDEX: usize = 4;

/// The interaction mode of the page.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
enum Mode {
    /// Browsing the list.
    #[default]
    Listing,
    /// Renaming a session in the composed dialog.
    Rename {
        /// The session to rename.
        session: norte_code_wire::SessionKey,
    },
    /// Confirming the removal in the composed dialog.
    ConfirmDelete {
        /// The session to delete.
        session: norte_code_wire::SessionKey,
    },
}

/// What the page is doing, as far as the host is concerned.
#[derive(Debug, Serialize)]
pub(crate) enum SessionsAction {
    /// Create a fresh session.
    Create,
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
    /// Fleet telemetry rolled up by session, for the rows and detail panel.
    #[serde(skip)]
    usage: crate::session_stats::SharedUsage,
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
    /// Whether Enter was pressed in the rename field.
    #[serde(skip)]
    submit_requested: bool,
    /// The list pager and the selected session's detail panel.
    children: Vec<Node>,
}

impl SessionsPage {
    /// Creates an empty sessions page.
    pub(crate) fn new() -> Self {
        Self {
            sessions: Vec::new(),
            active: None,
            usage: crate::session_stats::SharedUsage::default(),
            selected: 0,
            offset: 0,
            mode: Mode::Listing,
            area: Rect::new(0, 0, 0, 0),
            pending: Vec::new(),
            submit_requested: false,
            children: vec![
                node("sessions_pager", Pagination::new(1)),
                node("session_detail", DetailPanel::new()),
                node("new_session", Button::new(ButtonVariant::Outline, "+ new")),
                node("rename_dialog", Dialog::new(Vec::new())),
                node("delete_dialog", Dialog::new(Vec::new())),
            ],
        }
    }

    fn dialog_actions(cancel_id: &str, confirm_id: &str, destructive: bool) -> Node {
        let confirm = if destructive {
            ButtonVariant::Destructive
        } else {
            ButtonVariant::Primary
        };
        node(
            "actions",
            Stack::horizontal().gap(2).children(vec![
                node(cancel_id, Button::new(ButtonVariant::Secondary, CANCEL)),
                node(confirm_id, Button::new(confirm, CONFIRM)),
            ]),
        )
    }

    fn rename_dialog(title: &str) -> Dialog {
        let mut field = FormField::text("title", "title").required_with_message(TITLE_REQUIRED);
        for child in field.children_mut() {
            if let Some(input) = child.view.as_any().downcast_mut::<Input>() {
                input.set_value(title);
                input.state.anchor = Some(0);
            }
        }
        Dialog::new(vec![
            node("title_field", field),
            Self::dialog_actions("rename_cancel", "rename_confirm", false),
        ])
        .with_title(super::copy::RENAME_TITLE)
    }

    fn delete_dialog(title: &str) -> Dialog {
        Dialog::new(vec![Self::dialog_actions(
            "delete_cancel",
            "delete_confirm",
            true,
        )])
        .with_title(super::copy::DELETE_TITLE)
        .with_message(delete_question(title))
    }

    /// Replace the session list; selection is preserved when possible.
    pub(crate) fn set_data(
        &mut self,
        sessions: Vec<norte_code_wire::SessionSummary>,
        active: Option<String>,
        usage: crate::session_stats::SharedUsage,
    ) {
        let unchanged =
            self.sessions == sessions && self.active == active && Arc::ptr_eq(&self.usage, &usage);
        if unchanged {
            return;
        }
        self.sessions = sessions;
        self.active = active;
        self.usage = usage;
        self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
        self.clamp_view();
    }

    /// Take any pending requests the host should action.
    pub(crate) fn take_pending(&mut self) -> Vec<SessionsAction> {
        self.drain_modal();
        std::mem::take(&mut self.pending)
    }

    fn selected_session(&self) -> Option<&norte_code_wire::SessionSummary> {
        self.sessions.get(self.selected)
    }

    fn body_rows(&self) -> usize {
        usize::from(self.area.height.saturating_sub(3)).min(PAGE_ROWS)
    }

    fn clamp_view(&mut self) {
        let body = self.body_rows();
        // Before the first render the area is zero; do not let that seed a
        // bogus offset that later flips to a random page.
        if body == 0 {
            return;
        }
        if self.sessions.len() <= body {
            self.offset = 0;
        } else if self.selected < self.offset || self.selected >= self.offset + body {
            // Jump whole pages so the pager number and the visible rows agree.
            self.offset = (self.selected / body) * body;
        }
        let body = body.max(1);
        let page = self.offset / body;
        if let Some(pager) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<Pagination>())
        {
            pager.go_to(page);
        }
    }

    /// Keep the pager's page count and the list's offset in agreement.
    fn sync_pager(&mut self) {
        let body = self.body_rows().max(1);
        let total = self.sessions.len().div_ceil(body).max(1);
        let Some(pager) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<Pagination>())
        else {
            return;
        };
        if pager.total_pages != total {
            pager.set_total_pages(total);
        }
        let expected = (self.offset / body).min(total - 1);
        if pager.page() != expected {
            // The pager itself was clicked or keyed; follow its page.
            self.offset = (pager.page() * body).min(self.sessions.len().saturating_sub(1));
            self.selected = self.offset;
            self.clamp_view();
        }
    }

    /// Fill the detail panel from the selected session.
    fn sync_detail(&mut self) {
        let fields = self.selected_session().map_or_else(
            || vec![Field::new("no session", "select a row")],
            |session| {
                let usage = self
                    .usage
                    .get(&session.session_id)
                    .cloned()
                    .unwrap_or_default();
                super::session_detail::fields(session, &usage)
            },
        );
        let title = if self.sessions.is_empty() {
            "session".to_owned()
        } else {
            format!("session · {}", self.selected + 1)
        };
        if let Some(panel) = self
            .children
            .get_mut(1)
            .and_then(|node| node.view.as_any().downcast_mut::<DetailPanel>())
        {
            panel.set(title, fields);
        }
    }

    fn attach(&mut self) {
        if let Some(session) = self.selected_session() {
            self.pending.push(SessionsAction::Attach {
                session: Box::new(session.clone()),
            });
        }
    }

    fn open_rename(&mut self, session: &norte_code_wire::SessionSummary) {
        let title = session
            .title
            .clone()
            .unwrap_or_else(|| session.session_id.clone());
        if let Some(dialog) = self
            .children
            .get_mut(RENAME_DIALOG_INDEX)
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
        {
            *dialog = Self::rename_dialog(&title);
            dialog.open();
        }
        self.mode = Mode::Rename {
            session: norte_code_wire::SessionKey::new(session.session_id.clone()),
        };
        self.submit_requested = false;
    }

    fn open_delete(&mut self, session: &norte_code_wire::SessionSummary) {
        let title = session
            .title
            .clone()
            .unwrap_or_else(|| session.session_id.clone());
        if let Some(dialog) = self
            .children
            .get_mut(DELETE_DIALOG_INDEX)
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
        {
            *dialog = Self::delete_dialog(&title);
            dialog.open();
        }
        self.mode = Mode::ConfirmDelete {
            session: norte_code_wire::SessionKey::new(session.session_id.clone()),
        };
    }

    /// The view the host should focus while a dialog is open.
    pub(crate) fn focus_id(&self) -> Option<&'static str> {
        match self.mode {
            Mode::Listing => None,
            Mode::Rename { .. } => Some("title:input"),
            Mode::ConfirmDelete { .. } => Some("delete_confirm"),
        }
    }

    fn dialog_child<F, R>(
        &mut self,
        index: usize,
        child_id: &str,
        action: impl FnOnce(&mut F) -> R,
    ) -> Option<R>
    where
        F: 'static,
    {
        self.children
            .get_mut(index)
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
            .and_then(|dialog| find_node_mut(dialog.children_mut(), child_id))
            .and_then(|node| node.view.as_any().downcast_mut::<F>())
            .map(action)
    }

    fn dialog_open(&self, index: usize) -> bool {
        self.children
            .get(index)
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Dialog>())
            .is_some_and(Dialog::is_open)
    }

    /// Close finished dialogs and turn their component outcomes into actions.
    fn drain_modal(&mut self) {
        let mode = self.mode.clone();
        let (submitted, renamed, deleted) = match &mode {
            Mode::Listing => return,
            Mode::Rename { session } => {
                let submitted = self.submit_requested
                    || self
                        .dialog_child::<Button, _>(
                            RENAME_DIALOG_INDEX,
                            "rename_confirm",
                            Button::take_pressed,
                        )
                        .unwrap_or(false);
                let cancelled = self
                    .dialog_child::<Button, _>(RENAME_DIALOG_INDEX, "rename_cancel", |button| {
                        button.take_pressed()
                    })
                    .unwrap_or(false);
                if submitted || cancelled || !self.dialog_open(RENAME_DIALOG_INDEX) {
                    let title = self
                        .dialog_child::<FormField, _>(RENAME_DIALOG_INDEX, "title_field", |field| {
                            field.value().trim().to_owned()
                        })
                        .unwrap_or_default();
                    (submitted, Some((session.clone(), title)), None)
                } else {
                    return;
                }
            }
            Mode::ConfirmDelete { session } => {
                let confirmed = self
                    .dialog_child::<Button, _>(DELETE_DIALOG_INDEX, "delete_confirm", |button| {
                        button.take_pressed()
                    })
                    .unwrap_or(false);
                let cancelled = self
                    .dialog_child::<Button, _>(DELETE_DIALOG_INDEX, "delete_cancel", |button| {
                        button.take_pressed()
                    })
                    .unwrap_or(false);
                let was_open = self.dialog_open(DELETE_DIALOG_INDEX);
                if confirmed || cancelled || !was_open {
                    let delete = confirmed || !was_open;
                    (false, None, delete.then(|| session.clone()))
                } else {
                    return;
                }
            }
        };
        let index = match self.mode {
            Mode::Rename { .. } => RENAME_DIALOG_INDEX,
            Mode::ConfirmDelete { .. } => DELETE_DIALOG_INDEX,
            Mode::Listing => return,
        };
        if let Some(dialog) = self
            .children
            .get_mut(index)
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
        {
            dialog.close();
        }
        self.mode = Mode::Listing;
        self.submit_requested = false;
        if submitted
            && let Some((session, title)) = renamed
            && !title.is_empty()
        {
            self.pending.push(SessionsAction::Rename { session, title });
        }
        if let Some(session) = deleted {
            self.pending.push(SessionsAction::Delete { session });
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
            KeyCode::Char('n') => self.pending.push(SessionsAction::Create),
            KeyCode::Enter => self.attach(),
            KeyCode::Char('r') => {
                if let Some(session) = self.selected_session().cloned() {
                    self.open_rename(&session);
                }
            }
            KeyCode::Char('d') => {
                if let Some(session) = self.selected_session().cloned() {
                    if self.active.as_deref() == Some(session.session_id.as_str()) {
                        return;
                    }
                    self.open_delete(&session);
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
                // Leave the hint and pager rows to the components that own them.
                if y > self.area.y
                    && y <= self.area.y + u16::try_from(self.body_rows()).unwrap_or(0)
                {
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

    fn children(&self) -> &[Node] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn measure(&self) -> Option<(u16, u16)> {
        None
    }

    fn focusable(&self) -> bool {
        true
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let _ = cx.is_focused();
        self.area = area;
        buf.set_style(area, Style::default().bg(theme.background));
        let detail_width = (area.width / 2)
            .clamp(26, 48)
            .min(area.width.saturating_sub(26));
        let list_width = area.width.saturating_sub(detail_width + 1);
        let list_area = Rect {
            width: list_width,
            ..area
        };
        let detail_area = Rect {
            x: area.x + list_width + 1,
            width: detail_width,
            ..area
        };

        Self::paint_caption(list_area, buf, theme);
        let button_rect = Rect {
            x: list_area.x + list_area.width.saturating_sub(8),
            y: list_area.y,
            width: 7,
            height: 1,
        };
        if let Some(child) = self.children.get_mut(NEW_INDEX) {
            child.view.render(button_rect, buf, cx, theme);
            if let Some(button) = child.view.as_any().downcast_mut::<Button>() {
                button.state.area = button_rect;
            }
        }
        if self.mode == Mode::Listing {
            self.paint_list(list_area, buf, theme);
            Self::paint_hint(list_area, buf, theme);
        }

        self.sync_pager();
        self.sync_detail();
        let pager_rect = Rect {
            y: area.y + area.height.saturating_sub(1),
            width: list_width,
            height: 1,
            ..list_area
        };
        if let Some(child) = self.children.get_mut(PAGER_INDEX) {
            child.view.render(pager_rect, buf, cx, theme);
        }
        if let Some(child) = self.children.get_mut(DETAIL_INDEX) {
            child.view.render(detail_area, buf, cx, theme);
        }
        let dialog_index = match self.mode {
            Mode::Listing => None,
            Mode::Rename { .. } => Some(RENAME_DIALOG_INDEX),
            Mode::ConfirmDelete { .. } => Some(DELETE_DIALOG_INDEX),
        };
        if let Some(index) = dialog_index
            && let Some(dialog) = self.children.get_mut(index)
        {
            dialog.view.render(area, buf, cx, theme);
        }
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        use crossterm::event::KeyEventKind;
        let _ = cx.is_focused();
        match event {
            InputEvent::Mouse(mouse) => self.handle_mouse(*mouse),
            InputEvent::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Outcome::Ignored;
                }
                if matches!(self.mode, Mode::Rename { .. }) {
                    if key.code == crossterm::event::KeyCode::Enter {
                        self.submit_requested = true;
                        return Outcome::Consumed;
                    }
                    return Outcome::Ignored;
                }
                if self.mode != Mode::Listing {
                    return Outcome::Ignored;
                }
                self.handle_listing_key(key);
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }
}

impl SessionsPage {
    fn paint_caption(area: Rect, buf: &mut Buffer, theme: &Theme) {
        let caption = SESSIONS_CAPTION;
        buf.set_stringn(
            area.x + 1,
            area.y,
            caption,
            caption.chars().count(),
            Style::default().fg(theme.muted),
        );
    }

    fn paint_hint(area: Rect, buf: &mut Buffer, theme: &Theme) {
        let y = area.y + area.height.saturating_sub(2);
        let hint = SESSIONS_HINT;
        buf.set_stringn(
            area.x + 1,
            y,
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
            let clipped = terminalui::clip(title, 44);
            buf.set_stringn(area.x + 2, y, &clipped, 44, title_style);
            let provider = session.provider.as_deref().unwrap_or("unknown");
            let usage = self.usage.get(&session.session_id);
            let tokens = usage.map_or_else(
                || "–:–".to_owned(),
                |usage| {
                    format!(
                        "{}:{}",
                        crate::context::human_count(usage.input),
                        crate::context::human_count(usage.output)
                    )
                },
            );
            let cache = usage.map_or_else(
                || "–".to_owned(),
                |usage| format!("{:.0}%", usage.cache_rate()),
            );
            let turns = session
                .turn_count
                .or_else(|| usage.map(|usage| usage.turns.len() as u64));
            let mut secondary = format!("{provider} · {tokens} · {cache} cache");
            if let Some(turns) = turns {
                let _ = write!(&mut secondary, " · {turns}t");
            }
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
            let text = NO_SESSIONS;
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

#[cfg(test)]
#[path = "sessions_page_tests.rs"]
mod tests;
