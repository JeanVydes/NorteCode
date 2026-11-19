//! The plugins page: what the host has installed, and what to do with it.
//!
//! Every action sits on the toolbar: install (a git repository the host
//! clones, with an optional workspace package), update, update all,
//! remove. Enter on a row toggles the plugin between enabled and disabled.
//! The toolbar buttons are clickable and the keys work too: a install,
//! u update, U update all, x remove, ⏎ toggle. Operations go out as
//! actions the host drains; afterwards the host re-lists the plugins.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// What the page is doing over the list.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
enum Mode {
    #[default]
    /// Just browsing and acting on plugins.
    Listing,
    /// Typing the repository URL to install.
    Repository,
    /// Typing the optional workspace package; empty installs all.
    Package,
    /// Confirming the removal of the selected plugin.
    ConfirmRemove,
}

/// One toolbar button, in draw order.
const ACTIONS: [Action; 4] = [
    Action::Install,
    Action::Update,
    Action::UpdateAll,
    Action::Remove,
];

/// The toolbar actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    /// Install a plugin from a git repository.
    Install,
    /// Re-clone the selected plugin's source.
    Update,
    /// Re-clone every plugin with a recorded source.
    UpdateAll,
    /// Delete the selected plugin's library.
    Remove,
}

impl Action {
    const fn label(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::UpdateAll => "update all",
            Self::Remove => "remove",
        }
    }

    const fn hint(self) -> &'static str {
        match self {
            Self::Install => "clone a git repository and build the plugin",
            Self::Update => "re-clone the selected plugin's source",
            Self::UpdateAll => "re-clone every plugin with a recorded source",
            Self::Remove => "delete the selected plugin's library",
        }
    }
}

/// A pending request to the app layer, drained each frame.
#[derive(Serialize)]
pub(crate) enum PluginsAction {
    /// Install a repository, optionally one package.
    Install {
        /// The repository URL.
        url: String,
        /// One workspace package, or the whole workspace.
        package: Option<String>,
    },
    /// Update one plugin, or all when `name` is `None`.
    Update {
        /// The plugin, or every plugin when missing.
        name: Option<String>,
    },
    /// Remove one plugin.
    Remove {
        /// The plugin to remove.
        name: String,
    },
    /// Enable or disable one plugin.
    SetEnabled {
        /// The plugin to toggle.
        name: String,
        /// What it should be after the change.
        enabled: bool,
    },
    /// Re-fetch the plugin list.
    Refresh,
}

/// The plugins page.
#[derive(Serialize)]
pub(crate) struct PluginsPage {
    /// Installed plugins, in host order.
    #[serde(skip)]
    plugins: Vec<norte_code_client::PluginSummary>,
    /// The selected row.
    selected: usize,
    /// The first visible row.
    offset: usize,
    /// What the page is doing over the list.
    #[serde(default)]
    mode: Mode,
    /// The URL being typed for install.
    #[serde(default)]
    repository: String,
    /// The package being typed for install.
    #[serde(default)]
    package: String,
    /// The last drawn area.
    #[serde(skip)]
    area: Rect,
    /// The toolbar row, for click hit-testing.
    #[serde(skip)]
    toolbar: Rect,
    /// Requests waiting for the host.
    #[serde(skip)]
    pending: Vec<PluginsAction>,
    /// The hovered toolbar button, for the accent swap.
    #[serde(skip)]
    hovered: Option<usize>,
}

impl PluginsPage {
    /// Creates a page with an empty plugin list.
    pub(crate) fn new() -> Self {
        Self {
            plugins: Vec::new(),
            selected: 0,
            offset: 0,
            mode: Mode::Listing,
            repository: String::new(),
            package: String::new(),
            area: Rect::new(0, 0, 0, 0),
            toolbar: Rect::new(0, 0, 0, 0),
            pending: Vec::new(),
            hovered: None,
        }
    }

    /// Replace the plugin list; selection is preserved when possible.
    pub(crate) fn set_data(&mut self, plugins: Vec<norte_code_client::PluginSummary>) {
        self.plugins = plugins;
        self.selected = self.selected.min(self.plugins.len().saturating_sub(1));
        self.clamp_view();
    }

    /// Take any pending requests the host should action.
    pub(crate) fn take_pending(&mut self) -> Vec<PluginsAction> {
        std::mem::take(&mut self.pending)
    }

    fn body_rows(&self) -> usize {
        self.area.height.saturating_sub(1) as usize
    }

    fn selected_plugin(&self) -> Option<&norte_code_client::PluginSummary> {
        self.plugins.get(self.selected)
    }

