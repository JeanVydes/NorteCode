//! A spawned turn owns its session until it has settled its durable state.
//! Hosts consume its [`futures_core::Stream`] and join with [`Turn::finish`].
//! Cancellation never drops an input, consultation or persistence future.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::runtime::event::EngineEvent;
use crate::runtime::interceptor::{ConsultationPoint, Interception, InterceptionContext};
use crate::runtime::{Commit, Node, Phase, Session};
use norte::domain::message::Message;
use norte::domain::role::Role;
use norte::error::{Error, Result};
use norte::id::{NodeId, SessionId};
use norte::model::TurnOptions;

use super::{Harness, TurnOutcome, session_turn_options, turn_metadata, unanswered_call_errors};

/// An already-running turn with a bounded, ordered event channel.
///
/// Read events until `None`, then call [`Self::finish`] for the outcome or error.
/// Calling `finish` directly also works: it discards unread events while joining.
/// Dropping the turn requests cancellation; it never aborts its task or a commit.
#[must_use = "read the turn's events and finish it, or drop it to request cancellation"]
pub struct Turn {
    events: mpsc::Receiver<EngineEvent>,
    task: Option<JoinHandle<Result<TurnOutcome>>>,
    cancellation: CancellationToken,
}

impl Turn {
    pub(crate) fn new(
        engine: Harness,
        session: SessionId,
        input: Option<Message>,
        options: TurnOptions,
        cancellation: CancellationToken,
    ) -> Self {
        let (sender, events) = mpsc::channel(engine.config.limits.max_buffered_events.max(1));
        let token = cancellation.clone();
        let task = tokio::spawn(async move {
            engine
                .run_turn(session, input, options, &sender, &token)
                .await
        });
        Self {
            events,
            task: Some(task),
            cancellation,
        }
    }

    /// Discards remaining events and joins the running task without cancelling it.
    ///
    /// Draining before joining lets a bounded channel make progress even when
    /// the host does not need the remaining events. Dropping this future requests
    /// cancellation, just like dropping the turn.
    ///
    /// # Errors
    ///
    /// Returns the runtime error, or an error if the spawned task failed to join.
    pub async fn finish(mut self) -> Result<TurnOutcome> {
        while self.events.recv().await.is_some() {}
        let joined = self
            .task
            .as_mut()
            .ok_or_else(|| Error::Other("turn task already consumed".to_owned()))?
            .await;
        drop(self.task.take());
        joined.map_err(|error| Error::Other(format!("turn task failed: {error}")))?
    }

    /// Requests cooperative cancellation; persistence is allowed to finish.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
}

impl Stream for Turn {
    type Item = EngineEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_recv(cx)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if self.task.is_some() {
            self.cancellation.cancel();
        }
    }
}

impl Harness {
    /// No cancellation races this owner: only the request and tools observe the
    /// token internally. Every exit settles Streaming without losing its head.
    async fn run_turn(
        &self,
        session_id: SessionId,
        mut input: Option<Message>,
        options: TurnOptions,
        sender: &mpsc::Sender<EngineEvent>,
        cancellation: &CancellationToken,
    ) -> Result<TurnOutcome> {
        let mut session = self.store.load(session_id).await?;
        let effective =
            options.over(&session_turn_options(&session).over(&self.default_turn_options()));
        let result = self
            .run_turn_loop(&mut session, &mut input, &effective, sender, cancellation)
            .await;
        // A failure while recovering parked tools must not discard the new input.
        // run_tools settles all calls before returning an execution/hook error.
        if session.phase != Phase::AwaitingTool
            && let Some(message) = input.take()
        {
            session = self.append_turn(&session, message, sender).await?;
        }
        self.settle_streaming(&mut session, sender).await?;
        result.map(|message| TurnOutcome { session, message })
    }

