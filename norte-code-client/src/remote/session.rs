use async_trait::async_trait;
use norte_code_wire::ClientRequest;
use norte_code_wire::QueueMode;
use raep_types::{Envelope, FrameType, StandardFrameType};
use reqwest::header::{AUTHORIZATION, HeaderMap};
use serde_json::Value;
use tokio_stream::iter;

#[async_trait]
impl AgentClient for RemoteClient {
    async fn submit(&self, request: ClientRequest) -> Result<EventStream, ClientError> {
        self.submit_http(request).await
    }

    async fn send_message(
        &self,
        session: &str,
        text: &str,
        attachments: &[norte_code_wire::Attachment],
    ) -> Result<EventStream, ClientError> {
        self.submit_streaming(
            session,
            ClientRequest {
                request_id: 0,
                kind: norte_code_wire::ClientRequestKind::SendMessage {
                    session: Some(norte_code_wire::SessionKey::new(session)),
                    text: text.to_owned(),
                    attachments: attachments.to_vec(),
                    budget: None,
                    reasoning: None,
                    output: Vec::new(),
                    response_format: None,
                    metadata: None,
                    replay: false,
                },
            },
        )
        .await
    }

    async fn steer_message(
        &self,
        session: &str,
        text: &str,
        queue_mode: QueueMode,
    ) -> Result<EventStream, ClientError> {
        self.post_user_input(session, text, Self::raep_queue_policy(queue_mode))
            .await
    }

    async fn cancel_turn(&self, session: &str) -> Result<(), ClientError> {
        self.post_standard(
            session,
            FrameType::Standard(StandardFrameType::CancelTurn),
            serde_json::to_value(raep_types::CancelTurn { reason: None }).unwrap_or(Value::Null),
        )
        .await?;
        Ok(())
    }

    async fn set_model(
        &self,
        session: &str,
        provider: &str,
        model: &str,
    ) -> Result<EventStream, ClientError> {
        // `SESSION_CONFIGURE.agent_override` with the `model` fragment.
        let mut override_value = serde_json::json!({
            "model": { "id": model },
        });
        if !provider.is_empty() {
            override_value["provider"] = serde_json::Value::String(provider.to_owned());
        }
        self.submit_configure(session, override_value).await
    }

    async fn set_reasoning(
        &self,
        session: &str,
        reasoning: norte_code_wire::Reasoning,
    ) -> Result<EventStream, ClientError> {
        let override_value = Self::reasoning_override(&reasoning);
        self.submit_configure(session, override_value).await
    }

    async fn set_budget(
        &self,
        session: &str,
        budget: norte_code_wire::Budget,
    ) -> Result<EventStream, ClientError> {
        let override_value = Self::budget_override(&budget);
        self.submit_configure(session, override_value).await
    }

    async fn resume(&self, session: &str) -> Result<EventStream, ClientError> {
        // No `agent_override` equivalent: resume goes through the extension
        // command as a `ClientRequestKind::Resume`.
        let request = ClientRequest {
            request_id: 0,
            kind: norte_code_wire::ClientRequestKind::Resume {
                session: norte_code_wire::SessionKey::new(session),
                replay: true,
            },
        };
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(self.command_url(session))
            .headers(headers)
            .json(&Envelope::new(
                FrameType::Extension("norte_code.command".to_owned()),
                serde_json::to_value(&request)
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
        Ok(Box::pin(iter(events)))
    }

    async fn fork_session(&self, session: &str) -> Result<SessionSummary, ClientError> {
        // `SessionFork` has no payload struct in raep-types yet, so the
        // standard frame carries the session id it forks, in the shape the
        // spec's fork request uses.
        let events = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::SessionFork),
                serde_json::to_value(ForkPayload {
                    session_id: session.to_owned(),
                })
                .unwrap_or(Value::Null),
            )
            .await?;
        for frame in events {
            if let Some(event) = server_event_from_frame(&frame).ok()
                && let norte_code_wire::ServerEventKind::SessionForked { .. } = event.kind
                && let Some(key) = event.session
            {
                return Ok(SessionSummary {
                    session_id: key.as_str().to_owned(),
                    ..SessionSummary::default()
                });
            }
        }
        Err(ClientError::new(
            "the server did not return the forked session",
        ))
    }

    async fn export_session(&self, session: &str) -> Result<EventStream, ClientError> {
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::SessionExport),
                serde_json::json!({ "include": {} }),
            )
            .await?;
        let events = frames
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    async fn import_session(
        &self,
        session: &str,
        bundle: serde_json::Value,
    ) -> Result<EventStream, ClientError> {
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::SessionImport),
                serde_json::json!({ "bundle": bundle }),
            )
            .await?;
        let events = frames
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    async fn compact_session(&self, session: &str) -> Result<EventStream, ClientError> {
        // No `agent_override` equivalent: compact goes through the extension
        // command as `ClientRequestKind::Compact`.
        let request = ClientRequest {
            request_id: 0,
            kind: norte_code_wire::ClientRequestKind::Compact {
                session: norte_code_wire::SessionKey::new(session),
            },
        };
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(self.command_url(session))
            .headers(headers)
            .json(&Envelope::new(
                FrameType::Extension("norte_code.command".to_owned()),
                serde_json::to_value(&request)
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
        Ok(Box::pin(iter(events)))
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .get(format!("{}/raep/v1/sessions", self.base_url))
            .headers(headers)
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP list sessions failed: {error}")))?;
        let body: SessionListBody = response
            .json()
            .await
            .map_err(|error| ClientError::new(format!("invalid RAEP session list: {error}")))?;
        Ok(body.sessions)
    }

    async fn create_session(&self, title: Option<&str>) -> Result<SessionSummary, ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .post(format!("{}/raep/v1/sessions", self.base_url))
            .headers(headers)
            .json(&CreateSessionBody {
                agent_id: self.agent_id.clone(),
                title: title.map(str::to_owned),
            })
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP create session failed: {error}")))?;
        let status = response.status();
        let body: SessionSummary = response.json().await.map_err(|error| {
            ClientError::new(format!("invalid RAEP create session response: {error}"))
        })?;
        if !status.is_success() {
            return Err(ClientError::new("RAEP create session was rejected"));
        }
        Ok(body)
    }

    async fn rename_session(&self, session: &str, title: &str) -> Result<(), ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .patch(format!(
                "{}/raep/v1/sessions/{}",
                self.base_url,
                percent_encode(session)
            ))
            .headers(headers)
            .json(&RenameSessionBody {
                title: title.to_owned(),
            })
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP rename session failed: {error}")))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::new("RAEP rename session was rejected"))
        }
    }

    async fn delete_session(&self, session: &str) -> Result<(), ClientError> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        let response = self
            .http
            .delete(format!(
                "{}/raep/v1/sessions/{}",
                self.base_url,
                percent_encode(session)
            ))
            .headers(headers)
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP delete session failed: {error}")))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::new("RAEP delete session was rejected"))
        }
    }
}

use super::RemoteClient;
use crate::agent::{AgentClient, EventStream, SessionSummary};
use crate::decode::{percent_encode, server_event_from_frame, timestamp_millis};
use crate::error::ClientError;
use crate::frame::{CommandResponseBody, ForkPayload};

/// A session list body from the RAEP management resource.
#[derive(Debug, serde::Deserialize)]
struct SessionListBody {
    #[serde(default)]
    sessions: Vec<SessionSummary>,
}

/// A create-session request body.
#[derive(Debug, serde::Serialize)]
struct CreateSessionBody {
    agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

/// A rename-session request body.
#[derive(Debug, serde::Serialize)]
struct RenameSessionBody {
    title: String,
}
