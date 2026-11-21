//! The settings tab: host, agents, plugins and preferences in one scrollable view.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::{Position as RatatuiPosition, Rect};
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::components::panel::Panel;
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View, node};

use super::config_page::{ConfigAction, ConfigData, Setting};
use super::detail_panel::Field;
use super::host_info;
use super::plugins_page::PluginsAction;
use crate::ui_model::{DebugOverlayMode, PluginJob, UiModel};

/// Which list the arrow keys drive.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
enum Focus {
    #[default]
    Plugins,
    Preferences,
}

/// A prompt the user is filling in for a plugin operation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
enum Prompt {
    #[default]
    None,
    Repository,
    Package,
    ConfirmRemove,
}

/// The single settings page.
#[derive(Serialize)]
pub(crate) struct SettingsPage {
    host: Vec<Field>,
    #[serde(skip)]
    plugins: Vec<norte_code_client::PluginSummary>,
    #[serde(skip)]
    agents: Vec<norte_code_wire::AgentSummary>,
    #[serde(skip)]
    prefs: ConfigData,
    /// The selected row in each list.
    plugin_selected: usize,
    pref_selected: usize,
    focus: Focus,
    prompt: Prompt,
    repository: String,
    package: String,
    error: Option<String>,
    job: Option<PluginJob>,
    scroll: u16,
    /// Last painted area.
    #[serde(skip)]
    area: Rect,
    /// Virtual-content row where each list starts, for click hit-testing.
    #[serde(skip)]
    plugins_y: u16,
    #[serde(skip)]
    prefs_y: u16,
    /// Requests for the host, drained separately by the app.
    #[serde(skip)]
    plugin_actions: Vec<PluginsAction>,
    #[serde(skip)]
    config_actions: Vec<ConfigAction>,
    /// Host panel at index 0, plugin table at index 1.
    children: Vec<Node>,
}

impl SettingsPage {
    /// An empty page, filled by the host.
    pub(crate) fn new() -> Self {
        let header = TableRow::header(vec![
            node("h-name", TableCell::text("plugin").with_weight(5)),
            node("h-source", TableCell::text("origin").with_weight(6)),
            node("h-package", TableCell::text("package").with_weight(3)),
        ]);
        Self {
            host: Vec::new(),
            plugins: Vec::new(),
            agents: Vec::new(),
            prefs: ConfigData {
                theme: String::new(),
                thinking: String::new(),
                send_mode: String::new(),
                diff_mode: String::new(),
                rail_position: String::new(),
                debug: DebugOverlayMode::Off,
            },
            plugin_selected: 0,
            pref_selected: 0,
            focus: Focus::Plugins,
            prompt: Prompt::None,
            repository: String::new(),
            package: String::new(),
            error: None,
            job: None,
            scroll: 0,
            area: Rect::new(0, 0, 0, 0),
            plugins_y: 0,
            prefs_y: 0,
            plugin_actions: Vec::new(),
            config_actions: Vec::new(),
            children: vec![
                node(
                    "host_panel",
                    Panel::line().children(vec![node(
                        "host_table",
                        Table::new().with_header(node(
                            "header",
                            TableRow::header(vec![
                                node("h-field", TableCell::text("field").with_weight(4)),
                                node("h-value", TableCell::text("value").with_weight(10)),
                            ]),
                        )),
                    )]),
                ),
                node(
                    "plugin_table",
                    Table::new().with_header(node("header", header)),
                ),
            ],
        }
    }

    /// Refresh every section from the model.
    pub(crate) fn set_data(&mut self, model: &UiModel, prefs: ConfigData) {
        self.host = host_info::fields(model);
        self.plugins.clone_from(&model.plugins);
        self.agents.clone_from(&model.agents);
        self.job.clone_from(&model.plugin_job);
        self.prefs = prefs;
        self.plugin_selected = self
            .plugin_selected
            .min(self.plugins.len().saturating_sub(1));
        self.clamp_scroll();
    }

