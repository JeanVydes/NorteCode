use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Tabs,
};

use crate::{
    components::palette,
    components::{
        Component, ComponentAction, ComponentId, ComponentLifecycle, LifecyclePhase, McpState,
        ToolRuleAction, ViewAction, panel,
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
    fn next(self) -> Self {
        match self {
            Self::Overview => Self::Skills,
            Self::Skills => Self::Mcp,
            Self::Mcp => Self::Approvals,
            Self::Approvals => Self::Overview,
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Overview => "Host",
            Self::Skills => "Skills",
            Self::Mcp => "MCP",
            Self::Approvals => "Tool approvals",
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
}

#[derive(Default)]
pub(in crate::components) struct HostView {
    lifecycle: ComponentLifecycle,
    section: HostSection,
    selected: usize,
    pending_action: Option<ComponentAction>,
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

    fn render_section(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        match self.section {
            HostSection::Overview => panel::render_text(
                frame,
                area,
                TextBuilder::host_title(),
                TextBuilder::host_overview(model),
                model.preferences.theme,
            ),
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
            entries,
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
            entries,
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
        let entries = model
            .tool_rules
            .iter()
            .map(TextBuilder::tool_rule_row)
            .collect();
        panel::render_list(
            frame,
            area,
            entries,
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

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        match key.code {
            keys::FOCUS_NEXT => {
                self.section = self.section.next();
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
            KeyCode::Enter if self.section == HostSection::Mcp => {
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
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(area);
        let tabs = [
            HostSection::Overview,
            HostSection::Skills,
            HostSection::Mcp,
            HostSection::Approvals,
        ]
        .into_iter()
        .map(HostSection::title)
        .collect::<Vec<_>>();
        frame.render_widget(
            Tabs::new(tabs)
                .style(palette::panel_style(model.preferences.theme))
                .select(self.section.index()),
            rows[0],
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
