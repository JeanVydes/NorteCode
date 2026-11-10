use super::escape;
use async_trait::async_trait;
use norte::{FnExecutor, Result, SessionId, ToolContext, ToolDeclaration, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};

/// How long `ask_user` waits for an answer.
pub const QUESTION_TIMEOUT: Duration = Duration::from_secs(600);

/// One question waiting for a client.
#[derive(Clone, Debug)]
pub struct QuestionEvent {
    /// The question's id.
    pub id: String,
    /// The session that asked, when it has one.
    pub session: Option<SessionId>,
    /// What the model wants to know.
    pub question: String,
    /// Closed choices, when the model offered any.
    pub options: Vec<String>,
    /// Whether more than one option may be chosen.
    pub multiple: bool,
}

/// A durable record of a question, for hosts that survive a restart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableQuestion {
    /// The id shown to the client.
    pub id: String,
    /// The session that asked.
    pub session: SessionId,
    /// The tool call it belongs to, the durable key.
    pub call_id: String,
    /// What the model asked.
    pub question: String,
    /// Closed choices, when the model offered any.
    pub options: Vec<String>,
    /// Whether more than one option may be chosen.
    pub multiple: bool,
    /// The answer, once one was given.
    pub answer: Option<String>,
}

/// Storage for pending questions.
///
/// The broker keeps the live wait in memory; this trait is the write-through
/// log that lets a restart re-ask a question or replay a late answer.
#[async_trait]
pub trait QuestionStore: Send + Sync {
    /// The record for a call, if one was stored.
    async fn load(&self, call_id: &str) -> Option<DurableQuestion>;
    /// Stores a question that is now waiting.
    async fn save(&self, question: &DurableQuestion);
    /// Stores the answer. Returns `false` when the answer was already recorded.
    async fn answer(&self, call_id: &str, answer: &str) -> bool;
}

/// A question still waiting for its answer.
#[derive(Debug)]
struct PendingQuestion {
    session: Option<SessionId>,
    call_id: Option<String>,
    sender: oneshot::Sender<String>,
}

/// The questions a running turn is waiting on.
///
/// The broker lives above the tool so a client can answer a session without
/// cancelling the turn. With a [`QuestionStore`] attached, a question and its
/// answer survive a restart: a resumed turn re-asks an unanswered question and
/// replays an answer that arrived late.
pub struct QuestionBroker {
    next: AtomicU64,
    pending: Mutex<BTreeMap<String, PendingQuestion>>,
    events: broadcast::Sender<QuestionEvent>,
    timeout: Duration,
    store: Option<Arc<dyn QuestionStore>>,
}