    /// Tab switches between the plugin and preference lists.
    pub(crate) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Plugins => Focus::Preferences,
            Focus::Preferences => Focus::Plugins,
        };
    }

    /// Take plugin requests.
    pub(crate) fn take_plugin_actions(&mut self) -> Vec<PluginsAction> {
        std::mem::take(&mut self.plugin_actions)
    }

    /// Take preference-cycle requests.
    pub(crate) fn take_config_actions(&mut self) -> Vec<ConfigAction> {
        std::mem::take(&mut self.config_actions)
    }

    fn rows(len: usize, extra: u16) -> u16 {
        extra.saturating_add(u16::try_from(len).unwrap_or(u16::MAX))
    }

    fn section_heights(&self) -> (u16, u16, u16, u16) {
        (
            Self::rows(self.host.len(), 2),
            Self::rows(self.plugins.len(), 2),
            Self::rows(Setting::ALL.len(), 2),
            Self::rows(self.agents.len(), 3),
        )
    }

    fn total_height(&self) -> u16 {
        let (host, plugins, prefs, agents) = self.section_heights();
        host + plugins + prefs + agents + 8
    }

    fn clamp_scroll(&mut self) {
        let max = self.total_height().saturating_sub(self.area.height);
        self.scroll = self.scroll.min(max);
    }

    fn plugin_rows(&self) -> Vec<Node> {
        self.plugins
            .iter()
            .enumerate()
            .map(|(index, plugin)| {
                let marker = if plugin.enabled { "✓" } else { "·" };
                let status = if plugin.loaded {
                    ""
                } else if plugin.enabled {
                    " !"
                } else {
                    ""
                };
                node(
                    format!("plugin-{index}"),
                    TableRow::body(vec![
                        node(
                            "name",
                            TableCell::text(format!("{marker} {}{status}", plugin.name))
                                .with_weight(5),
                        ),
                        node(
                            "source",
                            TableCell::text(
                                plugin.source.clone().unwrap_or_else(|| "–".to_owned()),
                            )
                            .with_weight(6),
                        ),
                        node(
                            "package",
                            TableCell::text(
                                plugin.package.clone().unwrap_or_else(|| "–".to_owned()),
                            )
                            .with_weight(3),
                        ),
                    ]),
                )
            })
            .collect()
    }

    fn move_plugin(&mut self, delta: isize) {
        if self.plugins.is_empty() {
            return;
        }
        let current = isize::try_from(self.plugin_selected).unwrap_or(0);
        let last = isize::try_from(self.plugins.len() - 1).unwrap_or(0);
        self.plugin_selected = usize::try_from((current + delta).clamp(0, last)).unwrap_or(0);
        self.focus = Focus::Plugins;
    }

    fn move_pref(&mut self, delta: isize) {
        let current = isize::try_from(self.pref_selected).unwrap_or(0);
        let last = isize::try_from(Setting::ALL.len() - 1).unwrap_or(0);
        self.pref_selected = usize::try_from((current + delta).clamp(0, last)).unwrap_or(0);
        self.focus = Focus::Preferences;
    }

    fn activate(&mut self) {
        match self.focus {
            Focus::Plugins => {
                if let Some(plugin) = self.plugins.get(self.plugin_selected) {
                    let name = plugin.name.clone();
                    match self.prompt {
                        Prompt::None => {
                            self.plugin_actions.push(PluginsAction::SetEnabled {
                                name,
                                enabled: !plugin.enabled,
                            });
                        }
                        Prompt::ConfirmRemove => {
                            self.plugin_actions.push(PluginsAction::Remove { name });
                            self.prompt = Prompt::None;
                        }
                        _ => {}
                    }
                }
            }
            Focus::Preferences => {
                self.config_actions
                    .push(ConfigAction::Cycle(Setting::ALL[self.pref_selected]));
            }
        }
    }

    fn start_install(&mut self) {
        self.repository.clear();
        self.package.clear();
        self.error = None;
        self.prompt = Prompt::Repository;
    }

    fn handle_prompt(&mut self, key: &crossterm::event::KeyEvent) -> Outcome {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc => {
                self.prompt = Prompt::None;
                self.error = None;
            }
            KeyCode::Enter => match self.prompt {
                Prompt::Repository => {
                    let url = self.repository.trim().to_owned();
                    if url.is_empty() {
                        self.error = Some("a repository URL is required".to_owned());
                    } else {
                        self.prompt = Prompt::Package;
                    }
                }
                Prompt::Package => {
                    let url = std::mem::take(&mut self.repository);
                    let package = self.package.trim();
                    self.plugin_actions.push(PluginsAction::Install {
                        url,
                        package: (!package.is_empty()).then(|| package.to_owned()),
                    });
                    self.package.clear();
                    self.prompt = Prompt::None;
                }
                Prompt::ConfirmRemove => self.activate(),
                Prompt::None => {}
            },
            KeyCode::Backspace => match self.prompt {
                Prompt::Repository => {
                    self.repository.pop();
                }
                Prompt::Package => {
                    self.package.pop();
                }
                _ => {}
            },
            KeyCode::Char(ch) => match self.prompt {
                Prompt::Repository => self.repository.push(ch),
                Prompt::Package => self.package.push(ch),
                _ => {}
            },
            _ => return Outcome::Ignored,
        }
        Outcome::Consumed
    }

    fn paint_overlay(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (title, value, caption) = match self.prompt {
            Prompt::Repository => ("install plugin", self.repository.as_str(), "repository URL"),
            Prompt::Package => (
                "install plugin",
                self.package.as_str(),
                "workspace package (optional)",
            ),
            Prompt::ConfirmRemove => {
                let name = self
                    .plugins
                    .get(self.plugin_selected)
                    .map_or("", |p| p.name.as_str());
                ("remove plugin", name, "Enter removes · Esc cancels")
            }
            Prompt::None => {
                if let Some(job) = &self.job {
                    let box_rect = Rect {
                        x: area.x + 1,
                        y: area.y + area.height.saturating_sub(3),
                        width: area.width.saturating_sub(2),
                        height: 3,
                    };
                    buf.set_style(box_rect, Style::default().bg(theme.secondary));
                    let text = format!(
                        "{} · {}/{} · {}",
                        job.operation, job.step, job.total, job.stage
                    );
                    buf.set_stringn(
                        box_rect.x + 1,
                        box_rect.y + 1,
                        &text,
                        usize::from(box_rect.width.saturating_sub(2)),
                        Style::default().fg(theme.accent).bg(theme.secondary),
                    );
                }
                return;
            }
        };
        let box_rect = Rect {
            x: area.x + 2,
            y: area.y + area.height / 3,
            width: area.width.saturating_sub(4),
            height: 5,
        };
        buf.set_style(box_rect, Style::default().bg(theme.secondary));
        buf.set_stringn(
            box_rect.x + 1,
            box_rect.y + 1,
            title,
            title.len(),
            Style::default()
                .fg(theme.accent)
                .bg(theme.secondary)
                .add_modifier(Modifier::BOLD),
        );
        buf.set_stringn(
            box_rect.x + 1,
            box_rect.y + 2,
            caption,
            usize::from(box_rect.width.saturating_sub(2)),
            Style::default().fg(theme.muted).bg(theme.secondary),
        );
        buf.set_stringn(
            box_rect.x + 1,
            box_rect.y + 3,
            value,
            usize::from(box_rect.width.saturating_sub(2)),
            Style::default().fg(theme.foreground).bg(theme.secondary),
        );
        if let Some(error) = &self.error {
            buf.set_stringn(
                box_rect.x + 1,
                box_rect.y + 4,
                error,
                usize::from(box_rect.width.saturating_sub(2)),
                Style::default().fg(theme.destructive).bg(theme.secondary),
            );
        }
    }
}

