//! The state one turn carries while it runs.

use tokio::sync::mpsc;

use crate::runtime::Harness;
use norte::id::SessionId;
use norte_code_wire::{ServerEvent, SessionKey};

use super::Stats;
use super::compaction::CompactionGate;

/// Aborts a spawned task when dropped.
pub(super) struct AbortOnDrop(pub(super) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The state one turn carries through its event loop.
///
/// `handle`, `settle` and `finish_turn` all took the same eight parameters;
/// grouping them here makes the next field a one-line change instead of a
/// signature update in three places.
pub(super) struct TurnContext<'a> {
    pub(super) engine: &'a Harness,
    pub(super) request_id: u64,
    pub(super) key: &'a SessionKey,
    pub(super) nid: SessionId,
    pub(super) stats: &'a mut Stats,
    pub(super) tx: &'a mpsc::Sender<ServerEvent>,
    pub(super) gate: Option<&'a CompactionGate>,
    pub(super) output_schema_valid: Option<bool>,
}
