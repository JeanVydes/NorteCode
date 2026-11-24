//! The settings tab: host, plugins, preferences and agents in one scrollable view.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::{Position as RatatuiPosition, Rect};
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::components::panel::Panel;
use terminalui::components::scroll::{Scroll, ScrollbarStyle, render_scrollbar};
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View, node};

use super::config_page::{ConfigAction, ConfigData, Setting};
use super::detail_panel::Field;
use super::host_info;
use super::plugins_page::PluginsAction;
use super::settings_layout::{SettingsLayout, SettingsSection};
use crate::ui_model::{DebugOverlayMode, PluginJob, UiModel};

const HOST_CHILD: usize = 0;
const AGENT_CHILD: usize = 1;
const PLUGIN_CHILD: usize = 2;

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

#[derive(Clone, Copy)]
struct PreferenceStyle {
    label: Style,
    selected_label: Style,
    selected_bg: Style,
    value: Style,
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
    plugin_selected: usize,
    pref_selected: usize,
    focus: Focus,
    prompt: Prompt,
    repository: String,
    package: String,
    error: Option<String>,
    job: Option<PluginJob>,
    scroll: u16,
    #[serde(skip)]
    area: Rect,
    #[serde(skip)]
    plugins_y: u16,
    #[serde(skip)]
    prefs_y: u16,
    #[serde(skip)]
    plugin_actions: Vec<PluginsAction>,
    #[serde(skip)]
    config_actions: Vec<ConfigAction>,
    children: Vec<Node>,
}

