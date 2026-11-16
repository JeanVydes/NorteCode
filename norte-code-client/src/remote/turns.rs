//! Turning a session's turns: cancelling, rewinding and pinning.
//!
//! These are the standard RAEP frames the spec owns, so they go out as
//! standard frames and ride the session's attachment, which the client keeps
//! and takes again when the host says the session lost it.

use raep_types::{Envelope, FrameType, StandardFrameType, TurnId};
use serde::Deserialize;
use serde_json::Value;

use super::RemoteClient;
use crate::error::ClientError;

/// A `SESSION_REWIND` body.
#[derive(serde::Serialize)]
struct RewindRequest<'a> {
    to_turn_id: &'a str,
    inclusive: bool,
    cancel_pending: bool,
    workspace: RewindWorkspace,
}

/// What a rewind does to the workspace.
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum RewindWorkspace {
    Keep,
}

/// A `CONTEXT_PIN` body.
#[derive(serde::Serialize)]
struct PinRequest<'a> {
    turn_ids: &'a [String],
    pinned: bool,
}

/// A `CONTEXT_UPDATED` body.
#[derive(Deserialize)]
struct ContextUpdated {
    #[serde(default)]
    pinned_turn_ids: Vec<String>,
}

impl RemoteClient {
    /// Cancels the running turn of a session.
    pub(crate) async fn cancel(&self, session: &str, turn: &TurnId) -> Result<(), ClientError> {
        let payload =
            serde_json::to_value(raep_types::CancelTurn { reason: None }).unwrap_or(Value::Null);
        self.post_standard_for_turn(
            session,
            FrameType::Standard(StandardFrameType::CancelTurn),
            payload,
            Some(turn),
        )
        .await?;
        Ok(())
    }

    /// Rewinds a session to a turn and reports the turns it removed.
    pub(crate) async fn rewind_to(
        &self,
        session: &str,
        to_turn_id: &str,
        inclusive: bool,
    ) -> Result<Vec<String>, ClientError> {
        let payload = serde_json::to_value(RewindRequest {
            to_turn_id,
            inclusive,
            cancel_pending: false,
            workspace: RewindWorkspace::Keep,
        })
        .map_err(|error| ClientError::new(format!("serialize rewind: {error}")))?;
        let events = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::SessionRewind),
                payload,
            )
            .await?;
        Ok(rewound_turn_ids(&events))
    }

    /// Pins or unpins turns and reports the turns that are pinned.
    pub(crate) async fn set_pins(
        &self,
        session: &str,
        turn_ids: &[String],
        pinned: bool,
    ) -> Result<Vec<String>, ClientError> {
        let payload = serde_json::to_value(PinRequest { turn_ids, pinned })
            .map_err(|error| ClientError::new(format!("serialize pin: {error}")))?;
        let events = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::ContextPin),
                payload,
            )
            .await?;
        Ok(pinned_turn_ids(&events))
    }
}

/// The turns a rewind removed, as the host reported them.
fn rewound_turn_ids(events: &[Envelope<Value>]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.kind == FrameType::Standard(StandardFrameType::HistoryRewound))
        .filter_map(|event| {
            serde_json::from_value::<crate::frame::HistoryRewoundPayload>(event.payload.clone())
                .ok()
        })
        .flat_map(|payload| payload.rewound_turn_ids)
        .collect()
}

/// The turns that are pinned, as the host reported them.
fn pinned_turn_ids(events: &[Envelope<Value>]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.kind == FrameType::Standard(StandardFrameType::ContextUpdated))
        .filter_map(|event| serde_json::from_value::<ContextUpdated>(event.payload.clone()).ok())
        .flat_map(|payload| payload.pinned_turn_ids)
        .collect()
}