impl SettingsPage {
    fn paint_sections(&mut self, page: &mut Buffer, area: Rect, cx: &mut Context, theme: &Theme) {
        let bold = Style::default()
            .fg(theme.foreground)
            .bg(theme.background)
            .add_modifier(Modifier::BOLD);
        let normal = Style::default().fg(theme.foreground).bg(theme.background);
        let mut y = 0u16;

        let host_rows: Vec<Node> = self
            .host
            .iter()
            .map(|field| {
                node(
                    format!("field-{}", field.label),
                    TableRow::body(vec![
                        node("label", TableCell::text(field.label.clone()).with_weight(4)),
                        node(
                            "value",
                            TableCell::text(field.value.clone()).with_weight(10),
                        ),
                    ]),
                )
            })
            .collect();
        if let Some(panel) = self.children.first_mut() {
            if let Some(table) = panel
                .view
                .children_mut()
                .first_mut()
                .and_then(|node| node.view.as_any().downcast_mut::<Table>())
            {
                table.set_rows(host_rows);
            }
            panel.view.render(
                Rect {
                    x: 0,
                    y,
                    width: area.width,
                    height: Self::rows(self.host.len(), 3),
                },
                page,
                cx,
                theme,
            );
        }
        let (host, _, _, _) = self.section_heights();
        y += host;

        page.set_stringn(
            0,
            y,
            "plugins · ⏎ toggle · i install · u update · x remove",
            52,
            bold,
        );
        y += 1;
        self.plugins_y = y + 1;
        let plugin_rows = self.plugin_rows();
        if let Some(table) = self
            .children
            .get_mut(1)
            .and_then(|node| node.view.as_any().downcast_mut::<Table>())
        {
            table.set_rows(plugin_rows);
            table.state.selected = self.plugin_selected;
        }
        if let Some(node) = self.children.get_mut(1) {
            node.view.render(
                Rect {
                    x: 0,
                    y,
                    width: area.width,
                    height: Self::rows(self.plugins.len(), 3),
                },
                page,
                cx,
                theme,
            );
        }
        let (_, plugins, _, _) = self.section_heights();
        y += plugins.saturating_sub(1) + 1;

        self.prefs_y = y + 1;
        self.paint_preferences(page, area, y, bold, normal, theme);
        y += Self::rows(Setting::ALL.len(), 2);
        self.paint_agents(page, area, y, cx, theme);
    }
}

