//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use crate::payloads::{canonical_event_payload, payload, standard_event_payload};
use norte_code_wire::ServerEvent;
use raep_types::{Envelope, FrameType, StandardFrameType, TurnId};
use serde_json::Value;

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::capabilities::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::payloads::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::stores::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;

impl NorteCodeRaepHost {
    pub(crate) async fn emit_event(
        &self,
        session_id: &str,
        event: ServerEvent,
    ) -> Result<Envelope<Value>, HostError> {
        self.emit_event_with_turn_id(session_id, event, None).await
    }
    pub(crate) async fn emit_event_with_turn_id(
        &self,
        session_id: &str,
        event: ServerEvent,
        turn_id_override: Option<TurnId>,
    ) -> Result<Envelope<Value>, HostError> {
        let kind = frame_type_for_event(&event.kind);
        let session_id = event
            .session
            .as_ref()
            .map_or_else(|| session_id.to_owned(), |key| key.as_str().to_owned());
        let job_id = job_id_for_event(&event.kind);
        let event_payload = standard_event_payload(&event.kind)
            .unwrap_or_else(|| canonical_event_payload(&event.kind));
        self.emit_with_extension(
            &session_id,
            kind,
            event_payload,
            Some(payload(NorteCodeEventExtension { norte_code: &event })),
            turn_id_for_event(&event.kind),
            job_id.as_deref(),
            turn_id_override,
        )
        .await
    }
    pub(crate) async fn emit(
        &self,
        session_id: &str,
        kind: StandardFrameType,
        payload: Value,
    ) -> Result<Envelope<Value>, HostError> {
        self.emit_with_extension(session_id, standard(kind), payload, None, None, None, None)
            .await
    }
    pub(crate) async fn emit_with_turn_id(
        &self,
        session_id: &str,
        kind: StandardFrameType,
        payload: Value,
        turn_id: TurnId,
    ) -> Result<Envelope<Value>, HostError> {
        let mut frame = Envelope::new(standard(kind), payload, now_millis());
        frame.session_id = session_id.to_owned();
        frame.turn_id = Some(turn_id);
        self.emit_frame(session_id, frame).await
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn emit_with_extension(
        &self,
        session_id: &str,
        kind: FrameType,
        payload: Value,
        ext: Option<Value>,
        turn_id: Option<u64>,
        job_id: Option<&str>,
        turn_id_override: Option<TurnId>,
    ) -> Result<Envelope<Value>, HostError> {
        let mut frame = Envelope::new(kind, payload, now_millis());
        frame.session_id = session_id.to_owned();
        frame.ext = ext;
        if let Some(job) = job_id {
            frame.job_id = Some(
                raep_types::JobId::new(job.to_owned())
                    .map_err(|error| HostError::InvalidFrame(error.to_string()))?,
            );
        }
        if let Some(turn) = turn_id {
            let numeric_turn_id = TurnId::new(turn.to_string())
                .map_err(|error| HostError::InvalidFrame(error.to_string()))?;
            let state = self.state.lock().await;
            frame.turn_id = state
                .turn_aliases
                .get(&(session_id.to_owned(), turn))
                .cloned()
                .or(Some(numeric_turn_id));
        }
        if let Some(turn_id) = turn_id_override {
            frame.turn_id = Some(turn_id);
        }
        self.emit_frame(session_id, frame).await
    }
    pub(crate) async fn emit_frame(
        &self,
        session_id: &str,
        mut frame: Envelope<Value>,
    ) -> Result<Envelope<Value>, HostError> {
        {
            let mut state = self.state.lock().await;
            // The log outlives the process: a session's sequence continues
            // from what is already on disk instead of restarting at one.
            let current = state
                .sequences
                .get(session_id)
                .copied()
                .unwrap_or_else(|| self.replay.last_seq(session_id).unwrap_or(0));
            let sequence = current.saturating_add(1);
            frame.seq = Some(sequence);
            state.sequences.insert(session_id.to_owned(), sequence);
            if let Some(turn_id) = &frame.turn_id {
                let key = (session_id.to_owned(), turn_id.clone());
                if state.turn_request_fingerprints.contains_key(&key) {
                    state.turn_request_frames.insert(key, frame.clone());
                }
            }
        }
        // The file write happens outside the state lock: a slow disk must not
        // block every other session's frames.
        self.replay.append(&frame)?;
        let _ = self.events.send(frame.clone());
        Ok(frame)
    }
    pub(crate) async fn require_attached(&self, session_id: &str) -> Result<(), HostError> {
        if self.state.lock().await.attached.contains(session_id) {
            Ok(())
        } else {
            Err(HostError::InvalidFrame(
                "session is not attached".to_owned(),
            ))
        }
    }
    pub(crate) async fn remove_turn_request(&self, session_id: &str, turn_id: &TurnId) {
        let mut state = self.state.lock().await;
        let key = (session_id.to_owned(), turn_id.clone());
        state.turn_request_fingerprints.remove(&key);
        state.turn_request_frames.remove(&key);
        state
            .turn_request_order
            .retain(|candidate| candidate != &key);
        if state.turn_reservations.get(session_id) == Some(turn_id) {
            state.turn_reservations.remove(session_id);
            state.pending_output.remove(session_id);
        }
        if let Some(pending) = state.pending_turn_ids.get_mut(session_id) {
            pending.retain(|pending_turn| pending_turn != turn_id);
            if pending.is_empty() {
                state.pending_turn_ids.remove(session_id);
            }
        }
        if let Some(queue) = state.queued_turns.get_mut(session_id) {
            queue.retain(|queued| &queued.turn_id != turn_id);
            if queue.is_empty() {
                state.queued_turns.remove(session_id);
            }
        }
    }
}
