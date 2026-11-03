//! The turn loop: what the engine does between the session's head and the
//! model's answer.
//!
//! The loop reads the session, asks the host's interceptors what the turn
//! does next, builds the request, commits the answer and repeats until the
//! turn ends. The host gets the last word: a message it was holding keeps the
//! turn alive, and it may stop the turn with a reason.

use futures_core::Stream;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::runtime::Commit;
use crate::runtime::Node;
use crate::runtime::Phase;
use crate::runtime::Session;
use crate::runtime::event::EngineEvent;
use crate::runtime::interceptor::{ConsultationPoint, Interception, InterceptionContext};
use norte::domain::message::Message;
use norte::domain::part::ToolResultPart;
use norte::domain::role::Role;
use norte::error::{Error, Result};
use norte::id::{NodeId, SessionId};
use norte::model::TurnOptions;

use super::{
    Consultation, Harness, TurnOutcome, session_turn_options, turn_metadata, unanswered_call_errors,
};

/// One step of a host-driven turn.
#[derive(Clone, Debug)]
pub enum Step {
    /// `AwaitingTool` phase: the tool calls ran.
    Tools,
    /// Idle/Streaming phase: an answer was generated and committed.
    Generated {
        /// The assistant message that was committed.
        message: Box<Message>,
        /// Whether the generation was interrupted.
        interrupted: bool,
    },
    /// The turn finished (final answer or host stop).
    Done(Box<TurnOutcome>),
}

/// Internal result of the end-of-turn check.
pub(crate) enum EndOfTurn {
    /// The host added messages; re-evaluate.
    Continue,
    /// The turn finished.
    Finish(Box<TurnOutcome>),
}

/// The outcome of one `Turn::step`: the step plus the events it produced.
#[derive(Debug)]
pub struct StepResult {
    /// The step that was taken.
    pub step: Step,
    /// The engine events produced during the step, in order.
    pub events: Vec<EngineEvent>,
}

/// Runtime state of a turn, initialised lazily on the first `step()`.
#[derive(Default)]
struct TurnState {
    session: Option<Session>,
    effective: Option<TurnOptions>,
    last: Option<Message>,
    early_results: Vec<ToolResultPart>,
    iterations: usize,
    input_consumed: bool,
    finished: Option<TurnOutcome>,
}

/// The phase a turn is in, so its state is one value and not a handful of
/// fields that must agree with each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnPhase {
    /// The session has not been loaded yet.
    Init,
    /// The input is being consumed.
    Input,
    /// Checking whether the turn has ended.
    CheckEnd,
    /// Running the tool calls of one iteration.
    AwaitingTool,
    /// Generating and committing an answer.
    Generating,
    /// The turn finished.
    Done,
}

/// A host-driven turn: one iteration per `step()` call.
pub struct Turn {
    engine: Harness,
    session_id: SessionId,
    input: Option<Message>,
    options: TurnOptions,
    cancellation: CancellationToken,
    events_tx: mpsc::Sender<EngineEvent>,
    events_rx: mpsc::Receiver<EngineEvent>,
    state: TurnState,
    phase: TurnPhase,
}

impl Turn {
    /// Creates a turn that has not yet loaded its session.
    pub(crate) fn new(
        engine: Harness,
        session_id: SessionId,
        input: Option<Message>,
        options: TurnOptions,
        cancellation: CancellationToken,
    ) -> Self {
        let (tx, rx) = mpsc::channel(engine.config.limits.max_buffered_events);
        Self {
            engine,
            session_id,
            input,
            options,
            cancellation,
            events_tx: tx,
            events_rx: rx,
            state: TurnState::default(),
            phase: TurnPhase::Init,
        }
    }

    /// Ensures the session is loaded and the effective options computed.
    /// Loads the session and computes the effective options.
    ///
    /// # Errors
    ///
    /// Returns the store error when the session cannot be loaded.
    pub async fn init(&mut self) -> Result<()> {
        if self.state.session.is_some() {
            return Ok(());
        }
        let session = self.engine.store.load(self.session_id).await?;
        let effective = self
            .options
            .clone()
            .over(&session_turn_options(&session).over(&self.engine.default_turn_options()));
        self.state.session = Some(session);
        self.state.effective = Some(effective);
        self.phase = TurnPhase::Input;
        Ok(())
    }

