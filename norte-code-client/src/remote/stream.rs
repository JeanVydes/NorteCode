use futures_util::stream::{self, StreamExt};
use norte_code_wire::QueueMode;
use norte_code_wire::{ClientRequest, ServerEvent};
use raep_types::{Ack, Envelope, FrameType, StandardFrameType, UserInput};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio_stream::iter;

use super::RemoteClient;
use crate::agent::EventStream;
use crate::decode::{percent_encode, server_event_from_frame, timestamp_millis};
use crate::error::ClientError;
use crate::frame::CommandResponseBody;
use crate::sse::{SeqStep, drain_sse_buffer, seq_step};

impl RemoteClient {
    pub(crate) fn events_url(&self, session_id: &str) -> String {
        format!(
            "{}/raep/v1/sessions/{}/events",
            self.base_url,
            percent_encode(session_id)
        )
    }

    /// RAEP `QueuePolicy` for a [`QueueMode`]: the two enums spell the
    /// same three values, so the mapping is exhaustive and cannot fall
    /// through to a default.
    pub(crate) fn raep_queue_policy(mode: QueueMode) -> raep_types::QueuePolicy {
        match mode {
            QueueMode::Reject => raep_types::QueuePolicy::Reject,
            QueueMode::Queue => raep_types::QueuePolicy::Queue,
            QueueMode::Interrupt => raep_types::QueuePolicy::Interrupt,
        }
    }

    /// Submits one `UserInput` frame and maps its immediate reply onto the
    /// `NorteCode` event stream.
    pub(crate) async fn post_user_input(
        &self,
        session: &str,
        text: &str,
        queue_policy: raep_types::QueuePolicy,
    ) -> Result<EventStream, ClientError> {
        let input = UserInput {
            prompt: Some(text.to_owned()),
            queue_policy: Some(queue_policy),
            // Caller metadata (RFC 12.4): opaque to the model, stored with
            // the turn. The CLI sends only which client it is.
            metadata: Some(serde_json::json!({ "client_id": "cli" })),
            ..UserInput::default()
        };
        let frames = self
            .post_standard(
                session,
                FrameType::Standard(StandardFrameType::UserInput),
                serde_json::to_value(input)
                    .map_err(|error| ClientError::new(format!("serialize prompt: {error}")))?,
            )
            .await?;
        let events = frames
            .iter()
            .map(server_event_from_frame)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::pin(iter(events)))
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn submit_streaming(
        &self,
        session_id: &str,
        request: ClientRequest,
    ) -> Result<EventStream, ClientError> {
        let mut headers = HeaderMap::new();
        // The host answers 406 to a stream request that does not ask for one.
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        if let Some(token) = &self.bearer_token {
            headers.insert(AUTHORIZATION, token.clone());
        }
        if let Some(seq) = self.last_seq(session_id) {
            headers.insert(
                HeaderName::from_static("last-event-id"),
                HeaderValue::from_str(&seq.to_string())
                    .map_err(|error| ClientError::new(format!("invalid cursor: {error}")))?,
            );
        }
        let response = self
            .http
            .get(self.events_url(session_id))
            .headers(headers.clone())
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP event stream failed: {error}")))?;
        if !response.status().is_success() {
            return Err(ClientError::new(format!(
                "RAEP event stream returned HTTP {}",
                response.status()
            )));
        }

        let command_url = self.command_url(session_id);
        let mut command_headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            command_headers.insert(AUTHORIZATION, token.clone());
        }
        let command_body = Envelope::new(
            FrameType::Extension("norte_code.command".to_owned()),
            serde_json::to_value(request)
                .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
            timestamp_millis(),
        );
        let command_result: Arc<Mutex<Option<Result<(), ClientError>>>> =
            Arc::new(Mutex::new(None));
        let command_result_task = Arc::clone(&command_result);
        let http_for_command = self.http.clone();
        let url_for_command = command_url.clone();
        let headers_for_command = command_headers.clone();
        tokio::spawn(async move {
            let outcome = async {
                let response = http_for_command
                    .post(url_for_command)
                    .headers(headers_for_command)
                    .json(&command_body)
                    .send()
                    .await
                    .map_err(|error| ClientError::new(format!("RAEP command failed: {error}")))?;
                if !response.status().is_success() {
                    let status = response.status();
                    let body: CommandResponseBody =
                        response.json().await.unwrap_or(CommandResponseBody {
                            events: Vec::new(),
                            error: None,
                        });
                    return Err(ClientError::new(format!(
                        "RAEP command returned HTTP {status}: {}",
                        body.error_message()
                    )));
                }
                Ok(())
            }
            .await;
            *command_result_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
        });

