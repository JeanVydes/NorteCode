use norte_code_wire::ClientRequest;
use raep_types::{Envelope, FrameType, StandardFrameType};
use reqwest::header::{AUTHORIZATION, HeaderMap};
use tokio_stream::iter;

use super::RemoteClient;
use crate::agent::EventStream;
use crate::decode::{server_event_from_frame, session_from_request, timestamp_millis};
use crate::error::ClientError;
use crate::frame::CommandResponseBody;

impl RemoteClient {
    pub(crate) async fn submit_configure(
        &self,
        session: &str,
        agent_override: serde_json::Value,
    ) -> Result<EventStream, ClientError> {
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::SessionConfigure),
                serde_json::to_value(serde_json::json!({
                    "agent_override": agent_override,
                    "apply": "now",
                }))
                .map_err(|error| ClientError::new(format!("serialize command: {error}")))?,
            )
            .await?;
        let events = frames
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    pub(crate) async fn submit_http(
        &self,
        request: ClientRequest,
    ) -> Result<EventStream, ClientError> {
        if let norte_code_wire::ClientRequestKind::SetReasoning {
            session,
            scope: norte_code_wire::ReasoningScope::Session,
            reasoning,
        } = &request.kind
        {
            self.session_reasoning
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(session.as_str().to_owned(), reasoning.clone());
        }
        if let Some(session) = session_from_request(&request) {
            self.ensure_attached(session.as_str()).await?;
            if matches!(
                &request.kind,
                norte_code_wire::ClientRequestKind::SendMessage { .. }
            ) {
                return self.submit_streaming(session.as_str(), request).await;
            }
        }
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(self.command_url(&self.session_for(&request)))
            .headers(headers)
            .json(&Envelope::new(
                FrameType::Extension("norte_code.command".to_owned()),
                serde_json::to_value(request)
                    .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
                timestamp_millis(),
            ))
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP request failed: {error}")))?;
        let status = response.status();
        let response_body: CommandResponseBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP response: {error}")))?;
        if !status.is_success() {
            return Err(ClientError::new(response_body.error_message()));
        }
        let events = response_body
            .events
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        for frame in &response_body.events {
            self.note_frame_seq(frame);
        }
        self.remember_sessions(&events);
        Ok(Box::pin(iter(events)))
    }
}
