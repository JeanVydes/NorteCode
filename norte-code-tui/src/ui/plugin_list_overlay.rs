//! Installed plugin inventory opened by `/plugin list`.

#[path = "plugin_list_overlay_render.rs"]
mod render;

use std::any::Any;
use std::time::Duration;

use crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use serde::Serialize;
use terminalui::{Context, InputEvent, Outcome, Theme, View};

use norte_code_client::PluginSummary;

#[derive(Default, Serialize)]
pub(crate) struct PluginListOverlay {
    open: bool,
    #[serde(skip)]
    plugins: Vec<PluginSummary>,
    selected: usize,
    offset: usize,
    loading: bool,
    error: Option<String>,
    #[serde(skip)]
    area: Rect,
    #[serde(skip)]
    list_area: Rect,
    #[serde(skip)]
    elapsed: Duration,
}

impl PluginListOverlay {
    pub(crate) fn open(&mut self, plugins: Vec<PluginSummary>) {
        self.open = true;
        self.plugins = plugins;
        self.selected = 0;
        self.offset = 0;
        self.loading = false;
        self.error = None;
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        self.error = None;
    }

    pub(crate) fn loaded(&mut self, plugins: Vec<PluginSummary>) {
        self.plugins = plugins;
        self.selected = self.selected.min(self.plugins.len().saturating_sub(1));
        self.loading = false;
        self.error = None;
    }

    pub(crate) fn fail(&mut self, error: String) {
        self.loading = false;
        self.error = Some(error);
    }

    pub(crate) fn sync_plugins(&mut self, plugins: &[PluginSummary]) {
        if self.open && !self.loading && self.plugins != plugins {
            self.plugins.clone_from_slice(plugins);
            self.selected = self.selected.min(self.plugins.len().saturating_sub(1));
        }
    }

    fn move_selection(&mut self, amount: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(self.plugins.len().saturating_sub(1));
    }

    fn close(&mut self) {
        self.open = false;
    }
}

impl View for PluginListOverlay {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "plugin_list_overlay"
    }

    fn focusable(&self) -> bool {
        self.open
    }

    fn tick(&mut self, delta: Duration, _cx: &mut Context) {
        if self.open {
            self.elapsed = self.elapsed.saturating_add(delta);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        render::paint(self, area, buf, theme);
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        if !self.open {
            return Outcome::Ignored;
        }
        match event {
            InputEvent::Key(key) => match key.code {
                KeyCode::Esc => self.close(),
                KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
                KeyCode::PageUp => self.move_selection(-8),
                KeyCode::PageDown => self.move_selection(8),
                KeyCode::Home => self.selected = 0,
                KeyCode::End => self.selected = self.plugins.len().saturating_sub(1),
                _ => return Outcome::Ignored,
            },
            InputEvent::Mouse(mouse)
                if self.area.contains(Position::new(mouse.column, mouse.row)) =>
            {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.move_selection(-3),
                    MouseEventKind::ScrollDown => self.move_selection(3),
                    MouseEventKind::Down(MouseButton::Left)
                        if self
                            .list_area
                            .contains(Position::new(mouse.column, mouse.row)) =>
                    {
                        let index = self.offset
                            + usize::from(mouse.row.saturating_sub(self.list_area.y)) / 2;
                        if index < self.plugins.len() {
                            self.selected = index;
                        }
                    }
                    MouseEventKind::Down(_)
                    | MouseEventKind::Up(_)
                    | MouseEventKind::Drag(_)
                    | MouseEventKind::Moved => {}
                    _ => return Outcome::Ignored,
                }
            }
            _ => return Outcome::Ignored,
        }
        Outcome::Consumed
    }
}