        let session = norte_code_wire::SessionKey::new(session_id);
        let session_seq = Arc::clone(&self.session_seq);
        let ack_http = self.http.clone();
        let ack_url = self.command_url(session_id);
        let ack_token = self.bearer_token.clone();
        let bytes = response.bytes_stream();
        let stream = stream::unfold(
            (
                bytes,
                Vec::new(),
                VecDeque::<(ServerEvent, bool)>::new(),
                session,
                false,
                session_seq,
                command_result,
                false,
            ),
            move |(
                mut bytes,
                mut buffer,
                mut pending,
                session,
                mut finished,
                session_seq,
                command_result,
                mut command_reported,
            )| {
                let ack_http = ack_http.clone();
                let ack_url = ack_url.clone();
                let ack_token = ack_token.clone();
                async move {
                    if !command_reported {
                        let result = command_result
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone();
                        if let Some(Err(error)) = result {
                            command_reported = true;
                            finished = true;
                            pending.push_back((
                                ServerEvent {
                                    request_id: 0,
                                    session: Some(session.clone()),
                                    kind: norte_code_wire::ServerEventKind::Error {
                                        code: norte_code_wire::ErrorCode::Internal,
                                        message: error.to_string(),
                                    },
                                },
                                true,
                            ));
                        } else if result.is_some() {
                            command_reported = true;
                        }
                    }
                    loop {
                        if let Some((event, terminal)) = pending.pop_front() {
                            return Some((
                                event,
                                (
                                    bytes,
                                    buffer,
                                    pending,
                                    session,
                                    finished || terminal,
                                    session_seq,
                                    command_result,
                                    command_reported,
                                ),
                            ));
                        }
                        if finished {
                            return None;
                        }
                        let drained = drain_sse_buffer(&mut buffer);
                        let mut last_applied = None;
                        for (event, terminal, seq) in drained {
                            if let Some(seq) = seq {
                                let mut seqs = session_seq
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                match seq_step(seqs.get(session.as_str()).copied(), seq) {
                                    SeqStep::Duplicate => {
                                        // Still advance terminal bookkeeping? no.
                                        if event.is_none() {
                                            // undecodable frame: skip yielding
                                        }
                                        continue;
                                    }
                                    SeqStep::Gap { expected } => {
                                        drop(seqs);
                                        finished = true;
                                        pending.push_back((
                                            ServerEvent {
                                                request_id: 0,
                                                session: Some(session.clone()),
                                                kind: norte_code_wire::ServerEventKind::Error {
                                                    code: norte_code_wire::ErrorCode::Internal,
                                                    message: format!(
                                                        "RAEP sequence gap: expected {expected}, received {seq}; reconnecting from {}",
                                                        expected.saturating_sub(1)
                                                    ),
                                                },
                                            },
                                            true,
                                        ));
                                        break;
                                    }
                                    SeqStep::Apply => {}
                                }
                                seqs.insert(session.as_str().to_owned(), seq);
                                drop(seqs);
                                last_applied = Some(seq);
                            }
                            if let Some(event) = event {
                                pending.push_back((event, terminal));
                            }
                        }
                        if let Some(seq) = last_applied {
                            let mut headers = HeaderMap::new();
                            if let Some(token) = &ack_token {
                                headers.insert(AUTHORIZATION, token.clone());
                            }
                            let http = ack_http.clone();
                            let url = ack_url.clone();
                            tokio::spawn(async move {
                                let _ = http
                                    .post(url)
                                    .headers(headers)
                                    .json(&Envelope::new(
                                        FrameType::Standard(StandardFrameType::Ack),
                                        serde_json::to_value(Ack { last_seq: seq })
                                            .unwrap_or(Value::Null),
                                        timestamp_millis(),
                                    ))
                                    .send()
                                    .await;
                            });
                        }
                        if finished {
                            continue;
                        }
                        #[allow(clippy::single_match_else)]
                        match bytes.next().await {
                            Some(Ok(chunk)) => {
                                buffer.extend_from_slice(&chunk);
                            }
                            Some(Err(_)) | None => {
                                if !command_reported {
                                    let result = command_result
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .clone();
                                    if let Some(Err(error)) = result {
                                        command_reported = true;
                                        finished = true;
                                        pending.push_back((
                                            ServerEvent {
                                                request_id: 0,
                                                session: Some(session.clone()),
                                                kind: norte_code_wire::ServerEventKind::Error {
                                                    code: norte_code_wire::ErrorCode::Internal,
                                                    message: error.to_string(),
                                                },
                                            },
                                            true,
                                        ));
                                    } else if result.is_some() {
                                        command_reported = true;
                                    }
                                }
                                if pending.is_empty() {
                                    return None;
                                }
                            }
                        }
                    }
                }
            },
        );
        Ok(Box::pin(stream))
    }
}
