use norte_code_wire::{CanonicalEvent, ClientRequestKind, SessionKey};
use raep_types::{FrameType, StandardFrameType};

use super::shared::{accepted, decode_payload, find_event};
use super::types::{
    ExportRequest, HistoryGetRequest, JobListRequest, JobListResponse, PlanListResponse,
};
use super::{ApiError, Endpoint, RestClient};

impl RestClient {
    /// The session's plans.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn plans(
        &self,
        session: &str,
    ) -> Result<Vec<norte_code_wire::PlanSummary>, ApiError> {
        let response = self
            .command(
                Some(session),
                ClientRequestKind::ListPlans {
                    session: SessionKey::new(session),
                },
            )
            .await?;
        let payload = find_event(&response, CanonicalEvent::plan_list())?;
        Ok(decode_payload::<PlanListResponse>(&payload)?.entries)
    }

    /// One page of a session's history.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn history(
        &self,
        session: &str,
    ) -> Result<Vec<norte_code_wire::HistoryItem>, ApiError> {
        let response = self
            .post_frame(
                session,
                FrameType::Standard(StandardFrameType::HistoryGet),
                &HistoryGetRequest {
                    before_turn_id: None,
                    limit: 100,
                    include_rewound: false,
                },
            )
            .await?;
        let page = response
            .events
            .iter()
            .find(|frame| frame.kind == FrameType::Standard(StandardFrameType::HistoryPage))
            .ok_or_else(|| {
                ApiError::InvalidResponse("History page was not returned.".to_owned())
            })?;
        crate::history_items(&page.payload)
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))
    }

    /// The session's background jobs.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn list_jobs(
        &self,
        session: &str,
    ) -> Result<Vec<norte_code_wire::JobSummary>, ApiError> {
        let response = self
            .post_frame(
                session,
                FrameType::Standard(StandardFrameType::JobList),
                &JobListRequest {
                    status: None,
                    limit: 100,
                },
            )
            .await?;
        let result = response
            .events
            .iter()
            .find(|frame| frame.kind == FrameType::Standard(StandardFrameType::JobListResult))
            .ok_or_else(|| {
                ApiError::InvalidResponse("The job list was not returned.".to_owned())
            })?;
        Ok(decode_payload::<JobListResponse>(&result.payload)?.jobs)
    }

    /// Asks the host to package the session and returns the export id.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn export_session(&self, session: &str) -> Result<String, ApiError> {
        let frame = Self::frame(
            session,
            FrameType::Standard(StandardFrameType::SessionExport),
            &ExportRequest::default(),
        )?;
        let response = self.post_command(session, &frame).await?;
        accepted(response.clone(), "The export was refused.")?;
        response
            .events
            .iter()
            .find_map(|event| event.payload["export_id"].as_str().map(str::to_owned))
            .ok_or_else(|| ApiError::InvalidResponse("The export id was not returned.".to_owned()))
    }

    /// The URL an exported bundle downloads from.
    #[must_use]
    pub fn export_url(&self, export_id: &str) -> String {
        self.url(&Endpoint::Export { id: export_id })
    }
}
