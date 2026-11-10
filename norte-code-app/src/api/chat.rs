//! One group of endpoints the client exposes.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl ApiClient {
    pub async fn cancel_turn(&self, session_id: &str, turn_id: &TurnId) -> Result<(), ApiError> {
        let response = self
            .post_turn_frame(
                session_id,
                FrameType::Standard(StandardFrameType::CancelTurn),
                turn_id,
                &EmptyRequest {},
            )
            .await?;
        if response.accepted {
            Ok(())
        } else {
            Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "Turn cancellation was rejected.".to_owned(),
                |e| e.message,
            )))
        }
    }

    /// Sets how much the session reasons, now or for the next message.
    pub async fn set_reasoning(
        &self,
        session_id: &str,
        scope: ReasoningScope,
        reasoning: Reasoning,
    ) -> Result<(), ApiError> {
        self.norte_code_command(
            Some(session_id),
            ClientRequestKind::SetReasoning {
                session: SessionKey::new(session_id),
                scope,
                reasoning,
            },
        )
        .await
        .map(|_| ())
    }

    /// Sets the session's spending limit.
    pub async fn set_budget(&self, session_id: &str, budget: Budget) -> Result<(), ApiError> {
        self.norte_code_command(
            Some(session_id),
            ClientRequestKind::SetBudget {
                session: SessionKey::new(session_id),
                budget,
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn answer_question(
        &self,
        session_id: &str,
        question: &str,
        answer: &str,
    ) -> Result<(), ApiError> {
        self.norte_code_command(
            Some(session_id),
            ClientRequestKind::AnswerQuestion {
                session: SessionKey::new(session_id),
                question: question.to_owned(),
                answer: answer.to_owned(),
            },
        )
        .await
        .map(|_| ())
    }
}