    fn clamp_view(&mut self) {
        let body = self.body_rows();
        if self.plugins.len() <= body {
            self.offset = 0;
            return;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + body {
            self.offset = self.selected + 1 - body;
        }
    }

    fn start_install(&mut self) {
        self.repository.clear();
        self.package.clear();
        self.mode = Mode::Repository;
    }

    fn run(&mut self, action: Action) {
        match action {
            Action::Install => self.start_install(),
            Action::Update => {
                if let Some(plugin) = self.selected_plugin() {
                    self.pending.push(PluginsAction::Update {
                        name: Some(plugin.name.clone()),
                    });
                }
            }
            Action::UpdateAll => self.pending.push(PluginsAction::Update { name: None }),
            Action::Remove => {
                if self.selected_plugin().is_some() {
                    self.mode = Mode::ConfirmRemove;
                }
            }
        }
    }

    fn submit_install(&mut self) {
        let url = self.repository.trim().to_owned();
        let package = self.package.trim();
        self.pending.push(PluginsAction::Install {
            url,
            package: (!package.is_empty()).then(|| package.to_owned()),
        });
        self.mode = Mode::Listing;
    }

    fn type_path(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        if key.modifiers != KeyModifiers::NONE {
            return false;
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Listing;
                true
            }
            KeyCode::Backspace => {
                match self.mode {
                    Mode::Repository => {
                        self.repository.pop();
                    }
                    Mode::Package => {
                        self.package.pop();
                    }
                    Mode::ConfirmRemove | Mode::Listing => {}
                }
                true
            }
            KeyCode::Enter => match self.mode {
                Mode::Repository if !self.repository.trim().is_empty() => {
                    self.mode = Mode::Package;
                    true
                }
                Mode::Repository => true,
                Mode::Package => {
                    self.submit_install();
                    true
                }
                Mode::ConfirmRemove => {
                    if let Some(plugin) = self.selected_plugin() {
                        self.pending.push(PluginsAction::Remove {
                            name: plugin.name.clone(),
                        });
                    }
                    self.mode = Mode::Listing;
                    true
                }
                Mode::Listing => false,
            },
            KeyCode::Char(character) => {
                match self.mode {
                    Mode::Repository => self.repository.push(character),
                    Mode::Package => self.package.push(character),
                    Mode::ConfirmRemove | Mode::Listing => {}
                }
                true
            }
            _ => false,
        }
    }