impl SettingsPage {
    fn paint_agents(
        &mut self,
        page: &mut Buffer,
        area: Rect,
        y: u16,
        cx: &mut Context,
        theme: &Theme,
    ) {
        let rows: Vec<Node> = self
            .agents
            .iter()
            .map(|agent| {
                let provider = agent
                    .provider
                    .as_ref()
                    .map_or_else(|| "–".to_owned(), |p| p.id.clone());
                let model = agent
                    .model
                    .as_ref()
                    .map_or_else(|| "–".to_owned(), |m| m.id.clone());
                let version = if agent.version.is_empty() {
                    "–".to_owned()
                } else {
                    agent.version.clone()
                };
                let capabilities = agent.capabilities.as_ref().map_or_else(
                    || "–".to_owned(),
                    |caps| format!("in {} / out {}", caps.input.len(), caps.output.len()),
                );
                node(
                    format!("agent-{}", agent.id),
                    TableRow::body(vec![
                        node("agent", TableCell::text(agent.id.clone()).with_weight(4)),
                        node("provider", TableCell::text(provider).with_weight(3)),
                        node("model", TableCell::text(model).with_weight(6)),
                        node(
                            "info",
                            TableCell::text(format!("v{version} · {capabilities}")).with_weight(3),
                        ),
                    ]),
                )
            })
            .collect();
        if let Some(panel) = self.children.get_mut(2) {
            if let Some(table) = panel
                .view
                .children_mut()
                .first_mut()
                .and_then(|node| node.view.as_any().downcast_mut::<Table>())
            {
                table.set_rows(rows);
            }
            panel.view.render(
                Rect {
                    x: 0,
                    y,
                    width: area.width,
                    height: Self::rows(self.agents.len(), 3),
                },
                page,
                cx,
                theme,
            );
        }
    }

    fn paint_preferences(
        &self,
        page: &mut Buffer,
        area: Rect,
        mut y: u16,
        bold: Style,
        normal: Style,
        theme: &Theme,
    ) {
        page.set_stringn(0, y, "preferences", 11, bold);
        y += 1;
        let values = [
            self.prefs.theme.as_str(),
            self.prefs.thinking.as_str(),
            self.prefs.send_mode.as_str(),
            self.prefs.diff_mode.as_str(),
            self.prefs.rail_position.as_str(),
            self.prefs.debug.label(),
        ];
        for (index, setting) in Setting::ALL.iter().enumerate() {
            let selected = self.focus == Focus::Preferences && index == self.pref_selected;
            if selected {
                page.set_style(
                    Rect {
                        x: 0,
                        y,
                        width: area.width,
                        height: 1,
                    },
                    Style::default().bg(theme.secondary),
                );
            }
            page.set_stringn(
                2,
                y,
                setting.label(),
                16,
                if selected { bold } else { normal },
            );
            page.set_stringn(
                20,
                y,
                values[index],
                usize::from(area.width.saturating_sub(21)),
                Style::default().fg(theme.accent).bg(theme.background),
            );
            y += 1;
        }
    }
}

