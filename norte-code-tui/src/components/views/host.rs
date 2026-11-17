//! What the host offers, in sections the reader can reach by key or by click.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::Line,
    widgets::{Block, Borders},
};

use crate::{
    components::{
        Component, ComponentAction, ComponentId, ComponentLifecycle, LifecyclePhase, McpState,
        ToolRuleAction, ViewAction, palette, panel, spacing,
    },
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum HostSection {
    #[default]
    Overview,
    Skills,
    Mcp,
    Approvals,
}

impl HostSection {
    /// Every section, in the order the tabs show them.
    const ALL: [Self; 4] = [Self::Overview, Self::Skills, Self::Mcp, Self::Approvals];

    const fn title(self) -> &'static str {
        match self {
            Self::Overview => "Host",
            Self::Skills => "Skills",
            Self::Mcp => "MCP",
            Self::Approvals => "Tools",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Skills => 1,
            Self::Mcp => 2,
            Self::Approvals => 3,
        }
    }

    /// The section a tab index names.
    fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    fn next(self) -> Self {
        let index = (self.index() + 1) % Self::ALL.len();
        Self::from_index(index).unwrap_or_default()
    }

    fn previous(self) -> Self {
        let index = (self.index() + Self::ALL.len() - 1) % Self::ALL.len();
        Self::from_index(index).unwrap_or_default()
    }
}

#[derive(Default)]
pub(in crate::components) struct HostView {
    lifecycle: ComponentLifecycle,
    section: HostSection,
    selected: usize,
    pending_action: Option<ComponentAction>,
    /// The columns each section tab spans, so a click opens it.
    tabs: Vec<(u16, u16, HostSection)>,
    /// Where the view last drew, so a click maps to a tab.
    last_area: Rect,
}

impl HostView {
    fn select_next(&mut self, model: &UiModel, down: bool) {
        let length = match self.section {
            HostSection::Overview => model.agents.len(),
            HostSection::Skills => model
                .skills
                .as_ref()
                .map_or(0, |catalog| catalog.skills.len()),
            HostSection::Mcp => model
                .mcp
                .as_ref()
                .map_or(0, |catalog| catalog.servers.len()),
            HostSection::Approvals => model.tool_rules.len(),
        };
        panel::move_selection(&mut self.selected, length, down);
    }

    fn selected_mcp<'a>(&self, model: &'a UiModel) -> Option<&'a norte_code_wire::McpInfo> {
        model.mcp.as_ref()?.servers.get(self.selected)
    }

    fn selected_tool<'a>(&self, model: &'a UiModel) -> Option<&'a norte_code_client::ToolRule> {
        model.tool_rules.get(self.selected)
    }

    /// The tab at a column, when the reader clicked one.
    fn tab_at(&self, column: u16, row: u16) -> Option<HostSection> {
        if row != self.last_area.y {
            return None;
        }
        self.tabs
            .iter()
            .find(|(start, end, _)| column >= *start && column < *end)
            .map(|(_, _, section)| *section)
    }

    fn render_section(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        match self.section {
            HostSection::Overview => {
                let lines = crate::text_builder::host::overview(model, model.preferences.theme);
                panel::render_lines(
                    frame,
                    area,
                    TextBuilder::host_title(),
                    lines,
                    model.preferences.theme,
                );
            }
            HostSection::Skills => self.render_skills(frame, area, model),
            HostSection::Mcp => self.render_mcp(frame, area, model),
            HostSection::Approvals => self.render_approvals(frame, area, model),
        }
    }

    fn render_skills(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let entries = model.skills.as_ref().map_or_else(Vec::new, |catalog| {
            catalog
                .skills
                .iter()
                .map(TextBuilder::skill_row)
                .chain(
                    catalog
                        .warnings
                        .iter()
                        .map(|warning| TextBuilder::warning_row(warning.as_str())),
                )
                .collect()
        });
        panel::render_list(
            frame,
            area,
            entries.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Host,
                phase: if model.skills.is_some() {
                    LifecyclePhase::Ready
                } else {
                    LifecyclePhase::Loading
                },
                selected: Some(self.selected),
                focused: true,
                empty_message: "No skills loaded",
            },
        );
    }

    fn render_mcp(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let entries = model.mcp.as_ref().map_or_else(Vec::new, |catalog| {
            catalog
                .servers
                .iter()
                .map(TextBuilder::mcp_row)
                .chain(catalog.failures.iter().map(TextBuilder::mcp_failure_row))
                .collect()
        });
        panel::render_list(
            frame,
            area,
            entries.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Host,
                phase: if model.mcp.is_some() {
                    LifecyclePhase::Ready
                } else {
                    LifecyclePhase::Loading
                },
                selected: Some(self.selected),
                focused: true,
                empty_message: "No MCP servers loaded",
            },
        );
    }

    fn render_approvals(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let entries: Vec<Line<'static>> = model
            .tool_rules
            .iter()
            .map(TextBuilder::tool_rule_row)
            .collect();
        panel::render_list(
            frame,
            area,
            entries.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Host,
                phase: if model.tool_rules.is_empty() {
                    LifecyclePhase::Empty
                } else {
                    LifecyclePhase::Ready
                },
                selected: Some(self.selected),
                focused: true,
                empty_message: "No tool approval rules loaded",
            },
        );
    }
}

