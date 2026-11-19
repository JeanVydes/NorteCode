mod action;
pub(crate) mod command_menu;
mod component;
mod lifecycle;
pub(crate) mod markdown;
mod overlay;
pub(crate) mod palette;
mod panel;
pub(crate) mod prompt;
pub(crate) mod rect;
pub(crate) mod spacing;
pub(crate) mod toast;
pub(crate) mod views;

pub(crate) use action::{
    CommandMenuAction, ComponentAction, McpState, PluginOperation, Preference, ToolRuleAction,
    ViewAction,
};
pub(crate) use command_menu::{CommandMenu, CommandOutcome};
pub(crate) use component::{Component, ComponentEvent, ComponentId, LifecycleComponent};
pub(crate) use lifecycle::{
    ComponentLifecycle, LifecycleEvent, LifecyclePhase, LifecycleTransitionError,
};
pub(crate) use overlay::{MenuEntry, Modal, ModalList, paint_scrim};
pub(crate) use views::plan_review::PlanReviewView;

pub(crate) fn create_views() -> Vec<Box<dyn LifecycleComponent>> {
    vec![
        Box::new(views::session::SessionView::default()),
        Box::new(views::sessions::SessionsView::default()),
        Box::new(views::models::ModelsView::default()),
        Box::new(views::plans::PlansView::default()),
        Box::new(views::jobs::JobsView::default()),
        Box::new(views::telemetry::TelemetryView::default()),
        Box::new(views::host::HostView::default()),
        Box::new(views::plugins::PluginsView::default()),
        Box::new(views::costs::CostsView::default()),
    ]
}
