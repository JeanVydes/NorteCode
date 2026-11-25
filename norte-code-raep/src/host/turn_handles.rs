//! The event-stream tasks that outlive the command that started them.

use std::collections::HashMap;
use std::sync::Mutex;

use raep_types::FrameId;
use tokio::task::JoinHandle;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TurnKey {
    pub(super) session: String,
    pub(super) request_id: FrameId,
}

impl TurnKey {
    pub(crate) fn new(session: impl Into<String>, request_id: FrameId) -> Self {
        Self {
            session: session.into(),
            request_id,
        }
    }
}

#[derive(Default)]
pub(super) struct TurnHandles {
    entries: Mutex<HashMap<TurnKey, JoinHandle<()>>>,
}

impl TurnHandles {
    pub(super) fn insert(&self, key: TurnKey, handle: JoinHandle<()>) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, handle);
    }

    pub(super) fn finish(&self, key: &TurnKey) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
    }

    pub(super) async fn abort_all(&self) {
        let entries = std::mem::take(
            &mut *self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let handles = entries.into_values().collect::<Vec<_>>();
        for handle in &handles {
            handle.abort();
        }
        for handle in handles {
            let _ = handle.await;
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
#[path = "turn_handles_tests.rs"]
mod tests;