impl Component for HostView {
    fn id(&self) -> ComponentId {
        ComponentId::Host
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, _model: &UiModel) {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        if let Some(section) = self.tab_at(mouse.column, mouse.row) {
            self.section = section;
            self.selected = 0;
        }
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        match key.code {
            // Tab belongs to the whole interface; the sections answer to the
            // brackets, so the two are never ambiguous.
            KeyCode::Char(']') => {
                self.section = self.section.next();
                self.selected = 0;
            }
            KeyCode::Char('[') => {
                self.section = self.section.previous();
                self.selected = 0;
            }
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => self.select_next(model, true),
            keys::NAV_UP | keys::NAV_UP_ARROW => self.select_next(model, false),
            KeyCode::Char('r') => {
                self.pending_action = Some(ComponentAction::View(match self.section {
                    HostSection::Skills => ViewAction::RefreshSkills,
                    HostSection::Mcp => ViewAction::RefreshMcp,
                    HostSection::Overview | HostSection::Approvals => ViewAction::RefreshHost,
                }));
            }
            keys::SUBMIT if self.section == HostSection::Mcp => {
                if let Some(server) = self.selected_mcp(model) {
                    self.pending_action = Some(ComponentAction::View(ViewAction::SetMcp {
                        name: server.name.clone(),
                        state: if server.active {
                            McpState::Inactive
                        } else {
                            McpState::Active
                        },
                    }));
                }
            }
            KeyCode::Char('a') if self.section == HostSection::Approvals => {
                self.set_tool_rule(model, ToolRuleAction::Allow);
            }
            KeyCode::Char('d') if self.section == HostSection::Approvals => {
                self.set_tool_rule(model, ToolRuleAction::Deny);
            }
            KeyCode::Char('f') if self.section == HostSection::Approvals => {
                self.set_tool_rule(model, ToolRuleAction::Forget);
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        self.last_area = area;
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);
        self.tabs.clear();
        let mut column = area.x;
        let mut spans = Vec::new();
        for section in HostSection::ALL {
            let label = format!(" {} ", section.title());
            let width = u16::try_from(label.chars().count()).unwrap_or(0);
            self.tabs.push((column, column + width, section));
            column += width;
            spans.push(ratatui::text::Span::styled(
                label,
                if section == self.section {
                    palette::active_tab_style(model.preferences.theme)
                } else {
                    palette::inactive_tab_style(model.preferences.theme)
                },
            ));
        }
        frame.render_widget(
            ratatui::widgets::Paragraph::new(Line::from(spans)).style(
                Style::default()
                    .bg(palette::surface(model.preferences.theme))
                    .fg(palette::text(model.preferences.theme)),
            ),
            rows[0],
        );
        frame.render_widget(
            Block::default()
                .borders(Borders::TOP)
                .border_style(palette::border_style(model.preferences.theme))
                .style(palette::panel_style(model.preferences.theme))
                .padding(spacing::panel()),
            Rect {
                height: 1,
                ..rows[0]
            },
        );
        self.render_section(frame, rows[1], model);
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}

impl HostView {
    fn set_tool_rule(&mut self, model: &UiModel, action: ToolRuleAction) {
        if let Some(rule) = self.selected_tool(model) {
            self.pending_action = Some(ComponentAction::View(ViewAction::SetToolRule {
                name: rule.name.clone(),
                action,
            }));
        }
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
