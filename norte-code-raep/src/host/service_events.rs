//! Mapping agent events into RAEP frames.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;
use norte_code_wire::{Delta, ServerEventKind};
use raep_types::TurnId;

impl NorteCodeRaepHost {
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn collect_service_events(
        &self,
        session_id: &str,
        mut stream: norte_code_agent::EventStream,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        use futures_util::StreamExt;
        let mut output = Vec::new();
        while let Some(event) = stream.next().await {
            let event_session = event
                .session
                .as_ref()
                .map_or_else(|| session_id.to_owned(), |key| key.as_str().to_owned());
            if let ServerEventKind::TurnStarted { turn } = &event.kind {
                let mut state = self.state.lock().await;
                let client_turn_id = match state.turns.take_pending(&event_session) {
                    Some(id) => id,
                    None => TurnId::new(turn.to_string())
                        .map_err(|error| HostError::InvalidFrame(error.to_string()))?,
                };
                state.turns.set_alias(
                    &event_session,
                    *turn,
                    client_turn_id.clone(),
                    self.config.idempotency_capacity,
                );
                state.turns.activate(&event_session, client_turn_id);
                if let Some(requested) = state.turns.take_pending_output(&event_session) {
                    state
                        .turns
                        .set_turn_output(&event_session, *turn, requested);
                }
                drop(state);
            }
            if let ServerEventKind::Delta {
                delta: Delta::Media { .. },
                ..
            } = &event.kind
            {
                continue;
            }
            if let ServerEventKind::MediaPartCompleted { turn, index, media } = &event.kind {
                let requested = self
                    .state
                    .lock()
                    .await
                    .turns
                    .turn_output(&event_session, *turn)
                    .cloned()
                    .unwrap_or_default();
                if requested.contains(&raep_modality_for_media(media.kind)) {
                    output.extend(
                        self.emit_media_output(&event_session, *turn, *index, media)
                            .await?,
                    );
                }
                continue;
            }
            let tool_state_change = match &event.kind {
                ServerEventKind::ToolStarted { turn, .. } => {
                    Some((*turn, raep_types::TurnState::ExecutingTool))
                }
                _ => None,
            };
            let finished_tool_turn = match &event.kind {
                ServerEventKind::ToolFinished { turn, .. } => Some(*turn),
                _ => None,
            };
            if let Some((turn, state)) = tool_state_change {
                let turn_id = self.state.lock().await.turns.alias(&event_session, turn);
                if let Some(turn_id) = turn_id {
                    output.push(
                        self.emit_with_turn_id(
                            &event_session,
                            StandardFrameType::TurnStateChanged,
                            payload(raep_types::TurnStateChanged {
                                state,
                                stop_reason: None,
                                usage: None,
                                error: None,
                            }),
                            turn_id,
                        )
                        .await?,
                    );
                }
            }
            let (finished_turn, stopped_turn_id) = match &event.kind {
                ServerEventKind::TurnFinished { turn, .. } => (Some(*turn), None),
                ServerEventKind::TurnStopped { .. } => {
                    let state = self.state.lock().await;
                    let client_turn_id = state.turns.active(&event_session);
                    let service_turn_id = client_turn_id
                        .as_ref()
                        .and_then(|client| state.turns.service_turn(&event_session, client));
                    (service_turn_id, client_turn_id)
                }
                _ => (None, None),
            };
            output.push(
                self.emit_event_with_turn_id(session_id, event, stopped_turn_id.clone())
                    .await?,
            );
            if let Some(turn) = finished_tool_turn {
                let turn_id = {
                    let state = self.state.lock().await;
                    // A stopped turn has already been released: announcing
                    // that the provider is being called again would promise a
                    // request that is not coming.
                    state
                        .turns
                        .alias(&event_session, turn)
                        .filter(|id| state.turns.is_active(&event_session, id))
                };
                if let Some(turn_id) = turn_id {
                    output.push(
                        self.emit_with_turn_id(
                            &event_session,
                            StandardFrameType::TurnStateChanged,
                            payload(raep_types::TurnStateChanged {
                                state: raep_types::TurnState::CallingProvider,
                                stop_reason: None,
                                usage: None,
                                error: None,
                            }),
                            turn_id,
                        )
                        .await?,
                    );
                }
            }
            if finished_turn.is_some() || stopped_turn_id.is_some() {
                {
                    let mut state = self.state.lock().await;
                    if let Some(turn) = finished_turn {
                        state.turns.remove_turn_output(&event_session, turn);
                    }
                    state.turns.clear_running(&event_session);
                }
                if let Some(next) = self.reserve_next_queued_turn(&event_session).await {
                    output.extend(self.start_queued_turn(&event_session, next).await?);
                }
            }
        }
        self.state
            .lock()
            .await
            .turns
            .clear_pending_output(session_id);
        Ok(output)
    }
}