impl std::fmt::Debug for QuestionBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuestionBroker")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Default for QuestionBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl QuestionBroker {
    /// A broker with the default timeout.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
            pending: Mutex::new(BTreeMap::new()),
            events: broadcast::channel(64).0,
            timeout: QUESTION_TIMEOUT,
            store: None,
        }
    }

    /// Attaches the durable log questions are written to.
    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn QuestionStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// A broker that gives up after `timeout`.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout.max(Duration::from_secs(1));
        self
    }

    /// A receiver for the questions asked from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<QuestionEvent> {
        self.events.subscribe()
    }

    /// Drops every question of `session` that is still waiting.
    ///
    /// A cancelled turn cannot answer, and the tool waiting on the oneshot
    /// only ends when the receiver is dropped or answered. Without this a
    /// cancelled turn kept its `ask_user` open forever: the turn never
    /// finished, and the session stayed `SESSION_BUSY` for good.
    pub fn abandon_session(&self, session: Option<SessionId>) -> usize {
        let abandoned: Vec<String> = {
            let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            pending
                .iter()
                .filter(|(_, question)| question.session == session)
                .map(|(id, _)| id.clone())
                .collect()
        };
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let mut dropped = 0;
        for id in abandoned {
            if pending.remove(&id).is_some() {
                dropped += 1;
            }
        }
        dropped
    }

    /// Answers a pending question. Returns `false` when it is no longer waiting
    /// or belongs to another session.
    pub async fn answer(&self, session: Option<SessionId>, id: &str, answer: String) -> bool {
        let question = {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            match pending.get(id) {
                Some(question) if question.session == session => pending.remove(id),
                _ => None,
            }
        };
        let Some(question) = question else {
            return false;
        };
        if let (Some(store), Some(call_id)) = (&self.store, &question.call_id) {
            let _ = store.answer(call_id, &answer).await;
        }
        question.sender.send(answer).is_ok()
    }

    /// Asks a question and waits for its answer or the turn's cancellation.
    ///
    /// With a [`QuestionStore`], the call id is the durable key: a resumed turn
    /// finds the same question, replays an answer that was already given, or
    /// asks again when none was.
    ///
    /// # Errors
    ///
    /// Returns a message when no client is listening, the turn is cancelled,
    /// the question is dropped, or the timeout expires.
    pub async fn ask(
        &self,
        context: &ToolContext,
        mut question: String,
        mut options: Vec<String>,
        mut multiple: bool,
    ) -> std::result::Result<String, String> {
        let call_id = context.call_id.map(|call| call.to_string());
        let mut id = call_id.as_ref().map_or_else(
            || format!("q{}", self.next.fetch_add(1, Ordering::Relaxed)),
            |call| format!("q-{call}"),
        );
        if let (Some(store), Some(call), Some(session)) =
            (&self.store, call_id.as_deref(), context.session)
        {
            if let Some(existing) = store.load(call).await {
                id.clone_from(&existing.id);
                if let Some(answer) = existing.answer {
                    return Ok(answer);
                }
                question.clone_from(&existing.question);
                options.clone_from(&existing.options);
                multiple = existing.multiple;
            } else {
                store
                    .save(&DurableQuestion {
                        id: id.clone(),
                        session,
                        call_id: call.to_owned(),
                        question: question.clone(),
                        options: options.clone(),
                        multiple,
                        answer: None,
                    })
                    .await;
            }
        }
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id.clone(),
                PendingQuestion {
                    session: context.session,
                    call_id,
                    sender,
                },
            );

        let event = QuestionEvent {
            id: id.clone(),
            session: context.session,
            question,
            options,
            multiple,
        };
        if self.events.send(event).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            return Err(
                "no client is listening for questions; answer in plain text instead".to_owned(),
            );
        }
        let outcome = tokio::select! {
            () = context.cancelled() => {
                Err("the turn was cancelled while waiting for an answer".to_owned())
            }
            result = tokio::time::timeout(self.timeout, receiver) => match result {
                Ok(Ok(answer)) => Ok(answer),
                Ok(Err(_)) => Err("the question was dropped before it was answered".to_owned()),
                Err(_) => Err(format!(
                    "no answer after {} seconds; ask again or decide from context",
                    self.timeout.as_secs()
                )),
            }
        };
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        outcome
    }
}

/// Arguments for `ask_user`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUserArgs {
    /// The one question to ask, with enough context to answer without guessing.
    pub question: String,
    /// Closed choices, when the answer should be one of them instead of open
    /// text. The client adds an "answer something else" choice that falls back
    /// to the normal free-form answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// Whether several choices may be selected. Only meaningful with `options`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiple: Option<bool>,
}

/// Ask the human a question and wait for the answer.
#[derive(Clone, Debug)]
pub struct AskUserTool {
    broker: Arc<QuestionBroker>,
}

impl AskUserTool {
    /// A tool backed by `broker`.
    #[must_use]
    pub fn new(broker: Arc<QuestionBroker>) -> Self {
        Self { broker }
    }

    /// Turns the tool into a declaration the core registry accepts.
    #[must_use]
    pub fn into_declaration(self) -> ToolDeclaration<AskUserArgs> {
        ToolDeclaration::new(
            "ask_user",
            "Ask the human one question and wait for their answer. Use it only when the task cannot proceed without a decision or missing fact, not for confirmation you can infer. Pass `options` to offer closed choices; set `multiple` to true when several may apply. The client always adds a free-form \"something else\" choice.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<AskUserArgs, ToolOutput> for AskUserTool {
    async fn call(&self, _args: AskUserArgs) -> Result<ToolOutput> {
        Ok(ToolOutput::error(
            "ask_user needs a client that can answer questions",
        ))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: AskUserArgs,
    ) -> Result<ToolOutput> {
        if args.options.as_ref().is_some_and(Vec::is_empty) {
            return Ok(ToolOutput::error("ask_user: options must not be empty"));
        }
        if args.multiple == Some(true) && args.options.is_none() {
            return Ok(ToolOutput::error(
                "ask_user: multiple needs a non-empty options list",
            ));
        }
        let options = args.options.unwrap_or_default();
        let multiple = args.multiple.unwrap_or(false);
        match self
            .broker
            .ask(context, args.question, options, multiple)
            .await
        {
            Ok(answer) => Ok(ToolOutput::text(format!(
                "The user answered: {}",
                escape(&answer)
            ))),
            Err(message) => Ok(ToolOutput::error(message)),
        }
    }
}
