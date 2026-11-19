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
    /// The operation in flight, when the host reports one.
    #[serde(skip)]
    job: Option<crate::ui_model::PluginJob>,
    /// A validation message shown in the prompts.
    #[serde(skip)]
    error: Option<String>,
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
            job: None,
            error: None,
        }
    }

    /// Replace the operation in flight; `None` clears the progress row.
    pub(crate) fn set_job(&mut self, job: Option<crate::ui_model::PluginJob>) {
        self.job = job;
        self.clamp_view();
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
        self.area
            .height
            .saturating_sub(1 + u16::from(self.job.is_some())) as usize
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
        self.error = None;
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
        self.package.clear();
        self.repository.clear();
        self.mode = Mode::Listing;
    }

    fn type_path(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
        if key.kind != KeyEventKind::Press {
            return false;
        }
        if !matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) {
            return false;
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Listing;
                self.error = None;
                true
            }
            KeyCode::Backspace => {
                self.error = None;
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
                Mode::Repository => {
                    if looks_like_repo_url(&self.repository) {
                        self.error = None;
                        self.mode = Mode::Package;
                    } else {
                        self.error = Some("not a git repository URL".to_string());
                    }
                    true
                }
                Mode::Package => {
                    let package = self.package.trim();
                    if package.is_empty() || looks_like_package_name(package) {
                        self.error = None;
                        self.submit_install();
                    } else {
                        self.error =
                            Some("packages are lowercase letters, digits, - and _".to_string());
                    }
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
                self.error = None;
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
        if !matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) {
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
                let base = self.area.y + 1 + u16::from(self.job.is_some());
                if mouse.row >= base
                    && mouse.row < base + u16::try_from(self.body_rows()).unwrap_or(0)
                {
                    let index = self.offset + (mouse.row - base) as usize;
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
        if let Some(job) = &self.job {
            Self::paint_job(area, buf, theme, job);
        }
        match self.mode {
            Mode::Repository => {
                Self::paint_prompt(
                    area,
                    buf,
                    theme,
                    &PromptArgs {
                        title: "install plugin",
                        caption: "Repository URL",
                        value: &self.repository,
                        error: self.error.as_deref(),
                        controls: "Enter continues, Esc cancels",
                    },
                );
                return;
            }
            Mode::Package => {
                Self::paint_prompt(
                    area,
                    buf,
                    theme,
                    &PromptArgs {
                        title: "install plugin",
                        caption: "Workspace package (optional, Enter installs all)",
                        value: &self.package,
                        error: self.error.as_deref(),
                        controls: "Enter installs, Esc cancels",
                    },
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
                    &PromptArgs {
                        title: "remove plugin",
                        caption: &format!("Remove {name}?"),
                        value: "",
                        error: None,
                        controls: "Enter removes, Esc cancels",
                    },
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

/// The text every prompt asks for.
struct PromptArgs<'a> {
    /// The prompt heading, e.g. `install plugin`.
    title: &'a str,
    /// What the field wants.
    caption: &'a str,
    /// The current value; drawn with the caret.
    value: &'a str,
    /// A validation message, when the value does not qualify.
    error: Option<&'a str>,
    /// The key legend under the field.
    controls: &'a str,
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

    fn paint_prompt(area: Rect, buf: &mut Buffer, theme: &Theme, prompt: &PromptArgs<'_>) {
        let caption_rows = [
            (prompt.caption.to_string(), Style::default().fg(theme.muted)),
            (String::new(), Style::default()),
            (
                format!("{}▏", prompt.value),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            (String::new(), Style::default()),
            (
                prompt.controls.to_string(),
                Style::default().fg(theme.muted),
            ),
        ];
        let mut y = area.y + 3;
        buf.set_stringn(
            area.x + 1,
            y,
            prompt.title,
            prompt.title.chars().count(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        y += 1;
        for (line, style) in caption_rows {
            buf.set_stringn(area.x + 1, y, &line, line.chars().count(), style);
            y += 1;
        }
        if let Some(error) = prompt.error {
            buf.set_stringn(
                area.x + 1,
                y,
                error,
                error.chars().count(),
                Style::default().fg(theme.destructive),
            );
        }
    }

    fn paint_job(area: Rect, buf: &mut Buffer, theme: &Theme, job: &crate::ui_model::PluginJob) {
        let y = area.y + 1;
        let total = job.total.max(1);
        let cells = 16u16;
        let filled = u16::try_from((u64::from(cells) * job.step.min(total)).div_ceil(total))
            .unwrap_or(0)
            .min(cells);
        let bar = format!(
            "{}{}",
            "█".repeat(usize::from(filled)),
            "░".repeat(usize::from(cells - filled))
        );
        let line = format!(
            "{} · {bar} {}/{} · {}",
            job.operation,
            job.step.min(total),
            total,
            job.stage
        );
        buf.set_stringn(
            area.x + 1,
            y,
            &line,
            usize::from(area.width.saturating_sub(1)),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
    }

    fn paint_list(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let rows = self.body_rows();
        let end = (self.offset + rows).min(self.plugins.len());
        let base = area.y + 1 + u16::from(self.job.is_some());
        for (offset, index) in (self.offset..end).enumerate() {
            let y = base + u16::try_from(offset).unwrap_or(0);
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

/// Accepts `http(s)`, `ssh`, `git` URLs and `git@host:path`-style references.
fn looks_like_repo_url(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.contains(char::is_whitespace) {
        return false;
    }
    if let Some(rest) = trimmed.strip_prefix("git@") {
        return rest.split_once(':').is_some_and(|(host, path)| {
            !host.is_empty() && !path.is_empty() && !host.contains(':')
        });
    }
    for scheme in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = trimmed.strip_prefix(scheme) {
            let host = rest.split('/').next().unwrap_or("");
            return !host.is_empty();
        }
    }
    false
}

/// Cargo package names: lowercase letters, digits, '-', '_'; at least one char.
fn looks_like_package_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
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
