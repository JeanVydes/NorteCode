use norte_code_wire::{Budget, ClientRequestKind, Reasoning, ReasoningScope, SessionKey};
use raep_types::{ClientInfo, FrameType, SessionAttach, StandardFrameType, TurnId};

use super::shared::{EmptyPayload, accepted};
use super::{ApiError, CLIENT_NAME, RestClient};

impl RestClient {
    /// Attaches to a session with the client's info and capabilities.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn attach(&self, session: &str, agent_id: &str) -> Result<(), ApiError> {
        let payload = SessionAttach {
            agent_id: agent_id.to_owned(),
            agent_override: None,
            create_if_missing: Some(false),
            mode: None,
            takeover: None,
            resume_from_seq: None,
            capabilities: None,
            client_info: Some(ClientInfo {
                name: Some(CLIENT_NAME.to_owned()),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            options: None,
        };
        let response = self
            .post_frame(
                session,
                FrameType::Standard(StandardFrameType::SessionAttach),
                &payload,
            )
            .await?;
        accepted(response, "Session attachment was rejected.")
    }

    /// Detaches from a session.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn detach(&self, session: &str) -> Result<(), ApiError> {
        let response = self
            .post_frame(
                session,
                FrameType::Standard(StandardFrameType::SessionDetach),
                &EmptyPayload {},
            )
            .await?;
        accepted(response, "Session detachment was rejected.")
    }

    /// Cancels one turn.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn cancel_turn(&self, session: &str, turn: &TurnId) -> Result<(), ApiError> {
        let mut frame = Self::frame(
            session,
            FrameType::Standard(StandardFrameType::CancelTurn),
            &EmptyPayload {},
        )?;
        frame.turn_id = Some(turn.clone());
        let response = self.post_command(session, &frame).await?;
        accepted(response, "Turn cancellation was rejected.")
    }

    /// Sets the session's model.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn set_session_model(
        &self,
        session: &str,
        provider: &str,
        model: &str,
    ) -> Result<(), ApiError> {
        self.command_ok(
            Some(session),
            ClientRequestKind::SetModel {
                session: SessionKey::new(session),
                provider: provider.to_owned(),
                model: model.to_owned(),
            },
        )
        .await
    }

    /// Sets how much the session reasons.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn set_reasoning(
        &self,
        session: &str,
        scope: ReasoningScope,
        reasoning: Reasoning,
    ) -> Result<(), ApiError> {
        self.command_ok(
            Some(session),
            ClientRequestKind::SetReasoning {
                session: SessionKey::new(session),
                scope,
                reasoning,
            },
        )
        .await
    }

    /// Sets the session's spending limit.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn set_budget(&self, session: &str, budget: Budget) -> Result<(), ApiError> {
        self.command_ok(
            Some(session),
            ClientRequestKind::SetBudget {
                session: SessionKey::new(session),
                budget,
            },
        )
        .await
    }

    /// Answers a question a turn is parked on.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn answer_question(
        &self,
        session: &str,
        question: &str,
        answer: &str,
    ) -> Result<(), ApiError> {
        self.command_ok(
            Some(session),
            ClientRequestKind::AnswerQuestion {
                session: SessionKey::new(session),
                question: question.to_owned(),
                answer: answer.to_owned(),
            },
        )
        .await
    }
}
