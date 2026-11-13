//! The session's background jobs: listing them and stopping one.
//!
//! A job is a tool call the host let run on its own. The host lists them and
//! stops them through the standard frames, so they ride the session's
//! attachment the same way every other session command does.

use raep_types::{FrameType, StandardFrameType};

use norte_code_wire::{JobInfo, ServerEventKind};

use super::RemoteClient;
use crate::decode::server_event_from_frame;
use crate::error::ClientError;

/// A `JOB_LIST` body.
#[derive(serde::Serialize)]
struct JobListRequest {
    status: Option<Vec<&'static str>>,
    limit: u64,
}

/// A `CANCEL_JOB` body.
#[derive(serde::Serialize)]
struct CancelJobRequest<'a> {
    job_id: &'a str,
    grace_ms: u64,
}

impl RemoteClient {
    /// Lists the session's background jobs.
    pub(crate) async fn jobs_of(&self, session: &str) -> Result<Vec<JobInfo>, ClientError> {
        let payload = serde_json::to_value(JobListRequest {
            status: None,
            limit: 100,
        })
        .map_err(|error| ClientError::new(format!("serialize job list: {error}")))?;
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::JobList),
                payload,
            )
            .await?;
        let events = frames
            .iter()
            .filter_map(|frame| server_event_from_frame(frame).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(events
            .into_iter()
            .find_map(|event| match event.kind {
                ServerEventKind::JobList { jobs } => Some(jobs),
                _ => None,
            })
            .unwrap_or_default())
    }

    /// Stops one background job and reports how it ended.
    pub(crate) async fn stop(&self, session: &str, job_id: &str) -> Result<JobInfo, ClientError> {
        let payload = serde_json::to_value(CancelJobRequest {
            job_id,
            grace_ms: 5_000,
        })
        .map_err(|error| ClientError::new(format!("serialize job stop: {error}")))?;
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::CancelJob),
                payload,
            )
            .await?;
        let events = frames
            .iter()
            .filter_map(|frame| server_event_from_frame(frame).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        events
            .into_iter()
            .find_map(|event| match event.kind {
                ServerEventKind::JobFinished { job, .. } => Some(job),
                _ => None,
            })
            .ok_or_else(|| {
                ClientError::new("The host did not report how the job ended.".to_owned())
            })
    }
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
