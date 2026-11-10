//! One group of endpoints the client exposes.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl ApiClient {
    pub async fn sessions(&self) -> Result<Vec<SessionSummary>, ApiError> {
        let response: SessionListResponse = self.get_json("raep/v1/sessions").await?;
        Ok(response.sessions)
    }

    pub async fn child_sessions(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<ChildSessionSummary>, ApiError> {
        let url = self.session_url(parent_session_id, "children")?;
        let response: ChildSessionListResponse =
            self.send_json(self.auth(self.http.get(url))).await?;
        Ok(response.children)
    }

    pub async fn create_session(
        &self,
        agent_id: &str,
        title: Option<&str>,
    ) -> Result<SessionSummary, ApiError> {
        self.create_session_with_model(agent_id, title, None).await
    }

    /// Creates a session pinned to one provider/model from its first turn.
    pub async fn create_session_with_model(
        &self,
        agent_id: &str,
        title: Option<&str>,
        model: Option<(&str, &str)>,
    ) -> Result<SessionSummary, ApiError> {
        let agent_override = model.map(|(provider, model)| AgentOverride {
            model: AgentModelRef { id: model },
            provider,
        });
        let url = self.resource_url("raep/v1/sessions")?;
        self.send_json(self.auth(self.http.post(url).json(&CreateSessionRequest {
            agent_id,
            title,
            agent_override,
        })))
        .await
    }

    pub async fn rename_session(&self, session_id: &str, title: &str) -> Result<(), ApiError> {
        let url = self.session_resource_url(session_id)?;
        self.send_status(self.auth(self.http.patch(url).json(&RenameSessionRequest { title })))
            .await
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<(), ApiError> {
        let url = self.session_resource_url(session_id)?;
        self.send_status(self.auth(self.http.delete(url))).await
    }

    /// Lists the session's plans.
    pub async fn plans(&self, session_id: &str) -> Result<Vec<PlanSummary>, ApiError> {
        let response = self
            .norte_code_command(
                session_id,
                ClientRequestKind::ListPlans {
                    session: SessionKey::new(session_id),
                },
            )
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::plan_list().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("The plan list was not returned.".to_owned())
            })?;
        Ok(decode_payload::<PlanListResponse>(&payload.payload)?.entries)
    }

    pub async fn history(&self, session_id: &str) -> Result<Vec<ConversationItem>, ApiError> {
        let response = self
            .post_frame(
                session_id,
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
        let page: HistoryPage = decode_payload(&page.payload)?;
        Ok(page
            .items
            .into_iter()
            .enumerate()
            .map(|(index, item)| ConversationItem {
                id: format!(
                    "{}:{}:{index}",
                    item.turn_id.as_deref().unwrap_or("unassigned"),
                    item.timestamp
                ),
                turn_id: item.turn_id,
                role: item.role,
                kind: item.kind,
                content: item.content,
                created_at: item.timestamp,
                delivery: MessageDelivery::Sent,
                tool: None,
                usage: None,
            })
            .collect())
    }

    /// The session's background jobs, as the host retains them.
    pub async fn list_jobs(&self, session_id: &str) -> Result<Vec<JobSummary>, ApiError> {
        #[derive(Serialize)]
        struct JobListRequest {
            status: Option<Vec<&'static str>>,
            limit: u64,
        }
        #[derive(Deserialize)]
        struct JobListResponse {
            #[serde(default)]
            jobs: Vec<JobSummary>,
        }
        let response = self
            .post_frame(
                session_id,
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
        let result: JobListResponse = decode_payload(&result.payload)?;
        Ok(result.jobs)
    }

    /// Asks the host to package the session and returns the export id.
    pub async fn export_session(&self, session_id: &str) -> Result<String, ApiError> {
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionExport),
            serde_json::to_value(ExportRequest::default())
                .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            now_millis()?,
        );
        frame.session_id = session_id.to_owned();
        let response = self.post_command(session_id, &frame).await?;
        if !response.accepted {
            return Err(ApiError::InvalidResponse(response.error.map_or_else(
                || "The export was refused.".to_owned(),
                |error| error.message,
            )));
        }
        response
            .events
            .iter()
            .find_map(|event| event.payload["export_id"].as_str().map(str::to_owned))
            .ok_or_else(|| ApiError::InvalidResponse("The export id was not returned.".to_owned()))
    }

    /// The URL the export's bundle downloads from.
    pub fn export_url(&self, export_id: &str) -> Result<String, ApiError> {
        Ok(self
            .resource_url(&format!("raep/v1/exports/{export_id}"))?
            .to_string())
    }
}