    fn handle_listing_key(&mut self, key: &crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};
        if key.modifiers != KeyModifiers::NONE {
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.plugins.len().saturating_sub(1));
            }
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(self.body_rows().max(1));
            }
            KeyCode::PageDown => {
                self.selected = (self.selected + self.body_rows().max(1))
                    .min(self.plugins.len().saturating_sub(1));
            }
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.plugins.len().saturating_sub(1),
            KeyCode::Enter => {
                if let Some(plugin) = self.selected_plugin() {
                    self.pending.push(PluginsAction::SetEnabled {
                        name: plugin.name.clone(),
                        enabled: !plugin.enabled,
                    });
                }
            }
            KeyCode::Char('a') => self.start_install(),
            KeyCode::Char('u') => self.run(Action::Update),
            KeyCode::Char('U') => self.run(Action::UpdateAll),
            KeyCode::Char('x' | 'd') => self.run(Action::Remove),
            KeyCode::Char('r') => self.pending.push(PluginsAction::Refresh),
            _ => return,
        }
        self.clamp_view();
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Outcome {
        use crossterm::event::{MouseButton, MouseEventKind};
        if self.mode != Mode::Listing {
            return Outcome::Ignored;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.selected = self.selected.saturating_sub(3);
                self.clamp_view();
                Outcome::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.selected = (self.selected + 3).min(self.plugins.len().saturating_sub(1));
                self.clamp_view();
                Outcome::Consumed
            }
            MouseEventKind::Moved => {
                self.hovered = action_at(self.toolbar, mouse.column, mouse.row);
                Outcome::Ignored
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = action_at(self.toolbar, mouse.column, mouse.row) {
                    self.run(ACTIONS[index]);
                    return Outcome::Consumed;
                }
                if mouse.row > self.area.y {
                    let index = self.offset + (mouse.row - self.area.y - 1) as usize;
                    if index < self.plugins.len() {
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

impl View for PluginsPage {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "plugins_page"
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
        self.paint_toolbar(area, buf, theme);
        match self.mode {
            Mode::Repository => {
                Self::paint_prompt(
                    area,
                    buf,
                    theme,
                    "install plugin",
                    "Repository URL",
                    &self.repository.clone(),
                    "Enter continues, Esc cancels",
                );
                return;
            }
            Mode::Package => {
                Self::paint_prompt(
                    area,
                    buf,
                    theme,
                    "install plugin",
                    "Workspace package (optional, Enter installs all)",
                    &self.package.clone(),
                    "Enter installs, Esc cancels",
                );
                return;
            }
            Mode::ConfirmRemove => {
                let name = self
                    .selected_plugin()
                    .map(|plugin| plugin.name.clone())
                    .unwrap_or_default();
                Self::paint_prompt(
                    area,
                    buf,
                    theme,
                    "remove plugin",
                    &format!("Remove {name}?"),
                    "",
                    "Enter removes, Esc cancels",
                );
                return;
            }
            Mode::Listing => {}
        }
        self.paint_list(area, buf, theme);
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        use crossterm::event::KeyEventKind;
        match event {
            InputEvent::Mouse(mouse) => self.handle_mouse(*mouse),
            InputEvent::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Outcome::Ignored;
                }
                if self.mode != Mode::Listing {
                    return if self.type_path(key) {
                        Outcome::Consumed
                    } else {
                        Outcome::Ignored
                    };
                }
                self.handle_listing_key(key);
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }
}

impl PluginsPage {
    fn paint_toolbar(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let mut x = area.x.saturating_add(1);
        for (index, action) in ACTIONS.iter().enumerate() {
            let label = format!(" {} ", action.label());
            let hovered = self.hovered == Some(index);
            let style = if hovered {
                Style::default()
                    .fg(theme.background)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.secondary)
                    .add_modifier(Modifier::BOLD)
            };
            let width = u16::try_from(label.chars().count()).unwrap_or(0);
            buf.set_stringn(x, area.y, &label, usize::from(width), style);
            x = x.saturating_add(width + 1);
        }
        let hint = self
            .hovered
            .and_then(|index| ACTIONS.get(index))
            .map_or("⏎ enable/disable", |action| action.hint());
        buf.set_stringn(
            x.saturating_add(1),
            area.y,
            hint,
            hint.chars().count(),
            Style::default().fg(theme.muted),
        );
        self.toolbar = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
    }

    fn paint_prompt(
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        title: &str,
        caption: &str,
        value: &str,
        controls: &str,
    ) {
        let caption_rows = [
            (caption.to_string(), Style::default().fg(theme.muted)),
            (String::new(), Style::default()),
            (
                format!("{value}▏"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            (String::new(), Style::default()),
            (controls.to_string(), Style::default().fg(theme.muted)),
        ];
        let mut y = area.y + 3;
        buf.set_stringn(
            area.x + 1,
            y,
            title,
            title.chars().count(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        y += 1;
        for (line, style) in caption_rows {
            buf.set_stringn(area.x + 1, y, &line, line.chars().count(), style);
            y += 1;
        }
    }

    fn paint_list(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let rows = self.body_rows();
        let end = (self.offset + rows).min(self.plugins.len());
        for (offset, index) in (self.offset..end).enumerate() {
            let y = area.y + 1 + u16::try_from(offset).unwrap_or(0);
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
            let plugin = &self.plugins[index];
            let (glyph, color) = if plugin.load_error.is_some() {
                ("✗", theme.destructive)
            } else if plugin.enabled && plugin.loaded {
                ("●", theme.success)
            } else if !plugin.enabled {
                ("○", theme.muted)
            } else {
                ("!", theme.warning)
            };
            let marker = format!(" {glyph} ");
            buf.set_stringn(
                area.x,
                y,
                &marker,
                marker.chars().count(),
                Style::default().fg(color),
            );
            let name_style = if selected {
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            buf.set_stringn(
                area.x + 3,
                y,
                &plugin.name,
                plugin.name.chars().count(),
                name_style,
            );
            let state = if let Some(error) = &plugin.load_error {
                format!("    {}", clip(error, 44))
            } else if plugin.enabled && plugin.loaded {
                "    loaded".to_string()
            } else if !plugin.enabled {
                "    disabled".to_string()
            } else {
                "    enabled".to_string()
            };
            buf.set_stringn(
                area.x + 3 + u16::try_from(plugin.name.chars().count()).unwrap_or(0),
                y,
                &state,
                state.chars().count(),
                Style::default().fg(theme.muted),
            );
        }
        if self.plugins.is_empty() {
            let text = "no plugins installed · a installs from a git repository";
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

/// Which toolbar button sits under a column.
fn action_at(toolbar: Rect, column: u16, row: u16) -> Option<usize> {
    if row != toolbar.y {
        return None;
    }
    let mut start = toolbar.x.saturating_add(1);
    for (index, action) in ACTIONS.iter().enumerate() {
        let width = u16::try_from(action.label().chars().count() + 2).unwrap_or(0);
        if column >= start && column < start.saturating_add(width) {
            return Some(index);
        }
        start = start.saturating_add(width).saturating_add(1);
    }
    None
}

#[cfg(test)]
#[path = "plugins_page_tests.rs"]
mod tests;
