use futures_util::stream::{self, StreamExt};
use norte_code_wire::{ClientRequest, ServerEvent};
use raep_types::{Ack, Envelope, FrameType, StandardFrameType, TurnId, UserInput};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use super::{RemoteClient, SessionCache};
use crate::agent::EventStream;
use crate::decode::{percent_encode, timestamp_millis};
use crate::error::ClientError;
use crate::frame::CommandResponseBody;
use crate::sse::{SeqStep, drain_sse_buffer, seq_step};

/// The outcome of the command a stream was opened for.
type CommandOutcome = Arc<Mutex<Option<Result<(), ClientError>>>>;

const MAX_PENDING_EVENTS: usize = 512;

/// Queue one event, shedding an older non-terminal event when the queue is full.
fn queue_event(pending: &mut VecDeque<(ServerEvent, bool)>, event: ServerEvent, terminal: bool) {
    if pending.len() >= MAX_PENDING_EVENTS {
        let oldest = pending
            .iter()
            .position(|(_, is_terminal)| !*is_terminal)
            .unwrap_or(0);
        pending.remove(oldest);
    }
    pending.push_back((event, terminal));
}

/// Drops the belief that a session is attached.
///
/// The host detaches the session when its event stream closes, so the client
/// stops believing in the attachment at the same moment.
fn forget_attachment(attached: &Arc<Mutex<HashSet<String>>>, session: &str) {
    attached
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session);
}