    #[allow(clippy::too_many_lines)]
    async fn run_turn_loop(
        &self,
        session: &mut Session,
        input: &mut Option<Message>,
        options: &TurnOptions,
        sender: &mpsc::Sender<EngineEvent>,
        cancellation: &CancellationToken,
    ) -> Result<Option<Message>> {
        let mut last = None;
        let mut generations = 0;
        let mut stopped = false;
        loop {
            match session.phase {
                Phase::AwaitingTool => {
                    // Even a cancelled turn must settle every committed call.
                    self.run_tools(session, sender, cancellation).await?;
                    if !cancellation.is_cancelled() {
                        let history = self.store.history(session.head).await?;
                        stopped = self
                            .consult(
                                session,
                                sender,
                                Some(&history),
                                ConsultationPoint::PostTools,
                            )
                            .await?
                            .is_some();
                    }
                    // Visit the input branch before observing stop/cancellation:
                    // a new input must follow, never split, the call/result pair.
                }
                Phase::Idle | Phase::Streaming => {
                    if input.is_some() && self.append_input(session, input, sender).await? {
                        return Ok(last);
                    }
                    if stopped || cancellation.is_cancelled() {
                        return Ok(last);
                    }

                    if let Some(head) = session.head {
                        let message = self.store.node(head).await?.message;
                        if message.role == Role::Assistant {
                            let before = session.head;
                            let history = self.store.history(before).await?;
                            let stop = self
                                .consult(
                                    session,
                                    sender,
                                    Some(&history),
                                    ConsultationPoint::EndOfTurn,
                                )
                                .await?;
                            if stop.is_some() || session.head == before {
                                return Ok(last.or(Some(message)));
                            }
                            continue;
                        }
                    }

                    if generations >= self.config.max_iterations {
                        return Err(Error::InvalidInput(format!(
                            "agent exceeded {} iterations",
                            self.config.max_iterations
                        )));
                    }
                    let Some((head, history)) = self.prepare_request(session, sender).await? else {
                        return Ok(last);
                    };
                    if cancellation.is_cancelled() {
                        return Ok(last);
                    }
                    generations += 1;
                    let (message, interrupted) = self
                        .generate(session, history, options, sender, cancellation)
                        .await?;
                    // Never race a generation commit against cancellation.
                    self.commit_answer(session, &message, head, interrupted, options, sender)
                        .await?;
                    last = Some(message);
                    if interrupted {
                        // commit_answer closes all interrupted calls atomically.
                        return Ok(last);
                    }
                }
                phase => {
                    return Err(Error::Other(format!(
                        "unsupported session phase: {phase:?}"
                    )));
                }
            }
        }
    }

    /// Leaves accepted input/injections intact when no answer was generated.
    async fn settle_streaming(
        &self,
        session: &mut Session,
        sender: &mpsc::Sender<EngineEvent>,
    ) -> Result<()> {
        if session.phase == Phase::Streaming {
            *session = self
                .commit(
                    Commit {
                        session_id: session.id,
                        expected_version: Some(session.version),
                        nodes: Vec::new(),
                        head: None,
                        phase: Phase::Idle,
                        turn: session.turn,
                        metadata: None,
                    },
                    sender,
                )
                .await?;
            self.emit(
                sender,
                EngineEvent::PhaseChanged {
                    session: session.id,
                    from: Phase::Streaming,
                    to: Phase::Idle,
                },
            )
            .await;
        }
        Ok(())
    }

    /// Chains input onto the head unless the host explicitly rejects the turn.
    async fn append_input(
        &self,
        session: &mut Session,
        input: &mut Option<Message>,
        sender: &mpsc::Sender<EngineEvent>,
    ) -> Result<bool> {
        if session.phase == Phase::Idle {
            let history = self.store.history(session.head).await?;
            if self
                .consult(session, sender, Some(&history), ConsultationPoint::PreInput)
                .await?
                .is_some()
            {
                drop(input.take());
                return Ok(true);
            }
        }
        if let Some(message) = input.take() {
            *session = self.append_turn(session, message, sender).await?;
        }
        Ok(false)
    }

    /// Commits the answer and closes any calls an interruption left unanswered.
    async fn commit_answer(
        &self,
        session: &mut Session,
        message: &Message,
        head: NodeId,
        interrupted: bool,
        options: &TurnOptions,
        sender: &mpsc::Sender<EngineEvent>,
    ) -> Result<()> {
        let node = Node::message(message.clone(), Some(head));
        let node_id = node.id;
        let phase = if !interrupted && message.has_tool_calls() {
            Phase::AwaitingTool
        } else {
            Phase::Idle
        };
        let mut nodes = vec![node];
        if interrupted {
            let unanswered = unanswered_call_errors(message);
            if !unanswered.is_empty() {
                nodes.push(Node::message(Message::tool(unanswered), Some(node_id)));
            }
        }
        let head = nodes.last().map(|node| node.id);
        *session = self
            .commit(
                Commit {
                    session_id: session.id,
                    expected_version: Some(session.version),
                    nodes,
                    head,
                    phase,
                    turn: session.turn,
                    metadata: Some(turn_metadata(session, options)),
                },
                sender,
            )
            .await?;
        self.emit(
            sender,
            EngineEvent::GenerationCommitted {
                session: session.id,
                turn: session.turn,
            },
        )
        .await;
        Ok(())
    }

