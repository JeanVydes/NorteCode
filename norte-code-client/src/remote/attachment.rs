//! Keeping a session attached while the client talks to the host.
//!
//! The host detaches a session when its event stream closes, and it takes
//! that attachment away from any other client that asks for it. The client
//! that sent the last request is the one that learns about it, and only when
//! the next request is refused, so that is where the attachment is taken
//! again and the request is sent once more.

use std::future::Future;

use super::RemoteClient;
use crate::error::ClientError;

impl RemoteClient {
    /// Forgets that a session is attached.
    ///
    /// The host is the one that decides whether a session is attached, so the
    /// client drops its belief as soon as the host says otherwise.
    pub(crate) fn forget_attachment(&self, session_id: &str) {
        self.attached_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
    }

    /// Sends a request that needs the session attached, and recovers from a
    /// session that was detached in the meantime.
    ///
    /// The request is sent at most twice: once, and once more after the
    /// session was attached again.
    pub(crate) async fn with_attachment<F, Fut, T>(
        &self,
        session: &str,
        request: F,
    ) -> Result<T, ClientError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, ClientError>>,
    {
        match request().await {
            Err(error) if error.is_session_not_attached() => {
                self.forget_attachment(session);
                self.ensure_attached(session).await?;
                request().await
            }
            other => other,
        }
    }
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;