impl SettingsPage {
    /// An empty page, filled by the host.
    pub(crate) fn new() -> Self {
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
            focus: Focus::Preferences,
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
                                node("h-field", TableCell::text("field").with_weight(3)),
                                node("h-value", TableCell::text("value").with_weight(9)),
                            ]),
                        )),
                    )]),
                ),
                node(
                    "agent_panel",
                    Panel::line().children(vec![node(
                        "agent_table",
                        Table::new().with_header(node(
                            "header",
                            TableRow::header(vec![
                                node("h-agent", TableCell::text("agent").with_weight(3)),
                                node("h-provider", TableCell::text("provider").with_weight(3)),
                                node("h-model", TableCell::text("model").with_weight(6)),
                                node("h-info", TableCell::text("info").with_weight(4)),
                            ]),
                        )),
                    )]),
                ),
                node(
                    "plugin_table",
                    Table::new().with_header(node(
                        "header",
                        TableRow::header(vec![
                            node("h-name", TableCell::text("plugin").with_weight(5)),
                            node("h-source", TableCell::text("origin").with_weight(5)),
                            node("h-package", TableCell::text("package").with_weight(3)),
                        ]),
                    )),
                ),
            ],
        }
    }

    /// Refresh every section from the model.
    pub(crate) fn set_data(&mut self, model: &UiModel, prefs: ConfigData) {
        let host = host_info::fields(model);
        if self.host == host
            && self.plugins == model.plugins
            && self.agents == model.agents
            && self.job == model.plugin_job
            && self.prefs == prefs
        {
            return;
        }
        self.host = host;
        self.plugins.clone_from(&model.plugins);
        self.agents.clone_from(&model.agents);
        self.job.clone_from(&model.plugin_job);
        self.prefs = prefs;
        self.plugin_selected = self
            .plugin_selected
            .min(self.plugins.len().saturating_sub(1));
        self.clamp_scroll();
    }

    /// Tab switches between the preference and plugin lists.
    pub(crate) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Preferences => Focus::Plugins,
            Focus::Plugins => Focus::Preferences,
        };
        self.ensure_selected_visible();
    }

    /// Take plugin requests.
    pub(crate) fn take_plugin_actions(&mut self) -> Vec<PluginsAction> {
        std::mem::take(&mut self.plugin_actions)
    }

    /// Take preference-cycle requests.
    pub(crate) fn take_config_actions(&mut self) -> Vec<ConfigAction> {
        std::mem::take(&mut self.config_actions)
    }

    fn layout(&self) -> SettingsLayout {
        SettingsLayout::new(
            self.area.width,
            self.host.len(),
            self.plugins.len(),
            self.agents.len(),
            Setting::ALL.len(),
        )
    }

    fn clamp_scroll(&mut self) {
        let max = self
            .layout()
            .total_height()
            .saturating_sub(self.area.height);
        self.scroll = self.scroll.min(max);
    }

    fn ensure_selected_visible(&mut self) {
        let layout = self.layout();
        let first_row = match self.focus {
            Focus::Preferences => layout.preference_rows_y(),
            Focus::Plugins => layout.plugin_rows_y(),
        };
        let selected = match self.focus {
            Focus::Preferences => self.pref_selected,
            Focus::Plugins => self.plugin_selected,
        };
        let row = first_row.saturating_add(u16::try_from(selected).unwrap_or(u16::MAX));
        let bottom = self.scroll.saturating_add(self.area.height);
        if row < self.scroll {
            self.scroll = row;
        } else if row >= bottom {
            self.scroll = row.saturating_sub(self.area.height.saturating_sub(1));
        }
        self.clamp_scroll();
    }

    fn plugin_rows(&self) -> Vec<Node> {
        self.plugins
            .iter()
            .enumerate()
            .map(|(index, plugin)| {
                let marker = if plugin.enabled { "✓" } else { "x" };
                let status = if plugin.loaded {
                    ""
                } else if plugin.enabled {
                    " !"
                } else {
                    ""
                };
                let source = plugin.source.clone().unwrap_or_else(|| "-".to_owned());
                let package = plugin.package.clone().unwrap_or_else(|| "-".to_owned());
                node(
                    format!("plugin-{index}"),
                    TableRow::body(vec![
                        node(
                            "name",
                            TableCell::text(format!("{marker} {}{status}", plugin.name))
                                .with_weight(5),
                        ),
                        node("source", TableCell::text(source).with_weight(5)),
                        node("package", TableCell::text(package).with_weight(3)),
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
        self.ensure_selected_visible();
    }

    fn move_pref(&mut self, delta: isize) {
        let current = isize::try_from(self.pref_selected).unwrap_or(0);
        let last = isize::try_from(Setting::ALL.len() - 1).unwrap_or(0);
        self.pref_selected = usize::try_from((current + delta).clamp(0, last)).unwrap_or(0);
        self.focus = Focus::Preferences;
        self.ensure_selected_visible();
    }

    fn activate(&mut self) {
        match self.focus {
            Focus::Plugins => {
                if let Some(plugin) = self.plugins.get(self.plugin_selected) {
                    let name = plugin.name.clone();
                    match self.prompt {
                        Prompt::None => self.plugin_actions.push(PluginsAction::SetEnabled {
                            name,
                            enabled: !plugin.enabled,
                        }),
                        Prompt::ConfirmRemove => {
                            self.plugin_actions.push(PluginsAction::Remove { name });
                            self.prompt = Prompt::None;
                        }
                        _ => {}
                    }
                }
            }
            Focus::Preferences => self
                .config_actions
                .push(ConfigAction::Cycle(Setting::ALL[self.pref_selected])),
        }
    }

    fn start_install(&mut self) {
        self.repository.clear();
        self.package.clear();
        self.error = None;
        self.focus = Focus::Plugins;
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
                    .map_or("", |plugin| plugin.name.as_str());
                ("remove plugin", name, "Enter removes, Esc cancels")
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

    fn paint_sections(&mut self, page: &mut Buffer, area: Rect, cx: &mut Context, theme: &Theme) {
        let layout = self.layout();
        let margin = SettingsLayout::MARGIN;
        let width = area.width.saturating_sub(margin.saturating_mul(2));
        let background = Style::default().bg(theme.background);
        let heading = background.fg(theme.accent).add_modifier(Modifier::BOLD);
        let preference_style = PreferenceStyle {
            label: background.fg(theme.foreground),
            selected_label: background.fg(theme.foreground).add_modifier(Modifier::BOLD),
            selected_bg: Style::default().bg(theme.secondary),
            value: Style::default().fg(theme.accent).bg(theme.background),
        };

        let prefs = layout.section(SettingsSection::Preferences);
        page.set_stringn(margin, prefs.y, "preferences", 11, heading);
        self.prefs_y = prefs.y + 1;
        self.paint_preferences(page, &layout, prefs.y + 1, width, &preference_style);

        let host = layout.section(SettingsSection::Host);
        page.set_stringn(margin, host.y, "host", 4, heading);
        let host_rows = self
            .host
            .iter()
            .map(|field| {
                node(
                    format!("field-{}", field.label),
                    TableRow::body(vec![
                        node("label", TableCell::text(field.label.clone()).with_weight(3)),
                        node("value", TableCell::text(field.value.clone()).with_weight(9)),
                    ]),
                )
            })
            .collect();
        if let Some(panel) = self.children.get_mut(HOST_CHILD) {
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
                    x: margin,
                    y: host.y + 1,
                    width,
                    height: host.height.saturating_sub(1),
                },
                page,
                cx,
                theme,
            );
        }

        let agents = layout.section(SettingsSection::Agents);
        page.set_stringn(margin, agents.y, "agents", 6, heading);
        self.paint_agents(
            page,
            Rect {
                x: margin,
                y: agents.y + 1,
                width,
                height: agents.height.saturating_sub(1),
            },
            cx,
            theme,
        );

        let plugins = layout.section(SettingsSection::Plugins);
        let caption = "plugins | Enter toggle | i install | u update | x remove";
        page.set_stringn(margin, plugins.y, caption, usize::from(width), heading);
        self.plugins_y = layout.plugin_rows_y();
        let plugin_rows = self.plugin_rows();
        if let Some(table) = self
            .children
            .get_mut(PLUGIN_CHILD)
            .and_then(|node| node.view.as_any().downcast_mut::<Table>())
        {
            table.set_rows(plugin_rows);
            table.state.selected = self.plugin_selected;
        }
        if let Some(table) = self.children.get_mut(PLUGIN_CHILD) {
            table.view.render(
                Rect {
                    x: margin,
                    y: plugins.y + 1,
                    width,
                    height: plugins.height.saturating_sub(1),
                },
                page,
                cx,
                theme,
            );
        }
    }

    fn paint_agents(&mut self, page: &mut Buffer, rect: Rect, cx: &mut Context, theme: &Theme) {
        let rows = self
            .agents
            .iter()
            .map(|agent| {
                let provider = agent
                    .provider
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), |item| item.id.clone());
                let model = agent
                    .model
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), |item| item.id.clone());
                let version = if agent.version.is_empty() {
                    "-".to_owned()
                } else {
                    agent.version.clone()
                };
                let capabilities = agent.capabilities.as_ref().map_or_else(
                    || "-".to_owned(),
                    |caps| format!("in {} / out {}", caps.input.len(), caps.output.len()),
                );
                node(
                    format!("agent-{}", agent.id),
                    TableRow::body(vec![
                        node("agent", TableCell::text(agent.id.clone()).with_weight(3)),
                        node("provider", TableCell::text(provider).with_weight(3)),
                        node("model", TableCell::text(model).with_weight(6)),
                        node(
                            "info",
                            TableCell::text(format!("v{version} {capabilities}")).with_weight(4),
                        ),
                    ]),
                )
            })
            .collect();
        if let Some(panel) = self.children.get_mut(AGENT_CHILD) {
            if let Some(table) = panel
                .view
                .children_mut()
                .first_mut()
                .and_then(|node| node.view.as_any().downcast_mut::<Table>())
            {
                table.set_rows(rows);
            }
            panel.view.render(rect, page, cx, theme);
        }
    }

    fn paint_preferences(
        &self,
        page: &mut Buffer,
        layout: &SettingsLayout,
        y: u16,
        width: u16,
        style: &PreferenceStyle,
    ) {
        let values = [
            self.prefs.theme.as_str(),
            self.prefs.thinking.as_str(),
            self.prefs.send_mode.as_str(),
            self.prefs.diff_mode.as_str(),
            self.prefs.rail_position.as_str(),
            self.prefs.debug.label(),
        ];
        let value_width = width.saturating_sub(layout.label_width().saturating_add(2));
        for (index, setting) in Setting::ALL.iter().enumerate() {
            let row = y.saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
            let selected = self.focus == Focus::Preferences && index == self.pref_selected;
            if selected {
                page.set_style(
                    Rect {
                        x: SettingsLayout::MARGIN,
                        y: row,
                        width,
                        height: 1,
                    },
                    style.selected_bg,
                );
            }
            let label_style = if selected {
                style.selected_label
            } else {
                style.label
            };
            page.set_stringn(
                SettingsLayout::MARGIN,
                row,
                setting.label(),
                usize::from(layout.label_width()),
                label_style,
            );
            page.set_stringn(
                layout.value_x(),
                row,
                values[index],
                usize::from(value_width),
                style.value,
            );
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Outcome {
        use crossterm::event::{MouseButton, MouseEventKind};
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(3);
                Outcome::Consumed
            }
            MouseEventKind::ScrollDown => {
                let max = self
                    .layout()
                    .total_height()
                    .saturating_sub(self.area.height);
                self.scroll = (self.scroll + 3).min(max);
                Outcome::Consumed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let scrollbar_x = self
                    .area
                    .x
                    .saturating_add(self.area.width.saturating_sub(1));
                if mouse.column >= scrollbar_x {
                    return Outcome::Consumed;
                }
                let row = self.scroll + mouse.row.saturating_sub(self.area.y);
                let layout = self.layout();
                if let Some(index) = layout.preference_row(row) {
                    self.focus = Focus::Preferences;
                    self.pref_selected = index;
                } else if let Some(index) = layout.plugin_row(row) {
                    self.focus = Focus::Plugins;
                    self.plugin_selected = index;
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
            KeyCode::Char('x') if self.focus == Focus::Plugins && !self.plugins.is_empty() => {
                self.prompt = Prompt::ConfirmRemove;
                Outcome::Consumed
            }
            KeyCode::Char('u') if self.focus == Focus::Plugins => {
                let name = self
                    .plugins
                    .get(self.plugin_selected)
                    .map(|plugin| plugin.name.clone());
                self.plugin_actions.push(PluginsAction::Update { name });
                Outcome::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(self.area.height);
                Outcome::Consumed
            }
            KeyCode::PageDown => {
                let max = self
                    .layout()
                    .total_height()
                    .saturating_sub(self.area.height);
                self.scroll = (self.scroll + self.area.height).min(max);
                Outcome::Consumed
            }
            KeyCode::Home => {
                self.scroll = 0;
                Outcome::Consumed
            }
            KeyCode::End => {
                self.scroll = self
                    .layout()
                    .total_height()
                    .saturating_sub(self.area.height);
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

        let total = self.layout().total_height().max(area.height);
        let virtual_area = Rect::new(0, 0, area.width, total);
        let mut page = Buffer::empty(virtual_area);
        page.set_style(virtual_area, Style::default().bg(theme.background));
        self.paint_sections(&mut page, virtual_area, cx, theme);

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

        if total > area.height {
            let scroll_style = ScrollbarStyle {
                track_color: Some(theme.background),
                ..ScrollbarStyle::default()
            };
            let scroll = Scroll {
                offset: usize::from(self.scroll),
                viewport: usize::from(area.height),
                total: usize::from(total),
            };
            render_scrollbar(buf, area, &scroll_style, theme, &scroll);
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

#[cfg(test)]
#[path = "settings_page_tests.rs"]
mod tests;
