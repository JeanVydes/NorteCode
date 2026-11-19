//! The session's background jobs.

use std::sync::Arc;

use norte_code_client::AgentClient;
use norte_code_wire::SessionKey;

use super::{ClientEvent, ClientRuntime};

impl ClientRuntime {
    /// Asks the host for the session's background jobs.
    ///
    /// The live stream only carries the jobs that start while it is open, so
    /// the list is what fills in the ones that were already running.
    pub(crate) fn load_jobs(&self, session: SessionKey, generation: u64) {
        let client = Arc::clone(&self.client);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            match client.list_jobs(session.as_str()).await {
                Ok(jobs) => {
                    let _ = sender
                        .send(ClientEvent::Jobs {
                            session,
                            generation,
                            jobs,
                        })
                        .await;
                }
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    /// Stops one background job.
    pub(crate) fn stop_job(&self, session: SessionKey, job_id: String, generation: u64) {
        let client = Arc::clone(&self.client);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            match client.stop_job(session.as_str(), job_id.as_str()).await {
                Ok(job) => {
                    let _ = sender
                        .send(ClientEvent::JobStopped {
                            session,
                            generation,
                            job,
                        })
                        .await;
                }
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }
}
