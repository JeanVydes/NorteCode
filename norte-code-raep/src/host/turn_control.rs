//! Turn cancellation, detachment, closing, and acknowledgement.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;
use norte_code_wire::ClientRequest;
use raep_types::TurnId;

impl NorteCodeRaepHost {
    pub(crate) async fn cancel(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let requested_turn = frame.turn_id.as_ref().ok_or_else(|| {
            HostError::InvalidFrame("CANCEL_TURN requires envelope.turn_id".to_owned())
        })?;
        let (active_turn, cancelled_queued_turn) = {
            let mut state = self.state.lock().await;
            let active_turn = state.turns.active(&frame.session_id);
            // A turn that is running but has not produced its first event yet
            // is only reserved: the host reserved it when the input was
            // accepted and learned nothing since. Treating that as "turn not
            // active" refused the only cancel the client could send, and left
            // the session with a reservation nothing could clear.
            let reserved = state.turns.reserved(&frame.session_id);
            if active_turn.as_ref() == Some(requested_turn)
                || (active_turn.is_none() && reserved.as_ref() == Some(requested_turn))
            {
                (Some(requested_turn.clone()), false)
            } else {
                if !state.turns.remove_queued(&frame.session_id, requested_turn) {
                    return Err(HostError::TurnNotActive);
                }
                state
                    .turns
                    .pending_retain(&frame.session_id, requested_turn);
                (active_turn, true)
            }
        };
        if cancelled_queued_turn {
            return self
                .emit_with_turn_id(
                    &frame.session_id,
                    StandardFrameType::TurnStateChanged,
                    payload(raep_types::TurnStateChanged {
                        state: raep_types::TurnState::Cancelled,
                        stop_reason: Some(raep_types::StopReason::Cancelled),
                        usage: None,
                        error: None,
                    }),
                    requested_turn.clone(),
                )
                .await
                .map(|frame| vec![frame]);
        }
        let active_turn = active_turn.ok_or(HostError::TurnNotActive)?;
        if &active_turn != requested_turn {
            return Err(HostError::TurnNotActive);
        }
        let request = ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::CancelTurn {
                session: SessionKey::new(frame.session_id.clone()),
            },
        };
        let events = self
            .service
            .submit(request)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        let collected = self
            .collect_service_events(&frame.session_id, events)
            .await?;
        // The service cancels the token, but an engine parked in a tool sends
        // nothing, so the turn's reservation would stay and every later input
        // would meet `SESSION_BUSY`. The cancel itself is the one place that
        // knows the turn is over from the client's point of view.
        let mut released = self.release_turn(&frame.session_id, requested_turn).await;
        if collected.is_empty() {
            released.push(
                self.emit_with_turn_id(
                    &frame.session_id,
                    StandardFrameType::TurnStateChanged,
                    payload(raep_types::TurnStateChanged {
                        state: raep_types::TurnState::Cancelled,
                        stop_reason: Some(raep_types::StopReason::Cancelled),
                        usage: None,
                        error: None,
                    }),
                    requested_turn.clone(),
                )
                .await?,
            );
        }
        let mut output = collected;
        output.append(&mut released);
        Ok(output)
    }
    /// Drops the host's record of a running turn, answering the frames that
    /// tell the client it ended.
    pub(crate) async fn release_turn(
        &self,
        session_id: &str,
        turn_id: &TurnId,
    ) -> Vec<Envelope<Value>> {
        {
            let mut state = self.state.lock().await;
            state.turns.release(session_id, turn_id);
        }
        self.remove_turn_request(session_id, turn_id).await;
        Vec::new()
    }
    pub(crate) async fn detach(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let mut state = self.state.lock().await;
        state.attached.remove(&frame.session_id);
        state.attachment_modes.remove(&frame.session_id);
        Ok(Vec::new())
    }
    pub(crate) async fn close(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let purge = frame
            .payload
            .get("purge")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let events = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::DeleteSession {
                    session: SessionKey::new(frame.session_id.clone()),
                },
            )
            .await?;
        let mut output = events;
        output.push(
            self.emit(
                &frame.session_id,
                StandardFrameType::SessionClosed,
                payload(SessionClosedPayload {
                    reason: if purge {
                        SessionClosedReason::Purged
                    } else {
                        SessionClosedReason::Client
                    },
                }),
            )
            .await?,
        );
        {
            let mut state = self.state.lock().await;
            state.attached.remove(&frame.session_id);
            state.attachment_modes.remove(&frame.session_id);
        }
        self.remove_replay(&frame.session_id).await?;
        Ok(output)
    }
    pub(crate) async fn ack(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let last_seq = frame
            .payload
            .get("last_seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| HostError::InvalidFrame("ACK.last_seq is required".to_owned()))?;
        let state = self.state.lock().await;
        if last_seq
            > state
                .sequences
                .get(&frame.session_id)
                .copied()
                .unwrap_or_default()
        {
            return Err(HostError::InvalidFrame(
                "ACK.last_seq exceeds the last emitted sequence".to_owned(),
            ));
        }
        Ok(Vec::new())
    }
}
