//! Axum bindings for the Norte Code RAEP host.

use axum::{
    Json, Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderValue, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::StreamExt;
use raep_types::{Envelope, FrameType, SessionAttach, StandardFrameType};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::BroadcastStream;

use crate::{HostError, NorteCodeRaepHost};

/// Builds the RAEP HTTP, WebSocket and SSE routes for a host.
pub fn router(host: Arc<NorteCodeRaepHost>) -> Router {
    Router::new()
        .route("/raep/v1/ws", get(websocket))
        .route("/raep/v1/info", get(info))
        .route("/raep/v1/health/live", get(health_live))
        .route("/raep/v1/health/ready", get(health_ready))
        .route("/raep/v1/sessions/{session_id}/events", get(events))
        .route("/raep/v1/sessions/{session_id}/commands", post(commands))
        .with_state(host)
}

async fn info(State(host): State<Arc<NorteCodeRaepHost>>) -> Json<Value> {
    Json(host.info())
}

async fn health_live() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn health_ready(State(_host): State<Arc<NorteCodeRaepHost>>) -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn websocket(
    State(host): State<Arc<NorteCodeRaepHost>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade
        .protocols(["raep.v1"])
        .on_upgrade(move |socket| websocket_session(host, socket))
        .into_response()
}

async fn websocket_session(host: Arc<NorteCodeRaepHost>, mut socket: WebSocket) {
    if send_frame(&mut socket, host.hello()).await.is_err() {
        return;
    }
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Text(text) => {
                let frame = match serde_json::from_str::<Envelope<Value>>(&text) {
                    Ok(frame) => frame,
                    Err(error) => {
                        if send_frame(
                            &mut socket,
                            error_frame(&HostError::InvalidFrame(error.to_string())),
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                };
                let session_hint = frame.session_id.clone();
                let mut live = host.subscribe();
                let future = host.handle(frame);
                tokio::pin!(future);
                let mut emitted = std::collections::HashSet::new();
                loop {
                    tokio::select! {
                        result = &mut future => {
                            match result {
                                Ok(frames) => {
                                    for frame in frames {
                                        if emitted.insert(frame.id.clone())
                                            && send_frame(&mut socket, frame).await.is_err()
                                        {
                                            return;
                                        }
                                    }
                                }
                                Err(error) => {
                                    if send_frame(&mut socket, error_frame(&error)).await.is_err() {
                                        return;
                                    }
                                }
                            }
                            break;
                        }
                        result = live.recv() => {
                            if let Ok(frame) = result
                                && (session_hint.is_empty() || frame.session_id == session_hint)
                                && emitted.insert(frame.id.clone())
                                && send_frame(&mut socket, frame).await.is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            }
            Message::Ping(payload) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    return;
                }
            }
            Message::Pong(_) => {}
            Message::Binary(_) => {
                let _ = socket
                    .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: 1003,
                        reason: "RAEP requires text JSON frames".into(),
                    })))
                    .await;
                return;
            }
            Message::Close(_) => return,
        }
    }
}

async fn send_frame(socket: &mut WebSocket, frame: Envelope<Value>) -> Result<(), ()> {
    let text = serde_json::to_string(&frame).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

async fn events(
    State(host): State<Arc<NorteCodeRaepHost>>,
    Path(session_id): Path<String>,
) -> Response {
    let receiver = host.subscribe();
    let attach = Envelope {
        kind: FrameType::Standard(StandardFrameType::SessionAttach),
        payload: serde_json::to_value(SessionAttach {
            agent_id: host.agent_id().to_owned(),
            agent_override: None,
            create_if_missing: Some(false),
            mode: None,
            takeover: None,
            resume_from_seq: None,
            capabilities: None,
            client_info: None,
            options: None,
        })
        .unwrap_or_else(|_| json!({})),
        session_id: session_id.clone(),
        ..Envelope::new(
            FrameType::Standard(StandardFrameType::SessionAttach),
            json!({}),
            0,
        )
    };
    let initial = match host.handle(attach).await {
        Ok(frames) => frames,
        Err(error) => vec![error_frame(&error)],
    };
    let seen = Arc::new(std::sync::Mutex::new(
        initial
            .iter()
            .map(|frame| frame.id.clone())
            .collect::<std::collections::HashSet<_>>(),
    ));
    let initial_stream = futures_util::stream::iter(
        initial
            .into_iter()
            .map(|frame| Ok::<Event, Infallible>(sse_event(frame))),
    );
    let live = BroadcastStream::new(receiver).filter_map(move |result| {
        let session_id = session_id.clone();
        let seen = Arc::clone(&seen);
        async move {
            match result {
                Ok(frame)
                    if frame.session_id == session_id
                        && seen
                            .lock()
                            .map_or(true, |mut ids| ids.insert(frame.id.clone())) =>
                {
                    Some(Ok::<Event, Infallible>(sse_event(frame)))
                }
                _ => None,
            }
        }
    });
    let stream = initial_stream.chain(live);
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-cache"));
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

async fn commands(
    State(host): State<Arc<NorteCodeRaepHost>>,
    Path(session_id): Path<String>,
    Json(mut frame): Json<Envelope<Value>>,
) -> Response {
    if frame.session_id.is_empty() {
        frame.session_id = session_id;
    } else if frame.session_id != session_id {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "accepted": false,
                "error": {"code": "INVALID_FRAME", "message": "session_id does not match path"}
            })),
        )
            .into_response();
    }
    match host.handle(frame).await {
        Ok(events) => (
            StatusCode::ACCEPTED,
            Json(json!({"accepted": true, "events": events})),
        )
            .into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "accepted": false,
                "error": {"code": "INVALID_FRAME", "message": error.to_string()}
            })),
        )
            .into_response(),
    }
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

fn error_frame(error: &HostError) -> Envelope<Value> {
    Envelope::new(
        FrameType::Standard(StandardFrameType::Error),
        json!({
            "code": "INVALID_FRAME",
            "message": error.to_string(),
            "retryable": false
        }),
        0,
    )
}
