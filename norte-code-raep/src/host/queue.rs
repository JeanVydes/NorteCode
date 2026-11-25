//! Queued turn reservation and execution.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;
use norte_code_wire::ClientRequest;

impl NorteCodeRaepHost {
    pub(crate) async fn reserve_next_queued_turn(&self, session_id: &str) -> Option<QueuedTurn> {
        let mut state = self.state.lock().await;
        let next = state.turns.dequeue(session_id)?;
        state.turns.reserve(session_id, next.turn_id.clone());
        state.turns.pending_push(session_id, next.turn_id.clone());
        if next
            .output
            .iter()
            .any(|modality| !matches!(modality, raep_types::Modality::Text))
        {
            state
                .turns
                .set_pending_output(session_id, next.output.clone());
        }
        Some(next)
    }
    pub(crate) async fn start_queued_turn(
        &self,
        session_id: &str,
        turn: QueuedTurn,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        // `replaces_turn_id` is a one-step rewind-and-re-run: the rewind has
        // already happened when the input was accepted (see
        // `NorteCodeRaepHost::user_input`), so by the time a queued turn runs
        // there is nothing left to rewind. The queued turn simply carries the
        // response format the client asked for.
        let stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::SendMessage {
                    session: Some(SessionKey::new(session_id)),
                    text: turn.text,
                    attachments: turn.attachments,
                    budget: turn.budget,
                    reasoning: turn.reasoning,
                    output: Vec::new(),
                    response_format: turn.response_format,
                    metadata: turn.metadata,
                    replay: false,
                },
            })
            .await;
        match stream {
            Ok(stream) => Box::pin(self.collect_service_events(session_id, stream)).await,
            Err(error) => {
                let message = error.to_string();
                {
                    let mut state = self.state.lock().await;
                    state.turns.release(session_id, &turn.turn_id);
                }
                let terminal = self
                    .emit_with_turn_id(
                        session_id,
                        StandardFrameType::TurnStateChanged,
                        payload(raep_types::TurnStateChanged {
                            state: raep_types::TurnState::Failed,
                            stop_reason: Some(raep_types::StopReason::Error),
                            usage: None,
                            error: Some(raep_types::ErrorPayload {
                                code: "UNAVAILABLE".to_owned(),
                                message,
                                retryable: Some(true),
                                details: None,
                            }),
                        }),
                        turn.turn_id,
                    )
                    .await?;
                let mut output = vec![terminal];
                if let Some(next) = self.reserve_next_queued_turn(session_id).await {
                    output.extend(Box::pin(self.start_queued_turn(session_id, next)).await?);
                }
                Ok(output)
            }
        }
    }
}