    /// Advances the turn by one iteration.
    ///
    /// Returns `Ok(None)` once the turn has finished.
    /// # Errors
    ///
    /// Returns an error when the iteration limit is exceeded or a store,
    /// model or tool call fails.
    ///
    /// # Panics
    ///
    /// Panics if called before `init` has run (internal invariant; `step`
    /// initialises on first call).
    #[allow(clippy::too_many_lines)]
    pub async fn step(&mut self) -> Result<Option<StepResult>> {
        if let Some(outcome) = &self.state.finished {
            return Ok(Some(StepResult {
                step: Step::Done(Box::new(outcome.clone())),
                events: Vec::new(),
            }));
        }
        self.init().await?;

        // Consume the input on the first step.
        if !self.state.input_consumed {
            let session = self.state.session.as_mut().expect("initialised");
            if let Some(outcome) = self
                .engine
                .append_input(session, self.input.take(), &self.events_tx)
                .await?
            {
                self.state.finished = Some(outcome.clone());
                self.phase = TurnPhase::Done;
                self.state.session = Some(session.clone());
                let events = self.drain_events();
                return Ok(Some(StepResult {
                    step: Step::Done(Box::new(outcome)),
                    events,
                }));
            }
            self.state.input_consumed = true;
        }

        let events_tx = self.events_tx.clone();
        let cancellation = self.cancellation.clone();

        loop {
            self.state.iterations += 1;
            if self.state.iterations > self.engine.config.max_iterations {
                return Err(Error::InvalidInput(format!(
                    "agent exceeded {} iterations",
                    self.engine.config.max_iterations
                )));
            }

            let session_mut = self.state.session.as_mut().expect("initialised");
            match self
                .engine
                .end_of_turn_check(session_mut, &events_tx, &mut self.state.last)
                .await?
            {
                Some(EndOfTurn::Continue) => continue,
                Some(EndOfTurn::Finish(outcome)) => {
                    let outcome = *outcome;
                    self.state.finished = Some(outcome.clone());
                    self.phase = TurnPhase::Done;
                    self.state.session = Some(session_mut.clone());
                    let events = self.drain_events();
                    return Ok(Some(StepResult {
                        step: Step::Done(Box::new(outcome)),
                        events,
                    }));
                }
                None => {}
            }

            let session_mut = self.state.session.as_mut().expect("initialised");
            let effective = self.state.effective.clone().expect("initialised");
            let step_result = self
                .engine
                .run_one_iteration(
                    session_mut,
                    &mut self.state.last,
                    &mut self.state.early_results,
                    &effective,
                    &events_tx,
                    &cancellation,
                )
                .await;
            match step_result {
                Ok(step) => {
                    // Persist the (possibly updated) session back.
                    self.state.session = Some(session_mut.clone());

                    let events = self.drain_events();
                    match step {
                        Step::Done(outcome) => {
                            self.state.finished = Some((*outcome).clone());
                            self.phase = TurnPhase::Done;
                            return Ok(Some(StepResult {
                                step: Step::Done(outcome),
                                events,
                            }));
                        }
                        other => {
                            return Ok(Some(StepResult {
                                step: other,
                                events,
                            }));
                        }
                    }
                }
                Err(error) => {
                    // Ask the host interceptors how to handle the error.
                    let retry = self
                        .engine
                        .handle_error(session_mut, &events_tx, &error)
                        .await?;
                    if retry {
                        self.state.session = Some(session_mut.clone());
                        continue;
                    }
                    self.state.session = Some(session_mut.clone());
                    // Leave the events in the buffer for the caller to drain
                    // and process: draining here loses LayerShortCircuited and
                    // other events the caller needs to report.
                    return Err(error);
                }
            }
        }
    }

