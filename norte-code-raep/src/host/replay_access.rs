//! Async access to the filesystem-backed replay journal.

use std::sync::Arc;

use raep_types::Envelope;
use serde_json::Value;

use super::{HostError, NorteCodeRaepHost, ReplayWindow};

impl NorteCodeRaepHost {
    pub(crate) async fn replay_last_seq(&self, session_id: &str) -> Result<Option<u64>, HostError> {
        let replay = Arc::clone(&self.replay);
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || replay.last_seq(&session_id))
            .await
            .map_err(|error| HostError::Replay(format!("replay sequence lookup: {error}")))
    }

    pub(crate) async fn replay_after(
        &self,
        session_id: &str,
        sequence: u64,
    ) -> Result<ReplayWindow, HostError> {
        let replay = Arc::clone(&self.replay);
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || replay.after(&session_id, sequence))
            .await
            .map_err(|error| HostError::Replay(format!("replay window read: {error}")))
    }

    pub(crate) async fn replay_total(&self) -> Result<(u64, u64), HostError> {
        let replay = Arc::clone(&self.replay);
        tokio::task::spawn_blocking(move || replay.total())
            .await
            .map_err(|error| HostError::Replay(format!("replay totals read: {error}")))
    }

    pub(crate) async fn remove_replay(&self, session_id: &str) -> Result<(), HostError> {
        let replay = Arc::clone(&self.replay);
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || replay.remove(&session_id))
            .await
            .map_err(|error| HostError::Replay(format!("replay removal: {error}")))?;
        Ok(())
    }

    pub(crate) async fn append_replay(&self, frame: Envelope<Value>) -> Result<(), HostError> {
        let replay = Arc::clone(&self.replay);
        tokio::task::spawn_blocking(move || replay.append(&frame))
            .await
            .map_err(|error| HostError::Replay(format!("replay append task: {error}")))??;
        Ok(())
    }
}