/// The outcome of the command, when it already answered.
fn command_outcome(outcome: Option<&CommandOutcome>) -> Option<Result<(), ClientError>> {
    let outcome = outcome?;
    outcome
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The pieces the event reader takes from the client.
struct EventReader {
    http: reqwest::Client,
    ack_url: String,
    ack_token: Option<HeaderValue>,
    session_seq: Arc<Mutex<SessionCache<u64>>>,
    attached_sessions: Arc<Mutex<HashSet<String>>>,
}

impl RemoteClient {
    pub(crate) fn events_url(&self, session_id: &str) -> String {
        format!(
            "{}/raep/v1/sessions/{}/events?agent_id={}",
            self.base_url,
            percent_encode(session_id),
            percent_encode(&self.agent_for_session(session_id))
        )
    }

    /// Submits one `UserInput` frame while reading its live event stream.
    pub(crate) async fn submit_user_input_streaming(
        &self,
        session: &str,
        text: &str,
        queue_policy: raep_types::QueuePolicy,
    ) -> Result<EventStream, ClientError> {
        self.ensure_attached(session).await?;
        let payload = serde_json::to_value(UserInput {
            prompt: Some(text.to_owned()),
            queue_policy: Some(queue_policy),
            // Caller metadata (RFC 12.4): opaque to the model, stored with
            // the turn. The CLI sends only which client it is.
            metadata: Some(serde_json::json!({ "client_id": "cli" })),
            ..UserInput::default()
        })
        .map_err(|error| ClientError::new(format!("serialize prompt: {error}")))?;
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::UserInput),
            payload,
            timestamp_millis(),
        );
        frame.session_id = session.to_owned();
        frame.turn_id = Some(TurnId::generate());
        self.submit_streaming_frame(session, frame).await
    }

    pub(crate) async fn submit_streaming(
        &self,
        session_id: &str,
        request: ClientRequest,
    ) -> Result<EventStream, ClientError> {
        let command_body = Envelope::new(
            FrameType::Extension("norte_code.command".to_owned()),
            serde_json::to_value(request)
                .map_err(|error| ClientError::new(format!("serialize request: {error}")))?,
            timestamp_millis(),
        );
        self.submit_streaming_frame(session_id, command_body).await
    }

    /// Opens the event stream of a session and keeps it open.
    ///
    /// The stream is the attachment: the host binds the session to it when it
    /// opens, and releases it when the stream closes. A client that keeps one
    /// stream per session therefore sees every event of that session, whether
    /// a turn is running or not.
    pub(crate) async fn open_events_of(
        &self,
        session_id: &str,
    ) -> Result<EventStream, ClientError> {
        let response = self.event_bytes(session_id).await?;
        Ok(Box::pin(EventReader::read(
            self.reader(session_id),
            session_id.to_owned(),
            response,
            None,
        )))
    }

    async fn submit_streaming_frame(
        &self,
        session_id: &str,
        command_body: Envelope<Value>,
    ) -> Result<EventStream, ClientError> {
        let response = self.event_bytes(session_id).await?;
        let command_url = self.command_url(session_id);
        let mut command_headers = HeaderMap::new();
        if let Some(token) = &self.bearer_token {
            command_headers.insert(AUTHORIZATION, token.clone());
        }
        let command_result: CommandOutcome = Arc::new(Mutex::new(None));
        let command_result_task = Arc::clone(&command_result);
        let http_for_command = self.http.clone();
        let headers_for_command = command_headers.clone();
        tokio::spawn(async move {
            let outcome = async {
                let response = http_for_command
                    .post(command_url)
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
        Ok(Box::pin(EventReader::read(
            self.reader(session_id),
            session_id.to_owned(),
            response,
            Some(command_result),
        )))
    }

    /// The pieces the reader of an open stream needs.
    fn reader(&self, session_id: &str) -> EventReader {
        EventReader {
            http: self.http.clone(),
            ack_url: self.command_url(session_id),
            ack_token: self.bearer_token.clone(),
            session_seq: Arc::clone(&self.session_seq),
            attached_sessions: Arc::clone(&self.attached_sessions),
        }
    }

    async fn event_bytes(&self, session_id: &str) -> Result<reqwest::Response, ClientError> {
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
            .headers(headers)
            .send()
            .await
            .map_err(|error| ClientError::new(format!("RAEP event stream failed: {error}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|error| {
                    ClientError::new(format!(
                        "RAEP event stream returned HTTP {status}; could not read error response: {error}"
                    ))
                })?;
            let detail = serde_json::from_str::<raep_types::Envelope<Value>>(&body)
                .ok()
                .and_then(|frame| {
                    frame
                        .payload
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or(body);
            return Err(ClientError::new(format!(
                "RAEP event stream returned HTTP {status}: {detail}"
            )));
        }
        Ok(response)
    }
}

impl EventReader {
    /// Reads the events of an open stream.
    ///
    /// The command outcome is reported as an error event when the stream was
    /// opened for one, and there is none when the stream was opened for the
    /// session alone.
    #[allow(clippy::too_many_lines)]
    fn read(
        reader: EventReader,
        session_id: String,
        response: reqwest::Response,
        command_result: Option<CommandOutcome>,
    ) -> impl futures_util::Stream<Item = ServerEvent> + Send {
        let session = norte_code_wire::SessionKey::new(session_id);
        let session_seq = reader.session_seq;
        let attached_sessions = reader.attached_sessions;
        let ack_http = reader.http;
        let ack_url = reader.ack_url;
        let ack_token = reader.ack_token;
        let bytes = response.bytes_stream();
        stream::unfold(
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
                let attached_sessions = attached_sessions.clone();
                async move {
                    if !command_reported
                        && let Some(result) = command_outcome(command_result.as_ref())
                    {
                        if let Err(error) = result {
                            command_reported = true;
                            finished = true;
                            queue_event(
                                &mut pending,
                                ServerEvent {
                                    request_id: 0,
                                    session: Some(session.clone()),
                                    kind: norte_code_wire::ServerEventKind::Error {
                                        code: norte_code_wire::ErrorCode::Internal,
                                        message: error.to_string(),
                                    },
                                },
                                true,
                            );
                        } else {
                            command_reported = true;
                        }
                    }
                    loop {
                        if let Some((event, _)) = pending.pop_front() {
                            return Some((
                                event,
                                // A finished turn still belongs to this
                                // stream. Closing here would drop the host
                                // attachment and reject the next prompt.
                                (
                                    bytes,
                                    buffer,
                                    pending,
                                    session,
                                    finished,
                                    session_seq,
                                    command_result,
                                    command_reported,
                                ),
                            ));
                        }
                        if finished {
                            forget_attachment(&attached_sessions, session.as_str());
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
                                    SeqStep::Duplicate => continue,
                                    SeqStep::Gap { expected } => {
                                        // A gap cannot always be replayed:
                                        // the host may have pruned the frame.
                                        // Keep the stream and resync on the
                                        // next frame, or the reader loses
                                        // the turn entirely.
                                        queue_event(
                                            &mut pending,
                                            ServerEvent {
                                                request_id: 0,
                                                session: Some(session.clone()),
                                                kind: norte_code_wire::ServerEventKind::Error {
                                                    code: norte_code_wire::ErrorCode::Internal,
                                                    message: format!(
                                                        "RAEP sequence gap: expected {expected}, received {seq}"
                                                    ),
                                                },
                                            },
                                            false,
                                        );
                                    }
                                    SeqStep::Apply => {}
                                }
                                seqs.insert(session.as_str().to_owned(), seq);
                                drop(seqs);
                                last_applied = Some(seq);
                            }
                            if let Some(event) = event {
                                queue_event(&mut pending, event, terminal);
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
                        // Deliver decoded frames before waiting for more
                        // bytes. An idle stream can stay quiet for a long
                        // time after a turn ends.
                        if !pending.is_empty() || finished {
                            continue;
                        }
                        #[allow(clippy::single_match_else)]
                        match bytes.next().await {
                            Some(Ok(chunk)) => {
                                buffer.extend_from_slice(&chunk);
                            }
                            Some(Err(_)) | None => {
                                if !command_reported
                                    && let Some(result) = command_outcome(command_result.as_ref())
                                {
                                    if let Err(error) = result {
                                        command_reported = true;
                                        finished = true;
                                        queue_event(
                                            &mut pending,
                                            ServerEvent {
                                                request_id: 0,
                                                session: Some(session.clone()),
                                                kind: norte_code_wire::ServerEventKind::Error {
                                                    code: norte_code_wire::ErrorCode::Internal,
                                                    message: error.to_string(),
                                                },
                                            },
                                            true,
                                        );
                                    } else {
                                        command_reported = true;
                                    }
                                }
                                if pending.is_empty() {
                                    forget_attachment(&attached_sessions, session.as_str());
                                    return None;
                                }
                            }
                        }
                    }
                }
            },
        )
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
