//! One of the service's domains, as `impl Service` blocks.
//!
//! The parent's imports are in scope through `super`, so the domain
//! names types the same way the service does.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    pub(super) fn lock_cancels(&self) -> MutexGuard<'_, BTreeMap<SessionKey, CancelRegistration>> {
        self.cancels.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub(super) fn register_cancel(&self, key: SessionKey, token: CancellationToken) -> u64 {
        let generation = self.next_cancel_generation.fetch_add(1, Ordering::Relaxed);
        self.lock_cancels()
            .insert(key, CancelRegistration { generation, token });
        generation
    }
    pub(super) fn remove_cancel(&self, key: &SessionKey, generation: u64) {
        let mut cancels = self.lock_cancels();
        if cancels
            .get(key)
            .is_some_and(|registration| registration.generation == generation)
        {
            cancels.remove(key);
        }
    }
    /// Cancels the turn for `session`, if one is running. Returns whether a
    /// token was found and signalled.
    ///
    /// The token alone is not enough: the engine can be parked inside a tool
    /// that awaits a client (an unanswered `ask_user`), and then it never
    /// emits the terminal event: the session stayed busy for good. A watchdog
    /// closes the turn if the engine has not done so a few seconds after the
    /// cancel.
    pub fn cancel_turn(&self, session: &SessionKey) -> bool {
        let cancelled = {
            let cancels = self.lock_cancels();
            match cancels.get(session) {
                Some(registration) => {
                    registration.token.cancel();
                    true
                }
                None => false,
            }
        };
        // A question outlives the turn's token: the tool waits on a oneshot
        // that only closes when it is answered or dropped, so a cancel that
        // left it pending kept the turn open and the session busy for good.
        if let Ok((nid, _, _)) = self.require_record(session) {
            self.questions.abandon_session(Some(nid));
        }
        cancelled
    }
    /// Cancels a turn and closes it if the engine does not.
    ///
    /// The token alone is not enough: the engine can be parked inside a tool
    /// that awaits a client (an unanswered `ask_user`), and then it never
    /// emits the terminal event. This is the form callers with an `Arc` use.
    pub fn cancel_turn_and_watch(self: &Arc<Self>, session: &SessionKey) -> bool {
        let cancelled = self.cancel_turn(session);
        if cancelled && let Some(turn) = self.active_turn(session) {
            let service = Arc::clone(self);
            let key = session.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(CANCEL_CLOSE_GRACE)).await;
                service.close_cancelled_turn(&key, turn).await;
            });
        }
        cancelled
    }
    /// Closes a turn the engine did not close after its cancellation.
    ///
    /// The engine owns the turn and normally reports its end; when it cannot -
    /// it is parked in a tool that never returns: the service is the only one
    /// left that can put the session back to `Idle` and tell the client.
    pub(crate) async fn close_cancelled_turn(&self, key: &SessionKey, turn: u64) {
        self.clear_active_turn(key, turn);
        let Ok((_, _, phase)) = self.require_record(key) else {
            return;
        };
        if phase == norte_code_wire::Phase::Idle && self.active_turn(key).is_none() {
            return;
        }
        self.set_phase(key, norte_code_wire::Phase::Idle).await;
        self.emit_delegation(ServerEvent::new(
            0,
            Some(key.clone()),
            ServerEventKind::TurnStopped {
                reason: STOP_REASON_CANCELLED.to_owned(),
            },
        ));
        self.emit_delegation(ServerEvent::new(
            0,
            Some(key.clone()),
            ServerEventKind::TurnFinished {
                turn,
                phase: norte_code_wire::Phase::Idle,
                committed: false,
                output_schema_valid: None,
            },
        ));
    }
    pub(crate) fn active_turn(&self, key: &SessionKey) -> Option<u64> {
        self.active_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .copied()
    }
}
