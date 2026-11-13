use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
};

use crate::{
    components::{
        Component, ComponentAction, ComponentId, ComponentLifecycle, Preference, ViewAction, panel,
    },
    keys,
    ui_model::{ThemeMode, UiModel},
};

const PREFERENCES: [Preference; 8] = [
    Preference::Theme,
    Preference::Thinking,
    Preference::Timestamps,
    Preference::ToolOutput,
    Preference::LiveRefresh,
    Preference::DeveloperMode,
    Preference::DiffStyle,
    Preference::BudgetAction,
];

#[derive(Default)]
pub(in crate::components) struct SettingsView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    pending_action: Option<ComponentAction>,
}

impl SettingsView {
    fn preference_row(preference: Preference, model: &UiModel) -> Line<'static> {
        let value = match preference {
            Preference::Theme => match model.preferences.theme {
                ThemeMode::Dark => "dark",
                ThemeMode::Light => "light",
            },
            Preference::Thinking => match model.preferences.thinking {
                crate::ui_model::ThinkingVisibility::Expanded => "expanded",
                crate::ui_model::ThinkingVisibility::Collapsed => "collapsed",
            },
            Preference::Timestamps => enabled_label(model.preferences.show_timestamps.is_on()),
            Preference::ToolOutput => enabled_label(model.preferences.show_tool_output.is_on()),
            Preference::LiveRefresh => {
                enabled_label(model.preferences.auto_refresh_sessions.is_on())
            }
            Preference::DeveloperMode => enabled_label(model.preferences.developer_mode.is_on()),
            Preference::DiffStyle => match model.cli.diff_style {
                crate::tools::DiffMode::Unified => "unified",
                crate::tools::DiffMode::Split => "split",
            },
            Preference::BudgetAction => match model.budget_action {
                norte_code_wire::BudgetAction::Warn => "warn, then continue",
                norte_code_wire::BudgetAction::Ask => "ask before one continuation",
                norte_code_wire::BudgetAction::Stop => "stop the turn",
            },
        };
        let name = match preference {
            Preference::Theme => "Theme",
            Preference::Thinking => "Thinking",
            Preference::Timestamps => "Timestamps",
            Preference::ToolOutput => "Tool output",
            Preference::LiveRefresh => "Live session refresh",
            Preference::DeveloperMode => "Developer diagnostics",
            Preference::DiffStyle => "Diff style",
            Preference::BudgetAction => "Budget action",
        };
        Line::from(format!("{name}: {value}"))
    }

    fn toggle(&mut self) {
        if let Some(preference) = PREFERENCES.get(self.selected).copied() {
            self.pending_action = Some(ComponentAction::View(ViewAction::TogglePreference(
                preference,
            )));
        }
    }
}

impl Component for SettingsView {
    fn id(&self) -> ComponentId {
        ComponentId::Settings
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn handle_key(&mut self, key: KeyEvent, _model: &UiModel) {
        match key.code {
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                panel::move_selection(&mut self.selected, PREFERENCES.len(), true);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                panel::move_selection(&mut self.selected, PREFERENCES.len(), false);
            }
            keys::SUBMIT | KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('r') => {
                self.pending_action = Some(ComponentAction::View(ViewAction::TestConnection));
            }
            _ => {}
        }
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        let entries = PREFERENCES
            .into_iter()
            .map(|preference| Self::preference_row(preference, model))
            .collect();
        panel::render_list(
            frame,
            columns[0],
            entries,
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Settings,
                phase: self.lifecycle.phase(),
                selected: Some(self.selected),
                focused: true,
                empty_message: "No settings",
            },
        );
        let authentication = if model.authentication_configured {
            "configured, value hidden"
        } else {
            "not configured"
        };
        let detail = format!(
            "Connection\nendpoint      {}\nauthentication {authentication}\n\nSettings use ~/.config/norte-code/client.json. The bearer token is never rendered or printed.\n\nBudget actions: warn continues, ask pauses for one approved continuation, stop ends the turn.\n\nr tests the current connection using host probes.",
            model.endpoint
        );
        panel::render_text(
            frame,
            columns[1],
            "Connection",
            detail,
            model.preferences.theme,
        );
    }
}

const fn enabled_label(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
