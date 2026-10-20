//! Durable question storage backed by the session directory.

use std::sync::Arc;

use async_trait::async_trait;
use norte_code_store_sqlite::{SessionDirectory, StoredQuestion};
use norte_code_tools::{DurableQuestion, QuestionStore};

/// Bridges the question broker's durable log onto the session directory.
pub(super) struct DirectoryQuestions {
    directory: Arc<dyn SessionDirectory>,
}

impl DirectoryQuestions {
    pub(super) fn new(directory: Arc<dyn SessionDirectory>) -> Self {
        Self { directory }
    }
}

#[async_trait]
impl QuestionStore for DirectoryQuestions {
    async fn load(&self, call_id: &str) -> Option<DurableQuestion> {
        match self.directory.question(call_id).await {
            Ok(question) => question.map(|stored| DurableQuestion {
                id: stored.id,
                session: stored.session,
                call_id: stored.call_id,
                question: stored.question,
                options: stored.options,
                multiple: stored.multiple,
                answer: stored.answer,
            }),
            Err(error) => {
                tracing::warn!(%error, "could not read a stored question");
                None
            }
        }
    }

    async fn save(&self, question: &DurableQuestion) {
        let stored = StoredQuestion {
            id: question.id.clone(),
            session: question.session,
            call_id: question.call_id.clone(),
            question: question.question.clone(),
            options: question.options.clone(),
            multiple: question.multiple,
            answer: question.answer.clone(),
        };
        if let Err(error) = self.directory.save_question(&stored).await {
            tracing::warn!(%error, "could not store a pending question");
        }
    }

    async fn answer(&self, call_id: &str, answer: &str) -> bool {
        match self.directory.answer_question(call_id, answer).await {
            Ok(recorded) => recorded,
            Err(error) => {
                tracing::warn!(%error, "could not store a question answer");
                false
            }
        }
    }
}
