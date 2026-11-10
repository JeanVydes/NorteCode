//! The helpers every operation group shares.

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use norte_code_wire::{CanonicalEvent, ClientRequestKind};
use raep_types::{Envelope, FrameType};

use super::{ApiError, CommandResponse, RestClient};

impl RestClient {
    pub(super) async fn command_ok(
        &self,
        session: Option<&str>,
        kind: ClientRequestKind,
    ) -> Result<(), ApiError> {
        self.command(session, kind).await.map(|_| ())
    }

    pub(super) fn frame<P: Serialize>(
        session: &str,
        kind: FrameType,
        payload: &P,
    ) -> Result<Envelope<Value>, ApiError> {
        let payload = serde_json::to_value(payload)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))?;
        let mut frame = Envelope::new(kind, payload, super::now_millis()?);
        session.clone_into(&mut frame.session_id);
        Ok(frame)
    }
}

/// A frame payload that carries nothing.
#[derive(Serialize)]
pub(super) struct EmptyPayload {}

/// Turns an accepted command into `Ok`, or reports why it was refused.
pub(super) fn accepted(response: CommandResponse, fallback: &str) -> Result<(), ApiError> {
    if response.accepted {
        Ok(())
    } else {
        let message = response
            .error
            .map_or_else(|| fallback.to_owned(), |error| error.message);
        Err(ApiError::InvalidResponse(message))
    }
}

/// The payload of the one event matching `kind`.
pub(super) fn find_event(
    response: &CommandResponse,
    kind: CanonicalEvent,
) -> Result<Value, ApiError> {
    response
        .events
        .iter()
        .find(|frame| kind.matches(&frame.payload))
        .map(|frame| frame.payload.clone())
        .ok_or_else(|| ApiError::InvalidResponse("The expected event was not returned.".to_owned()))
}

/// Decodes a payload into `T`.
pub(super) fn decode_payload<T: DeserializeOwned>(payload: &Value) -> Result<T, ApiError> {
    serde_json::from_value(payload.clone())
        .map_err(|error| ApiError::InvalidResponse(error.to_string()))
}