    /// Consults the host without racing consumed injections against cancellation.
    async fn prepare_request(
        &self,
        session: &mut Session,
        sender: &mpsc::Sender<EngineEvent>,
    ) -> Result<Option<(NodeId, Vec<Message>)>> {
        let before = session.head;
        let history = if self.interceptors.is_empty() {
            None
        } else {
            Some(self.store.history(before).await?)
        };
        if self
            .consult(
                session,
                sender,
                history.as_deref(),
                ConsultationPoint::PreflightRequest,
            )
            .await?
            .is_some()
        {
            return Ok(None);
        }
        let Some(head) = session.head else {
            return Ok(None);
        };
        let history = match history {
            Some(previous) if session.head == before => previous,
            _ => self.store.history(Some(head)).await?,
        };
        Ok(Some((head, history)))
    }

    /// Persists consumed injections even if a subsequent interceptor fails.
    async fn consult(
        &self,
        session: &mut Session,
        sender: &mpsc::Sender<EngineEvent>,
        history: Option<&[Message]>,
        point: ConsultationPoint,
    ) -> Result<Option<String>> {
        if self.interceptors.is_empty() {
            return Ok(None);
        }
        let context = InterceptionContext {
            point,
            session: session.id,
            turn: session.turn,
            head: session.head,
            messages: history.unwrap_or_default(),
            events: sender,
        };
        let mut messages = Vec::new();
        let mut stop = None;
        let mut error = None;
        for interceptor in self.interceptors.interceptors() {
            match interceptor.intercept(&context).await {
                Ok(Interception::Continue) => {}
                Ok(Interception::Inject(added)) => messages.extend(added),
                Ok(Interception::Stop { reason } | Interception::RefuseCall { reason }) => {
                    self.emit(
                        sender,
                        EngineEvent::TurnStopped {
                            session: session.id,
                            interceptor: interceptor.name().to_owned(),
                            reason: reason.clone(),
                        },
                    )
                    .await;
                    stop = Some(reason);
                }
                Err(failure) => {
                    error = Some(failure);
                    break;
                }
            }
        }
        if !messages.is_empty() {
            let count = messages.len();
            let mut parent = session.head;
            let mut nodes = Vec::with_capacity(count);
            for message in messages {
                let node = Node::message(message, parent);
                parent = Some(node.id);
                nodes.push(node);
            }
            *session = self
                .commit(
                    Commit {
                        session_id: session.id,
                        expected_version: Some(session.version),
                        nodes,
                        head: parent,
                        phase: session.phase,
                        turn: session.turn,
                        metadata: None,
                    },
                    sender,
                )
                .await?;
            self.emit(
                sender,
                EngineEvent::InterceptionApplied {
                    session: session.id,
                    messages: count,
                },
            )
            .await;
        }
        match error {
            Some(error) => Err(error),
            None => Ok(stop),
        }
    }

    /// Chains the turn's input onto the head.
    ///
    /// # Errors
    ///
    /// Returns a store or store-conflict error.
    pub async fn append_turn(
        &self,
        session: &Session,
        message: Message,
        sender: &mpsc::Sender<EngineEvent>,
    ) -> Result<Session> {
        let node = Node::message(message, session.head);
        let head = node.id;
        let previous_phase = session.phase;
        let committed = self
            .commit(
                Commit {
                    session_id: session.id,
                    expected_version: Some(session.version),
                    nodes: vec![node],
                    head: Some(head),
                    phase: Phase::Streaming,
                    turn: session.turn + 1,
                    metadata: None,
                },
                sender,
            )
            .await?;
        self.emit(
            sender,
            EngineEvent::PhaseChanged {
                session: session.id,
                from: previous_phase,
                to: Phase::Streaming,
            },
        )
        .await;
        Ok(committed)
    }
}
