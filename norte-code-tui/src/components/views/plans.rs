use crossterm::event::KeyEvent;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

use crate::{
    components::{Component, ComponentId, ComponentLifecycle, panel},
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

#[derive(Default)]
pub(in crate::components) struct PlansView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    detail_mode: PlanDetailMode,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PlanDetailMode {
    #[default]
    Summary,
    Full,
}

impl Component for PlansView {
    fn id(&self) -> ComponentId {
        ComponentId::Plans
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        match key.code {
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                panel::move_selection(&mut self.selected, model.plans.len(), true);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                panel::move_selection(&mut self.selected, model.plans.len(), false);
            }
            keys::SUBMIT => {
                self.detail_mode = match self.detail_mode {
                    PlanDetailMode::Summary => PlanDetailMode::Full,
                    PlanDetailMode::Full => PlanDetailMode::Summary,
                };
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let entries = model
            .plans
            .iter()
            .map(TextBuilder::plan_row)
            .collect::<Vec<_>>();
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(area);
        panel::render_list(
            frame,
            columns[0],
            entries,
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Plans,
                phase: self.lifecycle.phase(),
                selected: Some(self.selected),
                focused: true,
                empty_message: TextBuilder::empty_message(ComponentId::Plans),
            },
        );
        let body = model.plans.get(self.selected).map_or_else(
            || TextBuilder::plan_selection_hint().to_owned(),
            |plan| TextBuilder::plan_detail(plan, self.detail_mode == PlanDetailMode::Full),
        );
        panel::render_text(
            frame,
            columns[1],
            TextBuilder::details_title(),
            body,
            model.preferences.theme,
        );
    }
}
