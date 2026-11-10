//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl NorteCodeRaepHost {
    /// Cancels one job owned by this session.
    pub(crate) async fn cancel_job(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        if frame
            .payload
            .get("grace_ms")
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(HostError::InvalidFrame(
                "CANCEL_JOB.grace_ms must be a non-negative integer".to_owned(),
            ));
        }
        let job_id = frame
            .job_id
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| {
                frame
                    .payload
                    .get("job_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .ok_or_else(|| HostError::InvalidFrame("CANCEL_JOB.job_id is required".to_owned()))?;
        let grace = frame
            .payload
            .get("grace_ms")
            .and_then(Value::as_u64)
            .map_or_else(
                || std::time::Duration::from_secs(5),
                std::time::Duration::from_millis,
            );
        let session = SessionKey::new(frame.session_id.clone());
        let service = self.service_for_session(&frame.session_id).await?;
        let job = service
            .stop_session_job_with_grace(&session, &job_id, Some(grace))
            .await
            .ok_or_else(|| HostError::InvalidFrame("JOB_NOT_FOUND".to_owned()))?;
        let triggered_new_turn = service
            .session_job_triggered_new_turn(&session, &job_id)
            .unwrap_or(false);
        let new_turn_id = if triggered_new_turn {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                service.session_job_new_turn_id(&session, &job_id),
            )
            .await
            .ok()
            .flatten()
            .map(|turn| turn.to_string())
        } else {
            None
        };
        let active_turn_id = service
            .session_active_turn_id(&session)
            .map(|turn| turn.to_string());
        let injected_into_turn_id = (!triggered_new_turn)
            .then_some(active_turn_id.as_deref())
            .flatten();
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::JobFinished,
                job_finished_payload(
                    &job,
                    triggered_new_turn,
                    new_turn_id.as_deref(),
                    injected_into_turn_id,
                ),
            )
            .await?,
        ])
    }
    /// Job listing (RFC 14.3): `JOB_LIST` returns `JOB_LIST_RESULT`.
    pub(crate) async fn job_list(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: JobListRequest =
            serde_json::from_value(frame.payload.clone()).map_err(|error| {
                HostError::InvalidFrame(format!("JOB_LIST payload is invalid: {error}"))
            })?;
        let limit = usize::try_from(request.limit.unwrap_or(100))
            .map_err(|_| HostError::InvalidFrame("JOB_LIST.limit is out of range".to_owned()))?;
        if !(1..=100).contains(&limit) {
            return Err(HostError::InvalidFrame(
                "JOB_LIST.limit must be between 1 and 100".to_owned(),
            ));
        }
        let mut frames = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::ListJobs {
                    session: SessionKey::new(frame.session_id.clone()),
                },
            )
            .await?;
        if let Some(result) = frames
            .iter_mut()
            .find(|result| result.kind == standard(StandardFrameType::JobListResult))
        {
            let mut jobs: Vec<JobListEntry> =
                serde_json::from_value(result.payload["jobs"].clone()).map_err(|error| {
                    HostError::InvalidFrame(format!("JOB_LIST_RESULT payload is invalid: {error}"))
                })?;
            if let Some(statuses) = request.status {
                jobs.retain(|job| statuses.contains(&job.status));
            }
            jobs.truncate(limit);
            result.payload = payload(JobListResultPayload { jobs });
        }
        Ok(frames)
    }
    /// Job state query (RFC 14.3): `JOB_GET` returns `JOB_STATE`.
    ///
    /// The retained output lives in the session's `JobList` summaries (the
    /// registry's `JobInfo` carries the most recent output) and in the
    /// replay window (`ToolOutput`/`ToolFinished` frames). The host
    /// answers from those: `from_index` skips that many leading frames;
    /// running jobs return their current output, finished jobs their
    /// latest output, and unknown jobs `JOB_NOT_FOUND`.
    pub(crate) async fn job_get(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let job_id = frame
            .job_id
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| {
                frame
                    .payload
                    .get("job_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .ok_or_else(|| HostError::InvalidFrame("JOB_NOT_FOUND".to_owned()))?;
        if frame
            .payload
            .get("from_index")
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(HostError::InvalidFrame(
                "JOB_GET.from_index must be a non-negative integer".to_owned(),
            ));
        }
        let from_index = frame
            .payload
            .get("from_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let service = self.service_for_session(&frame.session_id).await?;
        let job = service
            .session_job_with_output_retention(&SessionKey::new(frame.session_id.clone()), &job_id)
            .ok_or_else(|| HostError::InvalidFrame("JOB_NOT_FOUND".to_owned()))?;
        let output = job_output_from_index(&job, from_index)?;
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::JobState,
                payload(JobStatePayload {
                    job_id: &job_id,
                    status: job_status(&job),
                    output,
                    next_index: job.output_bytes_total,
                }),
            )
            .await?,
        ])
    }
}