impl SettingsPage {
    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Outcome {
        use crossterm::event::{MouseButton, MouseEventKind};
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(3);
                Outcome::Consumed
            }
            MouseEventKind::ScrollDown => {
                let max = self.total_height().saturating_sub(self.area.height);
                self.scroll = (self.scroll + 3).min(max);
                Outcome::Consumed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let row = self.scroll + mouse.row.saturating_sub(self.area.y);
                if row >= self.prefs_y {
                    let index = (row - self.prefs_y) as usize;
                    if index < Setting::ALL.len() {
                        self.focus = Focus::Preferences;
                        self.pref_selected = index;
                    }
                } else if row >= self.plugins_y {
                    let index = (row - self.plugins_y) as usize;
                    if index < self.plugins.len() {
                        self.focus = Focus::Plugins;
                        self.plugin_selected = index;
                    }
                }
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }

    fn handle_key(&mut self, key: &crossterm::event::KeyEvent) -> Outcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        if self.prompt != Prompt::None {
            return self.handle_prompt(key);
        }
        if !matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) {
            return Outcome::Ignored;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.focus == Focus::Plugins {
                    self.move_plugin(-1);
                } else {
                    self.move_pref(-1);
                }
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.focus == Focus::Plugins {
                    self.move_plugin(1);
                } else {
                    self.move_pref(1);
                }
                Outcome::Consumed
            }
            KeyCode::Tab => {
                self.toggle_focus();
                Outcome::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate();
                Outcome::Consumed
            }
            KeyCode::Char('i') => {
                self.start_install();
                Outcome::Consumed
            }
            KeyCode::Char('x') => {
                if self.focus == Focus::Plugins && !self.plugins.is_empty() {
                    self.prompt = Prompt::ConfirmRemove;
                }
                Outcome::Consumed
            }
            KeyCode::Char('u') => {
                if self.focus == Focus::Plugins {
                    let name = self
                        .plugins
                        .get(self.plugin_selected)
                        .map(|p| p.name.clone());
                    self.plugin_actions.push(PluginsAction::Update { name });
                }
                Outcome::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(self.area.height);
                Outcome::Consumed
            }
            KeyCode::PageDown => {
                let max = self.total_height().saturating_sub(self.area.height);
                self.scroll = (self.scroll + self.area.height).min(max);
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }
}

impl View for SettingsPage {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "settings_page"
    }

    fn children(&self) -> &[Node] {
        &self.children
    }
    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn focusable(&self) -> bool {
        true
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        self.area = area;
        self.clamp_scroll();
        buf.set_style(area, Style::default().bg(theme.background));

        let total = self.total_height().max(area.height);
        let mut page = Buffer::empty(Rect::new(0, 0, area.width, total));
        self.paint_sections(&mut page, area, cx, theme);

        // Copy the visible window onto the real buffer.
        for row in 0..area.height {
            let source = row.saturating_add(self.scroll);
            if source >= total {
                break;
            }
            for column in 0..area.width {
                if let (Some(cell), Some(source_cell)) = (
                    buf.cell_mut(RatatuiPosition::new(area.x + column, area.y + row)),
                    page.cell(RatatuiPosition::new(column, source)),
                ) {
                    *cell = source_cell.clone();
                }
            }
        }

        if self.job.is_some() || self.prompt != Prompt::None {
            self.paint_overlay(area, buf, theme);
        }
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        match event {
            InputEvent::Mouse(mouse) => self.handle_mouse(*mouse),
            InputEvent::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                self.handle_key(key)
            }
            _ => Outcome::Ignored,
        }
    }
}
