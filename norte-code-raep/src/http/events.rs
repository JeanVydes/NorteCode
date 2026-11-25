//! Server-Sent Events transport for RAEP sessions.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use raep_types::{AttachMode, Envelope, FrameType, SessionAttach, StandardFrameType};
use serde_json::Value;
use std::convert::Infallible;
use std::sync::Arc;

use super::seen_frames::SeenFrames;
use super::{correlated_error_frame, error_frame, error_status, payload, principal};
use crate::{HostError, NorteCodeRaepHost};

#[derive(serde::Serialize)]
struct ReplayLostPayload {
    lost: u64,
    close_code: u16,
}

#[derive(serde::Deserialize)]
pub(super) struct EventsQuery {
    resume_from_seq: Option<u64>,
    agent_id: Option<String>,
    mode: Option<AttachMode>,
    create_if_missing: Option<bool>,
    takeover: Option<bool>,
    attach: Option<String>,
}

struct StreamAttachmentGuard {
    host: Arc<NorteCodeRaepHost>,
    session_id: String,
    attachment: raep_types::FrameId,
}

impl Drop for StreamAttachmentGuard {
    fn drop(&mut self) {
        let host = Arc::clone(&self.host);
        let session_id = self.session_id.clone();
        let attachment = self.attachment.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                host.detach_stream(&session_id, &attachment).await;
            });
        }
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn events(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<EventsQuery>,
) -> Response {
    let accepts_event_stream = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|media_type| media_type.trim().starts_with("text/event-stream"))
        });
    if !accepts_event_stream {
        return StatusCode::NOT_ACCEPTABLE.into_response();
    }
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let Some(permit) = host.try_connection() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let receiver = host.subscribe();
    let resume_from_seq = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .or(query.resume_from_seq);
    let attach_payload = if let Some(encoded) = query.attach {
        let Ok(bytes) = URL_SAFE_NO_PAD.decode(encoded) else {
            return (
                StatusCode::BAD_REQUEST,
                Json(error_frame(&HostError::InvalidFrame(
                    "invalid base64url SESSION_ATTACH payload".to_owned(),
                ))),
            )
                .into_response();
        };
        let Ok(attach) = serde_json::from_slice::<SessionAttach>(&bytes) else {
            return (
                StatusCode::BAD_REQUEST,
                Json(error_frame(&HostError::InvalidFrame(
                    "invalid SESSION_ATTACH payload".to_owned(),
                ))),
            )
                .into_response();
        };
        attach
    } else {
        SessionAttach {
            agent_id: query.agent_id.unwrap_or_else(|| host.agent_id().to_owned()),
            agent_override: None,
            create_if_missing: query.create_if_missing.or(Some(true)),
            mode: query.mode,
            takeover: query.takeover,
            resume_from_seq,
            capabilities: None,
            client_info: None,
            options: None,
        }
    };
    let attach = Envelope {
        kind: FrameType::Standard(StandardFrameType::SessionAttach),
        payload: payload(attach_payload),
        session_id: if session_id == "new" {
            String::new()
        } else {
            session_id.clone()
        },
        ..Envelope::new(
            FrameType::Standard(StandardFrameType::SessionAttach),
            Value::Object(serde_json::Map::new()),
            0,
        )
    };
    let request_id = attach.id.clone();
    let request_session_id = attach.session_id.clone();
    let mut initial = vec![host.hello()];
    match host.handle_as(attach, Some(&principal)).await {
        Ok(frames) => initial.extend(frames),
        Err(error) => {
            return (
                error_status(&error),
                Json(correlated_error_frame(
                    &error,
                    Some(request_id),
                    request_session_id,
                )),
            )
                .into_response();
        }
    }
    let stream_session_id = initial
        .iter()
        .find(|frame| frame.kind == FrameType::Standard(StandardFrameType::SessionAttached))
        .map_or(session_id, |frame| frame.session_id.clone());
    let attachment_guard = initial
        .iter()
        .any(|frame| frame.kind == FrameType::Standard(StandardFrameType::SessionAttached))
        .then(|| StreamAttachmentGuard {
            host: Arc::clone(&host),
            session_id: stream_session_id.clone(),
            attachment: request_id,
        });
    let seen = Arc::new(std::sync::Mutex::new(SeenFrames::from_initial(
        initial.iter().map(|frame| frame.id.clone()),
    )));
    let prelude = futures_util::stream::iter([
        Ok::<Event, Infallible>(Event::default().retry(std::time::Duration::from_secs(2))),
        Ok::<Event, Infallible>(Event::default().comment("keepalive")),
    ]);
    let initial_stream = prelude.chain(futures_util::stream::iter(
        initial
            .into_iter()
            .map(|frame| Ok::<Event, Infallible>(sse_event(frame))),
    ));
    let live = futures_util::stream::unfold(
        (
            receiver,
            stream_session_id,
            seen,
            permit,
            attachment_guard,
            false,
        ),
        |(mut receiver, session_id, seen, permit, attachment_guard, done)| async move {
            if done {
                return None;
            }
            loop {
                match receiver.recv().await {
                    Ok(frame)
                        if frame.session_id == session_id
                            && seen
                                .lock()
                                .map_or(true, |mut ids| ids.insert(frame.id.clone())) =>
                    {
                        return Some((
                            Ok::<Event, Infallible>(sse_event(frame)),
                            (receiver, session_id, seen, permit, attachment_guard, false),
                        ));
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(lost)) => {
                        let mut frame = Envelope::new(
                            FrameType::Standard(StandardFrameType::Error),
                            payload(raep_types::ErrorPayload {
                                code: "REPLAY_TRUNCATED".to_owned(),
                                message: format!(
                                    "the event stream fell behind and dropped {lost} frames; reconnect from the last sequence number"
                                ),
                                retryable: Some(true),
                                details: Some(payload(ReplayLostPayload {
                                    lost,
                                    close_code: 4008,
                                })),
                            }),
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |elapsed| {
                                    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
                                }),
                        );
                        frame.session_id.clone_from(&session_id);
                        return Some((
                            Ok::<Event, Infallible>(sse_event(frame)),
                            (receiver, session_id, seen, permit, attachment_guard, true),
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    );
    let stream = initial_stream.chain(live);
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-cache"));
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn sse_event(frame: Envelope<Value>) -> Event {
    let event = match &frame.kind {
        FrameType::Standard(kind) => serde_json::to_string(kind)
            .unwrap_or_else(|_| "MESSAGE".to_owned())
            .trim_matches('"')
            .to_owned(),
        FrameType::Extension(name) => name.clone(),
    };
    let mut output = Event::default().event(event);
    if let Some(seq) = frame.seq {
        output = output.id(seq.to_string());
    }
    output.json_data(frame).unwrap_or_else(|_| Event::default())
}