    /// Drains all buffered engine events produced so far.
    pub fn drain_events(&mut self) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.events_rx.try_recv() {
            out.push(ev);
        }
        out
    }

    /// Drives the turn to completion and returns its outcome.
    ///
    /// # Errors
    ///
    /// Returns an error when a step fails.
    /// Drives the turn to completion and returns its outcome.
    ///
    /// # Errors
    ///
    /// Returns an error when a step fails.
    pub async fn finish(&mut self) -> Result<TurnOutcome> {
        loop {
            if let Some(StepResult {
                step: Step::Done(outcome),
                ..
            }) = self.step().await?
            {
                return Ok(*outcome);
            }
        }
    }

    /// Cancels the turn at the next await point.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// The session as last committed.
    #[must_use]
    /// # Panics
    ///
    /// Panics if `step` has never been called.
    pub fn session(&self) -> &Session {
        self.state.session.as_ref().expect("step() not yet called")
    }

    /// The receiver for engine events produced by this turn.
    pub fn events(&mut self) -> &mut mpsc::Receiver<EngineEvent> {
        &mut self.events_rx
    }
}

impl Stream for Turn {
    type Item = Result<EngineEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        match self.events_rx.poll_recv(cx) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(Ok(event))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Harness {
    /// Chains the turn's input onto the head, asking the host first.
    ///
    /// The host may add messages before the input, so they come first. A parked
    /// session still owes a tool result, and the messages must not come between
    /// the call and it. Returns the turn outcome when the host stopped the turn.
    pub(crate) async fn append_input(
        &self,
        session: &mut Session,
        input: Option<Message>,
        sender: &mpsc::Sender<EngineEvent>,
    ) -> Result<Option<TurnOutcome>> {
        let Some(message) = input else {
            return Ok(None);
        };
        // The host may add messages before the input, so they come first.
        // A parked session still owes a tool result, and the messages must
        // not come between the call and it.
        if session.phase == Phase::Idle {
            let history = self.store.history(session.head).await?;
            let consultation = self
                .consult(session, sender, Some(&history), ConsultationPoint::PreInput)
                .await?;
            *session = consultation.session;
            if consultation.stop.is_some() {
                return Ok(Some(TurnOutcome {
                    session: session.clone(),
                    message: None,
                }));
            }
        }
        *session = self.append_turn(session, message, sender).await?;
        Ok(None)
    }

    /// End-of-turn check: if the head is a final assistant message, consult the
    /// host. Returns `Some(Continue)` when the host added messages
    /// (re-evaluate), `Some(Finish)` when the turn ends, or `None` when this is
    /// not an end-of-turn situation.
    pub(crate) async fn end_of_turn_check(
        &self,
        session: &mut Session,
        sender: &mpsc::Sender<EngineEvent>,
        last: &mut Option<Message>,
    ) -> Result<Option<EndOfTurn>> {
        let head_message = match session.head {
            Some(head) => Some(self.store.node(head).await?.message),
            None => None,
        };
        if let Some(message) = &head_message
            && message.role == Role::Assistant
            && !Self::continues_turn(session, message)
        {
            let Some(outcome) = self
                .end_of_turn(session, sender, last.clone(), message)
                .await?
            else {
                return Ok(Some(EndOfTurn::Continue));
            };
            return Ok(Some(EndOfTurn::Finish(Box::new(outcome))));
        }
        Ok(None)
    }

    /// Runs one iteration: either the tool calls or a generate+commit.
    async fn run_one_iteration(
        &self,
        session: &mut Session,
        last: &mut Option<Message>,
        early_results: &mut Vec<ToolResultPart>,
        effective: &TurnOptions,
        sender: &mpsc::Sender<EngineEvent>,
        cancellation: &CancellationToken,
    ) -> Result<Step> {
        match session.phase {
            Phase::AwaitingTool => {
                *session = self
                    .run_tools(session, sender, cancellation, early_results)
                    .await?;
                early_results.clear();
                // The host may add messages after the tools ran, before the next
                // request is built: a tool result the host wants to annotate,
                // or a decision to stop the turn now.
                let history = self.store.history(session.head).await?;
                let consultation = self
                    .consult(
                        session,
                        sender,
                        Some(&history),
                        ConsultationPoint::PostTools,
                    )
                    .await?;
                *session = consultation.session;
                if consultation.stop.is_some() {
                    return Ok(Step::Done(Box::new(TurnOutcome {
                        session: session.clone(),
                        message: last.clone(),
                    })));
                }
                if cancellation.is_cancelled() {
                    return Ok(Step::Done(Box::new(TurnOutcome {
                        session: session.clone(),
                        message: last.clone(),
                    })));
                }
                Ok(Step::Tools)
            }
            Phase::Idle | Phase::Streaming => {
                // The host may add messages before this request is built.
                // The history is read once and the request reuses it.
                let Some((next, head, history)) = self.prepare_request(session, sender).await?
                else {
                    return Ok(Step::Done(Box::new(TurnOutcome {
                        session: session.clone(),
                        message: last.clone(),
                    })));
                };
                *session = next;
                let (message, interrupted, pending) = self
                    .generate(session, history, effective, sender, cancellation)
                    .await?;
                *early_results = pending;
                self.commit_answer(session, &message, head, interrupted, effective, sender)
                    .await?;
                *last = Some(message.clone());
                if interrupted {
                    // Tool calls in the message still need results: give
                    // them the cancellation reason so nothing is left
                    // without an answer.
                    if last.as_ref().is_some_and(|m| !m.tool_calls().is_empty()) {
                        *session = self.run_tools(session, sender, cancellation, &[]).await?;
                    }
                    return Ok(Step::Done(Box::new(TurnOutcome {
                        session: session.clone(),
                        message: last.clone(),
                    })));
                }
                Ok(Step::Generated {
                    message: Box::new(message),
                    interrupted,
                })
            }
            _ => unreachable!(),
        }
    }

    /// Asks the host interceptors how to handle an error. Returns `true` when
    /// the host asked to retry the current iteration.
    async fn handle_error(
        &self,
        session: &Session,
        sender: &mpsc::Sender<EngineEvent>,
        error: &Error,
    ) -> Result<bool> {
        if self.interceptors.is_empty() {
            return Ok(false);
        }
        let sink = sender;
        let context = InterceptionContext {
            point: ConsultationPoint::PreflightRequest,
            session: session.id,
            turn: session.turn,
            head: session.head,
            messages: &[],
            events: sink,
        };
        let mut injected = false;
        for interceptor in self.interceptors.interceptors() {
            match interceptor.on_error(&context, error).await? {
                Interception::Continue | Interception::RefuseCall { .. } => {}
                Interception::Stop { .. } => return Ok(false),
                Interception::Inject(added) => {
                    if !added.is_empty() {
                        injected = true;
                    }
                }
            }
        }
        Ok(injected)
    }

    /// Whether the loop keeps working from the current head.
    ///
    /// The turn ends at a final assistant message. It carries on while the
    /// session is still waiting for the calls that message asked for: they run,
    /// and the model is asked again with their results.
    fn continues_turn(session: &Session, message: &Message) -> bool {
        session.phase == Phase::AwaitingTool && message.has_tool_calls()
    }

    /// Commits the answer the model produced.
    ///
    /// The phase follows the answer: an interrupted one leaves the session
    /// idle, an answer with tool calls waits for them, and a final answer ends
    /// the turn. An interrupted answer that had already streamed a call is
    /// closed here too: the error result for each unanswered call commits
    /// beside the message.
    pub(crate) async fn commit_answer(
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
        let phase = if interrupted {
            Phase::Idle
        } else if message.has_tool_calls() {
            Phase::AwaitingTool
        } else {
            Phase::Idle
        };
        // An answer with tool calls that the interruption left unrun is a call
        // no turn will ever execute: the session goes idle, not parked, so the
        // loop never reaches `run_tools` for it. Commit an error result for each
        // open call in the same commit, so the graph keeps the invariant a
        // `tool_call` is answered by its `tool_result` — the one a replay across
        // models and the store's own history depend on.
        let mut nodes = vec![node];
        if interrupted {
            let unanswered = unanswered_call_errors(message);
            if !unanswered.is_empty() {
                let parent = node_id;
                nodes.push(Node::message(Message::tool(unanswered), Some(parent)));
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
        Ok(())
    }

    /// Handles the end of a turn: the host gets the last word.
    ///
    /// A message the host was holding keeps the turn alive, so the turn ends
    /// only when the host has nothing left to say.
    pub(crate) async fn end_of_turn(
        &self,
        session: &mut Session,
        sender: &mpsc::Sender<EngineEvent>,
        last: Option<Message>,
        head_message: &Message,
    ) -> Result<Option<TurnOutcome>> {
        let history = self.store.history(session.head).await?;
        let consultation = self
            .consult(
                session,
                sender,
                Some(&history),
                ConsultationPoint::EndOfTurn,
            )
            .await?;
        let added = consultation.session.head != session.head;
        *session = consultation.session;
        if added {
            return Ok(None);
        }
        Ok(Some(TurnOutcome {
            session: session.clone(),
            message: last.or_else(|| Some(head_message.clone())),
        }))
    }

    /// Prepares the next request.
    ///
    /// The host may add messages to the session first. An interceptor reads
    /// the history the request carries, so the store is read once and the
    /// request reuses it.
    pub(crate) async fn prepare_request(
        &self,
        session: &Session,
        sender: &mpsc::Sender<EngineEvent>,
    ) -> Result<Option<(Session, NodeId, Vec<Message>)>> {
        let before = session.head;
        let history = if self.interceptors.is_empty() {
            None
        } else {
            Some(self.store.history(before).await?)
        };
        let consultation = self
            .consult(
                session,
                sender,
                history.as_deref(),
                ConsultationPoint::PreflightRequest,
            )
            .await?;
        let session = consultation.session;
        if consultation.stop.is_some() {
            return Ok(None);
        }
        let Some(head) = session.head else {
            return Ok(None);
        };
        let history = match history {
            Some(previous) if session.head == before => previous,
            _ => self.store.history(Some(head)).await?,
        };
        Ok(Some((session, head, history)))
    }

    /// Asks the host interceptors what the turn does next, and applies the
    /// messages they add.
    ///
    /// The engine is the only writer while a turn runs, so the commit carries
    /// the version the turn already holds: an interceptor never races the
    /// turn, and what it adds lands in the graph, in the request digest and in
    /// the history the policies read.
    #[allow(private_interfaces)]
    pub(crate) async fn consult(
        &self,
        session: &Session,
        sender: &mpsc::Sender<EngineEvent>,
        history: Option<&[Message]>,
        point: ConsultationPoint,
    ) -> Result<Consultation> {
        if self.interceptors.is_empty() {
            return Ok(Consultation::unchanged(session.clone()));
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
        for interceptor in self.interceptors.interceptors() {
            match interceptor.intercept(&context).await? {
                // A refusal is a decision about one call, and `consult` is not
                // asked about one: the engine refuses a call through
                // `Harness::refuse_call`, where the call is in the context.
                Interception::Continue | Interception::RefuseCall { .. } => {}
                Interception::Inject(added) => messages.extend(added),
                // `Interception` is `#[non_exhaustive]`: a decision this build
                // does not know is not a licence to run the turn as if the host
                // had said nothing, so it stops like an explicit `Stop`.
                Interception::Stop { reason } => {
                    // The host ended the turn on purpose; say so, so the client
                    // does not read it as a failure.
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
            }
        }
        if messages.is_empty() {
            return Ok(Consultation {
                session: session.clone(),
                stop,
            });
        }
        let count = messages.len();
        let mut parent = session.head;
        let mut nodes = Vec::with_capacity(count);
        for message in messages {
            let node = Node::message(message, parent);
            parent = Some(node.id);
            nodes.push(node);
        }
        let committed = self
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
        Ok(Consultation {
            session: committed,
            stop,
        })
    }

    /// Chains the turn's input onto the head.
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
        let nodes = vec![node];

        let previous_phase = session.phase;
        let committed = self
            .commit(
                Commit {
                    session_id: session.id,
                    expected_version: Some(session.version),
                    nodes,
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
