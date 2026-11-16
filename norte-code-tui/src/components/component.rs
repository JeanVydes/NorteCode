use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{Frame, layout::Rect};

use super::{
    ComponentAction, ComponentLifecycle, LifecycleEvent, LifecyclePhase, LifecycleTransitionError,
};
use crate::ui_model::UiModel;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ComponentId {
    Session,
    Plans,
    Telemetry,
    Plugins,
    Models,
    Host,
    Sessions,
    Jobs,
    PlanReview,
    Costs,
    Settings,
}

impl ComponentId {
    #[must_use]
    pub(crate) const fn number(self) -> char {
        match self {
            Self::Session => crate::keys::SESSION_VIEW,
            Self::Plans => crate::keys::PLANS_VIEW,
            Self::Telemetry => crate::keys::TELEMETRY_VIEW,
            Self::Plugins => crate::keys::PLUGINS_VIEW,
            Self::Models => crate::keys::MODELS_VIEW,
            Self::Host => crate::keys::HOST_VIEW,
            Self::Sessions => crate::keys::SESSIONS_VIEW_CHAR,
            Self::Jobs => crate::keys::JOBS_VIEW,
            Self::PlanReview => '0',
            Self::Costs => crate::keys::COSTS_VIEW,
            Self::Settings => crate::keys::SETTINGS_VIEW,
        }
    }

    #[must_use]
    pub(crate) const fn all() -> [Self; 10] {
        [
            Self::Session,
            Self::Plans,
            Self::Telemetry,
            Self::Plugins,
            Self::Models,
            Self::Host,
            Self::Jobs,
            Self::Costs,
            Self::Settings,
            Self::Sessions,
        ]
    }

    #[must_use]
    pub(crate) const fn from_shortcut(shortcut: char) -> Option<Self> {
        match shortcut {
            crate::keys::SESSION_VIEW => Some(Self::Session),
            crate::keys::PLANS_VIEW => Some(Self::Plans),
            crate::keys::TELEMETRY_VIEW => Some(Self::Telemetry),
            crate::keys::PLUGINS_VIEW => Some(Self::Plugins),
            crate::keys::MODELS_VIEW | crate::keys::MODELS_VIEW_CHAR => Some(Self::Models),
            crate::keys::HOST_VIEW => Some(Self::Host),
            crate::keys::JOBS_VIEW => Some(Self::Jobs),
            crate::keys::SESSIONS_VIEW_CHAR => Some(Self::Sessions),
            crate::keys::COSTS_VIEW => Some(Self::Costs),
            crate::keys::SETTINGS_VIEW => Some(Self::Settings),
            _ => None,
        }
    }
}

/// Something that arrived for a component.
pub(crate) enum ComponentEvent {
    /// A file was uploaded and is ready to attach to the next message.
    Uploaded(crate::chips::Chip),
    /// Another session was opened and the component starts clean.
    SessionOpened,
}

pub(crate) trait Component {
    fn id(&self) -> ComponentId;
    fn lifecycle(&self) -> &ComponentLifecycle;
    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle;
    /// Scrolls the view when it is a conversation transcript.
    fn scroll_transcript(&mut self, _lines: isize) {}
    fn captures_global_keys(&self) -> bool {
        false
    }
    /// Handles a mouse event: a terminal sends one when the reader clicks,
    /// drags or scrolls.
    fn handle_mouse(&mut self, mouse: MouseEvent, model: &UiModel) {
        let _ = (mouse, model);
    }
    fn handle_key(&mut self, key: KeyEvent, model: &UiModel);
    /// Handles the keys that arrived together.
    ///
    /// A terminal delivers a paste as a burst of keys, and a view that cares
    /// about the burst reads it here instead of key by key.
    fn handle_keys(&mut self, keys: &[KeyEvent], model: &UiModel) {
        for key in keys {
            self.handle_key(*key, model);
        }
    }
    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel);
    fn take_action(&mut self) -> Option<ComponentAction> {
        None
    }
    /// Takes something that arrived from the host.
    fn accept(&mut self, _event: ComponentEvent) {}
}

pub(crate) trait LifecycleComponent: Component {
    fn transition(
        &mut self,
        event: LifecycleEvent,
    ) -> Result<LifecyclePhase, LifecycleTransitionError> {
        self.lifecycle_mut().transition(event)
    }
}

impl<T: Component + ?Sized> LifecycleComponent for T {}

#[cfg(test)]
#[path = "component_tests.rs"]
mod tests;
