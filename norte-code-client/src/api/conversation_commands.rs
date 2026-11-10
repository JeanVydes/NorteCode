/// The client name a Norte Code client advertises when it attaches.
pub const CLIENT_NAME: &str = "norte-code";

use raep_types::{FrameType, StandardFrameType, TurnId};

use super::shared::accepted;
use super::{ApiError, RestClient};

impl RestClient {
    /// Sends a user message on the session's open stream.
    ///
    /// Text-only prompts take the plain form; anything with attachments goes
    /// as content parts, with the text as the first part so the order the
    /// person wrote survives.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn send_prompt(
        &self,
        session: &str,
        text: Option<&str>,
        content: Option<Vec<norte_code_wire::UserInputPart>>,
        replaces_turn_id: Option<&str>,
        queue_mode: &str,
    ) -> Result<TurnId, ApiError> {
        let turn_id = TurnId::generate();
        let (prompt, content) = match (text, content) {
            (Some(text), Some(mut parts)) => {
                parts.insert(0, norte_code_wire::UserInputPart::text(text.to_owned()));
                (None, Some(parts))
            }
            other => other,
        };
        let request = super::types::UserInputRequest::build(
            prompt.map(str::to_owned),
            content,
            replaces_turn_id.map(str::to_owned),
            replaces_turn_id.is_none().then(|| queue_mode.to_owned()),
        )
        .ok_or_else(|| {
            ApiError::InvalidResponse("A message needs text or an attachment.".to_owned())
        })?;
        let mut frame = Self::frame(
            session,
            FrameType::Standard(StandardFrameType::UserInput),
            &request,
        )?;
        frame.turn_id = Some(turn_id.clone());
        let response = self.post_command(session, &frame).await?;
        accepted(response.clone(), "The message was rejected.")?;
        let accepted_turn =
            response
                .events
                .iter()
                .find_map(|event| match crate::decode_frame(event) {
                    Ok(decoded) => match decoded.event.kind {
                        norte_code_wire::ServerEventKind::InputAccepted { turn_id, .. } => {
                            TurnId::new(turn_id).ok()
                        }
                        _ => None,
                    },
                    Err(_) => None,
                });
        Ok(accepted_turn.unwrap_or(turn_id))
    }

    /// Rewinds a session to a turn and reports which turns were dropped.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn rewind(
        &self,
        session: &str,
        to_turn_id: &str,
        inclusive: bool,
    ) -> Result<Vec<String>, ApiError> {
        let frame = Self::frame(
            session,
            FrameType::Standard(StandardFrameType::SessionRewind),
            &super::types::RewindRequest {
                to_turn_id,
                inclusive,
                cancel_pending: false,
                workspace: super::types::RewindWorkspace::Keep,
            },
        )?;
        let response = self.post_command(session, &frame).await?;
        Ok(response
            .events
            .iter()
            .filter_map(|event| crate::decode_frame(event).ok())
            .find_map(|decoded| match decoded.event.kind {
                norte_code_wire::ServerEventKind::HistoryRewound {
                    rewound_turn_ids, ..
                } => Some(rewound_turn_ids),
                _ => None,
            })
            .unwrap_or_default())
    }

    /// Forks a session, optionally at a turn, and returns the new session id.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn fork(
        &self,
        session: &str,
        from_turn_id: Option<&str>,
        title: Option<&str>,
    ) -> Result<String, ApiError> {
        let frame = Self::frame(
            session,
            FrameType::Standard(StandardFrameType::SessionFork),
            &super::types::ForkRequest {
                from_turn_id,
                title,
                include: super::types::ForkInclude::default(),
            },
        )?;
        let response = self.post_command(session, &frame).await?;
        response
            .events
            .iter()
            .filter_map(|event| crate::decode_frame(event).ok())
            .find(|decoded| {
                matches!(
                    decoded.event.kind,
                    norte_code_wire::ServerEventKind::SessionForked { .. }
                )
            })
            .and_then(|decoded| {
                decoded
                    .event
                    .session
                    .map(|session| session.as_str().to_owned())
            })
            .ok_or_else(|| {
                ApiError::InvalidResponse(
                    "The server did not return the forked session.".to_owned(),
                )
            })
    }

    /// Compacts a session now.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn compact(&self, session: &str) -> Result<(), ApiError> {
        let frame = Self::frame(
            session,
            FrameType::Standard(StandardFrameType::SessionCompact),
            &super::types::CompactRequest {
                strategy: super::types::CompactStrategy::Summarize,
            },
        )?;
        self.post_command(session, &frame).await.map(|_| ())
    }

    /// Pins or unpins turns so compaction keeps them verbatim.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn pin_turns(
        &self,
        session: &str,
        turn_ids: &[String],
        pinned: bool,
    ) -> Result<Vec<String>, ApiError> {
        let frame = Self::frame(
            session,
            FrameType::Standard(StandardFrameType::ContextPin),
            &super::types::PinRequest { turn_ids, pinned },
        )?;
        let response = self.post_command(session, &frame).await?;
        Ok(response
            .events
            .iter()
            .find(|event| event.kind == FrameType::Standard(StandardFrameType::ContextUpdated))
            .and_then(|event| event.payload["pinned_turn_ids"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect())
    }
}
