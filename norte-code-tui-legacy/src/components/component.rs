use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{Frame, layout::Rect};

use super::{
    ComponentAction, ComponentLifecycle, LifecycleEvent, LifecyclePhase, LifecycleTransitionError,
};
use crate::ui_model::UiModel;

/// The tabs of the interface, in the order they are shown and numbered.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ComponentId {
    Session,
    Sessions,
    Models,
    Plans,
    Jobs,
    Telemetry,
    Host,
    Plugins,
    Costs,
    /// The review overlay. It is not a tab; no shortcut selects it.
    PlanReview,
}

impl ComponentId {
    /// Every tab, in display order.
    #[must_use]
    pub(crate) const fn tabs() -> [Self; 9] {
        [
            Self::Session,
            Self::Sessions,
            Self::Models,
            Self::Plans,
            Self::Jobs,
            Self::Telemetry,
            Self::Host,
            Self::Plugins,
            Self::Costs,
        ]
    }

    /// The digit that selects the tab, when it is one.
    #[must_use]
    pub(crate) const fn shortcut(self) -> Option<char> {
        match self {
            Self::Session => Some('1'),
            Self::Sessions => Some('2'),
            Self::Models => Some('3'),
            Self::Plans => Some('4'),
            Self::Jobs => Some('5'),
            Self::Telemetry => Some('6'),
            Self::Host => Some('7'),
            Self::Plugins => Some('8'),
            Self::Costs => Some('9'),
            Self::PlanReview => None,
        }
    }

    /// The tab a digit selects.
    #[must_use]
    pub(crate) const fn from_shortcut(shortcut: char) -> Option<Self> {
        match shortcut {
            '1' => Some(Self::Session),
            '2' => Some(Self::Sessions),
            '3' => Some(Self::Models),
            '4' => Some(Self::Plans),
            '5' => Some(Self::Jobs),
            '6' => Some(Self::Telemetry),
            '7' => Some(Self::Host),
            '8' => Some(Self::Plugins),
            '9' => Some(Self::Costs),
            _ => None,
        }
    }

    /// The index of the tab among the tabs.
    #[must_use]
    pub(crate) fn tab_index(self) -> Option<usize> {
        Self::tabs().iter().position(|tab| *tab == self)
    }

    /// The tab next to this one, wrapping at both ends.
    #[must_use]
    pub(crate) fn next_tab(self) -> Self {
        let tabs = Self::tabs();
        let index = self.tab_index().unwrap_or(0);
        tabs[(index + 1) % tabs.len()]
    }

    /// The tab before this one, wrapping at both ends.
    #[must_use]
    pub(crate) fn previous_tab(self) -> Self {
        let tabs = Self::tabs();
        let index = self.tab_index().unwrap_or(0);
        tabs[(index + tabs.len() - 1) % tabs.len()]
    }
}

/// Something that arrived for a component.
pub(crate) enum ComponentEvent {
    /// A file was uploaded and is ready to attach to the next message.
    Uploaded(crate::chips::Chip),
    /// Another session was opened and the component starts clean.
    SessionOpened,
    /// The menu asked to pin or unpin the turn under the viewport.
    PinCurrentTurn,
}

pub(crate) trait Component {
    fn id(&self) -> ComponentId;
    fn lifecycle(&self) -> &ComponentLifecycle;
    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle;
    /// Whether the component owns the keys while it is open.
    ///
    /// A prompt, a filter or a modal does; a plain list does not, so the
    /// global shortcuts keep working.
    fn captures_global_keys(&self) -> bool {
        false
    }
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
