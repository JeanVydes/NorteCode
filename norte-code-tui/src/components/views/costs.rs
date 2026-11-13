use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect};

use crate::{
    components::{Component, ComponentAction, ComponentId, ComponentLifecycle, ViewAction, panel},
    text_builder::TextBuilder,
    ui_model::UiModel,
};

#[derive(Default)]
pub(in crate::components) struct CostsView {
    lifecycle: ComponentLifecycle,
    pending_action: Option<ComponentAction>,
}

impl Component for CostsView {
    fn id(&self) -> ComponentId {
        ComponentId::Costs
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn handle_key(&mut self, key: KeyEvent, _model: &UiModel) {
        if key.code == KeyCode::Char('r') {
            self.pending_action = Some(ComponentAction::View(ViewAction::RefreshCosts));
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let body = TextBuilder::cost_reports(
            model.costs.as_ref(),
            model.session_costs.as_ref(),
            model.active_session.as_ref(),
        );
        panel::render_text(
            frame,
            area,
            TextBuilder::panel_title_for(ComponentId::Costs),
            body,
            model.preferences.theme,
        );
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}
