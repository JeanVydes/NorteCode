//! One of the service's domains, as `impl Service` blocks.
//!
//! Child modules see the service's private fields, so a domain owns its
//! behaviour here while the state stays in one place.

use std::sync::PoisonError;
use std::time::Duration;

use norte_code_wire::{JobTurnPolicy, SessionKey};

use jobs::job_info;

#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    /// Returns a background job only when it belongs to `session`.
    #[must_use]
    pub fn session_job(
        &self,
        session: &SessionKey,
        job_id: &str,
    ) -> Option<norte_code_wire::JobInfo> {
        let session_id = self.lock_sessions().get(session)?.id;
        self.jobs
            .get_for_session(session_id, job_id)
            .map(|job| job_info(&job))
    }
    /// Returns a session job after applying the configured output-retention window.
    #[must_use]
    pub fn session_job_with_output_retention(
        &self,
        session: &SessionKey,
        job_id: &str,
    ) -> Option<norte_code_wire::JobInfo> {
        let session_id = self.lock_sessions().get(session)?.id;
        self.jobs
            .get_for_session_with_retention(session_id, job_id)
            .map(|job| job_info(&job))
    }
    /// Sets the limits applied to detached jobs owned by this service.
    pub fn configure_job_limits(&self, max_concurrent: usize, output_retention_s: u64) {
        self.jobs
            .configure_limits(max_concurrent, output_retention_s);
    }
    /// Sets the session's policy for turns caused by detached job results.
    #[must_use]
    pub fn set_job_turn_policy(&self, session: &SessionKey, policy: JobTurnPolicy) -> bool {
        let Some(session_id) = self.lock_sessions().get(session).map(|record| record.id) else {
            return false;
        };
        self.job_turn_policies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session_id, policy);
        true
    }
    /// Whether a session's finished job reserved a new turn.
    #[must_use]
    pub fn session_job_triggered_new_turn(
        &self,
        session: &SessionKey,
        job_id: &str,
    ) -> Option<bool> {
        self.session_job(session, job_id)?;
        Some(
            self.job_auto_turns
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(job_id),
        )
    }
    /// Waits for the turn reserved by a job to start, or returns `None` if it
    /// could not be started.
    pub async fn session_job_new_turn_id(&self, session: &SessionKey, job_id: &str) -> Option<u64> {
        self.session_job(session, job_id)?;
        loop {
            let notified = self.job_auto_turn_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let state = self
                .job_auto_turns
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(job_id)
                .copied();
            match state {
                Some(Some(turn)) => return Some(turn),
                Some(None) => notified.await,
                None => return None,
            }
        }
    }
    /// Cancels a job belonging to `session` and waits for its runner to settle.
    ///
    /// The job may show as killed as soon as cancellation is requested, but the
    /// final output and duration are not stable until its streams have closed.
    pub async fn stop_session_job(
        &self,
        session: &SessionKey,
        job_id: &str,
    ) -> Option<norte_code_wire::JobInfo> {
        self.stop_session_job_with_grace(session, job_id, None)
            .await
    }
    /// Cancels a job, applying an optional grace before forced process termination.
    pub async fn stop_session_job_with_grace(
        &self,
        session: &SessionKey,
        job_id: &str,
        grace: Option<Duration>,
    ) -> Option<norte_code_wire::JobInfo> {
        let session_id = self.lock_sessions().get(session)?.id;
        let mut terminal = self.jobs.terminal_for_session(session_id, job_id)?;
        let _ = self
            .jobs
            .cancel_in_session_with_grace(session_id, job_id, grace);
        while !*terminal.borrow() {
            if terminal.changed().await.is_err() {
                break;
            }
        }
        self.session_job(session, job_id)
    }
}
