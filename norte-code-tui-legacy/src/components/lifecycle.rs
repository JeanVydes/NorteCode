use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LifecyclePhase {
    #[default]
    Dormant,
    Mounting,
    Loading,
    Ready,
    Empty,
    Updating,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum LifecycleEvent {
    Mount,
    LoadStarted,
    Loaded,
    Empty,
    Refresh,
    Failed,
    Retry,
    Unmount,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("invalid component lifecycle transition: {phase:?} + {event:?}")]
pub(crate) struct LifecycleTransitionError {
    phase: LifecyclePhase,
    event: LifecycleEvent,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ComponentLifecycle {
    phase: LifecyclePhase,
}

impl ComponentLifecycle {
    #[must_use]
    pub(crate) const fn phase(self) -> LifecyclePhase {
        self.phase
    }

    pub(crate) fn transition(
        &mut self,
        event: LifecycleEvent,
    ) -> Result<LifecyclePhase, LifecycleTransitionError> {
        let current = self.phase;
        let next = match (current, event) {
            (LifecyclePhase::Dormant, LifecycleEvent::Mount) => LifecyclePhase::Mounting,
            (LifecyclePhase::Mounting, LifecycleEvent::LoadStarted)
            | (LifecyclePhase::Failed, LifecycleEvent::Retry) => LifecyclePhase::Loading,
            (LifecyclePhase::Loading | LifecyclePhase::Updating, LifecycleEvent::Loaded) => {
                LifecyclePhase::Ready
            }
            (LifecyclePhase::Loading | LifecyclePhase::Updating, LifecycleEvent::Empty) => {
                LifecyclePhase::Empty
            }
            (
                LifecyclePhase::Mounting
                | LifecyclePhase::Loading
                | LifecyclePhase::Ready
                | LifecyclePhase::Empty
                | LifecyclePhase::Updating,
                LifecycleEvent::Failed,
            ) => LifecyclePhase::Failed,
            (LifecyclePhase::Ready | LifecyclePhase::Empty, LifecycleEvent::Refresh) => {
                LifecyclePhase::Updating
            }
            (
                LifecyclePhase::Mounting
                | LifecyclePhase::Loading
                | LifecyclePhase::Ready
                | LifecyclePhase::Empty
                | LifecyclePhase::Updating
                | LifecyclePhase::Failed,
                LifecycleEvent::Unmount,
            ) => LifecyclePhase::Dormant,
            _ => {
                return Err(LifecycleTransitionError {
                    phase: current,
                    event,
                });
            }
        };
        self.phase = next;
        Ok(next)
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
