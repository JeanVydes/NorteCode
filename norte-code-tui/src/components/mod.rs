mod action;
mod component;
mod lifecycle;
pub(crate) mod palette;
mod panel;
pub(crate) mod rect;
mod views;

pub(crate) use action::{
    CatalogOperation, ComponentAction, HeaderModal, McpState, PluginOperation, Preference,
    ToolRuleAction, ViewAction,
};
pub(crate) use component::{Component, ComponentEvent, ComponentId, LifecycleComponent};
pub(crate) use lifecycle::{
    ComponentLifecycle, LifecycleEvent, LifecyclePhase, LifecycleTransitionError,
};
pub(crate) use views::plan_review::PlanReviewView;

pub(crate) fn create_views() -> Vec<Box<dyn LifecycleComponent>> {
    vec![
        Box::new(views::session::SessionView::default()),
        Box::new(views::sessions::SessionsView::default()),
        Box::new(views::jobs::JobsView::default()),
        Box::new(views::plans::PlansView::default()),
        Box::new(views::telemetry::TelemetryView::default()),
        Box::new(views::plugins::PluginsView::default()),
        Box::new(views::models::ModelsView::default()),
        Box::new(views::host::HostView::default()),
        Box::new(views::costs::CostsView::default()),
        Box::new(views::catalog::CatalogView::default()),
        Box::new(views::settings::SettingsView::default()),
    ]
}
