//! One of the service's domains, as `impl Service` blocks.
//!
//! The parent's imports are in scope through `super`, so the domain
//! names types the same way the service does.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    /// Installs the runner the `subagent` tool asks, once the service
    /// exists as an `Arc`.
    ///
    /// `tools` is a `&self` method and cannot produce the `Arc<Self>` the
    /// runner needs; the host that builds the service through
    /// `Service::new` (which returns the service) installs one here.
    pub fn install_subagent_runner(self: &Arc<Self>) {
        *self
            .subagent_runner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::new(SubagentRunnerImpl::new(Some(Arc::clone(self)))) as _);
        *self
            .job_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(self));
    }
    /// Subscribes to the delegation events the service emits (RFC 8.2.10).
    ///
    /// The RAEP host is the only consumer: it drains this and emits the
    /// `CHILD_SESSION_STARTED/UPDATED` and `DELEGATOR_STATE_CHANGED` frames
    /// on the parent's stream. Returns the replaceable receiver: the host
    /// takes it once at startup.
    pub fn delegation_events(
        &self,
    ) -> std::sync::MutexGuard<'_, Option<tokio::sync::broadcast::Receiver<ServerEvent>>> {
        self.delegation_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
    /// Takes the delegation event receiver, once.
    ///
    /// The RAEP host calls this at startup and spawns the drain that turns
    /// the service's events into standard RAEP frames. A second call
    /// returns `None`: there is only one drain.
    #[must_use]
    pub fn delegation_rx_take(&self) -> Option<tokio::sync::broadcast::Receiver<ServerEvent>> {
        self.delegation_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
    /// Emits a delegation event to the host subscribers.
    pub(super) fn emit_delegation(&self, mut event: ServerEvent) {
        // A producer inside the engine knows a session by its Norte id; the
        // wire names it by the client's key. Sending the engine id through
        // put the frame on a session that does not exist on the wire, so the
        // host dropped it: job start and progress never reached a client,
        // while job *finish* did, because that one already carries the key.
        if let Some(session) = event.session.as_ref()
            && let Some(key) = self
                .lock_sessions()
                .iter()
                .find(|(_, record)| record.id.to_string() == session.as_str())
                .map(|(key, _)| key.clone())
        {
            event.session = Some(key);
        }
        let _ = self.delegation_events.send(event);
    }
    /// The next request id for an auto-turn.
    pub(crate) fn next_auto_request_id(&self) -> u64 {
        self.auto_request_id.fetch_add(1, Ordering::Relaxed)
    }
    /// Schedules the configured automatic turn for a completed child task.
    pub(super) fn schedule_auto_turn(self: &Arc<Self>, session: norte::SessionId) {
        if !self.config().tools.auto_turn_on_job_end {
            return;
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service.auto_turn_on_job_end(session).await;
        });
    }
    /// Reserves an auto-turn when the session policy allows this job result.
    ///
    /// Reserving synchronously lets `JOB_FINISHED` accurately report whether
    /// a new turn is queued, while preventing two job completions from
    /// scheduling duplicate turns.
    pub(super) fn reserve_job_auto_turn(
        &self,
        session: norte::SessionId,
        job: &JobInfo,
    ) -> Option<SessionKey> {
        let policy = self
            .job_turn_policies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&session)
            .copied()
            .unwrap_or_else(|| {
                if self.config().tools.auto_turn_on_job_end {
                    JobTurnPolicy::Always
                } else {
                    JobTurnPolicy::Never
                }
            });
        if !job_policy_allows_turn(policy, job) {
            return None;
        }
        let key = self.reserve_auto_turn(session)?;
        self.job_auto_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(job.id.clone(), None);
        Some(key)
    }
    /// Queues a completed job's report and emits its terminal event.
    pub(crate) fn complete_background_job(
        self: &Arc<Self>,
        session: norte::SessionId,
        steering: Arc<steering::Steering>,
        steered: steering::Steered,
        job: JobInfo,
        reserved_turn: Option<SessionKey>,
        emit_event: bool,
    ) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = service.queue_job_result(session, &steered).await {
                tracing::warn!(session = %session, %error, "could not persist job result in session history");
            }
            steering.push(session, steered);
            if let Some(key) = reserved_turn {
                service
                    .run_reserved_job_turn(key, session, job, emit_event)
                    .await;
            } else if emit_event {
                let injected_into_turn_id = service.active_turn_id(session);
                service.emit_job_finished(session, job, false, None, injected_into_turn_id);
            }
        });
    }
    pub(crate) fn active_turn_id(&self, session: norte::SessionId) -> Option<u64> {
        let key = self
            .lock_sessions()
            .iter()
            .find(|(_, record)| record.id == session)
            .map(|(key, _)| key.clone())?;
        self.active_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .copied()
    }
    pub(crate) fn set_active_turn(&self, key: SessionKey, turn: u64) {
        self.active_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, turn);
    }
    pub(crate) fn clear_active_turn(&self, key: &SessionKey, turn: u64) {
        let mut active_turns = self
            .active_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if active_turns.get(key) == Some(&turn) {
            active_turns.remove(key);
        }
    }
    pub(crate) fn emit_job_finished(
        &self,
        session_id: norte::SessionId,
        job: JobInfo,
        triggered_new_turn: bool,
        new_turn_id: Option<u64>,
        injected_into_turn_id: Option<u64>,
    ) {
        let session = self
            .lock_sessions()
            .iter()
            .find(|(_, record)| record.id == session_id)
            .map(|(key, _)| key.clone())
            .or_else(|| Some(SessionKey::new(session_id.to_string())));
        self.emit_delegation(ServerEvent {
            request_id: 0,
            session,
            kind: ServerEventKind::JobFinished {
                job,
                triggered_new_turn,
                new_turn_id,
                injected_into_turn_id,
            },
        });
    }
    pub(crate) async fn run_reserved_job_turn(
        self: &Arc<Self>,
        key: SessionKey,
        session_id: norte::SessionId,
        job: JobInfo,
        emit_event: bool,
    ) {
        let request_id = self.next_auto_request_id();
        let mut reported = false;
        match self
            .start_turn(
                request_id,
                Some(key.clone()),
                None,
                Vec::new(),
                None,
                None,
                false,
            )
            .await
        {
            Ok(mut stream) => {
                while let Some(event) = stream.next().await {
                    if !reported && let ServerEventKind::TurnStarted { turn } = &event.kind {
                        let turn = *turn;
                        self.job_auto_turns
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(job.id.clone(), Some(turn));
                        self.job_auto_turn_changed.notify_waiters();
                        if emit_event {
                            self.emit_job_finished(session_id, job.clone(), true, Some(turn), None);
                        }
                        reported = true;
                    }
                    // A turn started here has no request behind it, so nobody
                    // else reads its stream. Draining it without forwarding
                    // left the client with the job's end and then silence: the
                    // injected message, the answer and the turn's end never
                    // reached the page, which stayed on "Thinking…" until a
                    // reload read the history instead.
                    self.emit_delegation(event);
                }
            }
            Err(error) => {
                tracing::warn!(session = %key, %error, "job-triggered turn could not start");
            }
        }
        if !reported {
            self.job_auto_turns
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&job.id);
            self.job_auto_turn_changed.notify_waiters();
            if emit_event {
                self.emit_job_finished(session_id, job, false, None, None);
            }
        }
        self.auto_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
    }
    pub(crate) fn reserve_auto_turn(&self, session: norte::SessionId) -> Option<SessionKey> {
        let key = self
            .lock_sessions()
            .iter()
            .find(|(_, record)| record.id == session)
            .map(|(key, _)| key.clone())?;
        if !self
            .lock_sessions()
            .get(&key)
            .is_some_and(|record| record.phase == Phase::Idle)
        {
            return None;
        }
        self.auto_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone())
            .then_some(key)
    }
    /// Starts a turn on its own because a job ended with a result to read.
    ///
    /// The `SessionId` is the same one the tool calls got so the surface and
    /// the service, then map it back to the key and check whether it is worth
    /// starting: the flag is on, the session is idle, and no other auto-turn
    /// is running for it.
    ///
    /// `start_turn` with `input: None` only, and then drain the events -
    /// the client sees the turn happen through them.
    pub(super) async fn auto_turn_on_job_end(self: &Arc<Self>, session: norte::SessionId) {
        if !self.config().tools.auto_turn_on_job_end {
            return;
        }
        let key = {
            let sessions = self.lock_sessions();
            sessions
                .iter()
                .find(|(_, record)| record.id == session)
                .map(|(key, _)| key.clone())
        };
        let Some(key) = key else {
            return;
        };
        {
            let sessions = self.lock_sessions();
            let is_idle = sessions
                .get(&key)
                .is_some_and(|record| record.phase == Phase::Idle);
            if !is_idle {
                return;
            }
        }
        {
            let mut auto_turns = self
                .auto_turns
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !auto_turns.insert(key.clone()) {
                // An auto-turn for this session is already in progress.
                return;
            }
        }
        let request_id = self.next_auto_request_id();
        match self
            .start_turn(
                request_id,
                Some(key.clone()),
                None,
                Vec::new(),
                None,
                None,
                false,
            )
            .await
        {
            Ok(mut stream) => while stream.next().await.is_some() {},
            Err(error) => {
                tracing::warn!(session = %key, %error, "auto-turn could not start");
            }
        }
        self.auto_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key);
    }
}
