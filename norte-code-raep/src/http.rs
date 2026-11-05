//! Axum bindings for the Norte Code RAEP host.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, StatusCode},
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
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};
use tower_http::{
    limit::RequestBodyLimitLayer, timeout::RequestBodyTimeoutLayer, trace::TraceLayer,
};

use crate::{HostError, NorteCodeRaepHost};

/// Builds the RAEP HTTP, WebSocket and SSE routes for a host.
pub fn router(host: Arc<NorteCodeRaepHost>) -> Router {
    Router::new()
        .route("/raep/v1/ws", get(websocket))
        .route("/raep/v1/info", get(info))
        .route("/raep/v1/telemetry", get(telemetry))
        .route("/raep/v1/artifacts", post(upload_artifact))
        .route("/raep/v1/artifacts/{artifact_id}", get(download_artifact))
        .route("/raep/v1/health/live", get(health_live))
        .route("/raep/v1/health/ready", get(health_ready))
        .route("/raep/v1/sessions", get(list_sessions).post(create_session))
        .route(
            "/raep/v1/sessions/{session_id}",
            get(read_session)
                .patch(patch_session)
                .delete(delete_session),
        )
        .route("/raep/v1/sessions/{session_id}/events", get(events))
        .route("/raep/v1/sessions/{session_id}/commands", post(commands))
        .layer(RequestBodyLimitLayer::new(
            HostConfigLimits::from_host(&host).max_body_bytes,
        ))
        .layer(RequestBodyTimeoutLayer::new(
            HostConfigLimits::from_host(&host).command_timeout,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(host)
}

struct HostConfigLimits {
    max_body_bytes: usize,
    command_timeout: std::time::Duration,
}

impl HostConfigLimits {
    fn from_host(host: &NorteCodeRaepHost) -> Self {
        let info = host.info();
        Self {
            max_body_bytes: info["limits"]["max_frame_bytes"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(1024 * 1024),
            command_timeout: host.command_timeout(),
        }
    }
}

async fn info(State(host): State<Arc<NorteCodeRaepHost>>, headers: HeaderMap) -> Response {
    if let Err(response) = origin_guard(&host, &headers) {
        return response;
    }
    Json(host.info()).into_response()
}

async fn health_live() -> Json<Value> {
    Json(json!({"status": "live"}))
}

async fn health_ready(State(host): State<Arc<NorteCodeRaepHost>>) -> Json<Value> {
    Json(host.readiness())
}

#[derive(serde::Deserialize)]
struct TelemetryQuery {
    session: Option<String>,
}

async fn telemetry(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Query(query): Query<TelemetryQuery>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host
        .telemetry_report(query.session.as_deref(), &principal)
        .await
    {
        Ok(report) => Json(report).into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "status": "error",
                "error": {"code": "TELEMETRY_FAILED", "message": error.to_string()}
            })),
        )
            .into_response(),
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "accepted": false,
            "error": {"code": "NOT_FOUND", "message": "session not found"}
        })),
    )
        .into_response()
}

fn invalid(message: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({
            "accepted": false,
            "error": {"code": "INVALID_FRAME", "message": message}
        })),
    )
        .into_response()
}

async fn list_sessions(State(host): State<Arc<NorteCodeRaepHost>>, headers: HeaderMap) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    Json(json!({
        "sessions": host.session_summaries(&principal).await,
        "next_cursor": Value::Null,
    }))
    .into_response()
}

async fn create_session(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let body = body.map_or(Value::Null, |Json(value)| value);
    if let Some(agent) = body.get("agent_id").and_then(Value::as_str)
        && !agent.is_empty()
        && agent != host.agent_id()
    {
        return invalid(&format!("unknown agent `{agent}`"));
    }
    if body.get("labels").is_some_and(|value| !value.is_null()) {
        return invalid("labels are not supported by this host");
    }
    let idempotency = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(key) = &idempotency
        && let Some(session) = host.session_for_create_key(key).await
    {
        return match host.session_summary(&session, &principal).await {
            Some(summary) => (StatusCode::OK, Json(summary)).into_response(),
            None => not_found(),
        };
    }
    let title = body.get("title").and_then(Value::as_str).map(str::to_owned);
    let session = match host.create_session(title, &principal).await {
        Ok(session) => session,
        Err(error) => return invalid(&error.to_string()),
    };
    if let Some(key) = &idempotency {
        host.remember_create_key(key, &session).await;
    }
    match host.session_summary(&session, &principal).await {
        Some(summary) => (StatusCode::CREATED, Json(summary)).into_response(),
        None => invalid("created session could not be read back"),
    }
}

async fn read_session(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.session_summary(&session_id, &principal).await {
        Some(summary) => Json(summary).into_response(),
        None => not_found(),
    }
}

async fn patch_session(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Option<Json<Value>>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let body = body.map_or(Value::Null, |Json(value)| value);
    let Some(object) = body.as_object() else {
        return invalid("PATCH body must be a JSON object");
    };
    let mut title = None;
    for (key, value) in object {
        match key.as_str() {
            "title" => {
                let Some(text) = value.as_str() else {
                    return invalid("title must be a string");
                };
                title = Some(text.to_owned());
            }
            "labels" | "pinned" | "archived" => {
                if !value.is_null() {
                    return invalid(&format!("{key} is not supported by this host"));
                }
            }
            other => return invalid(&format!("unknown field `{other}`")),
        }
    }
    let Some(title) = title else {
        return invalid("PATCH must set the title");
    };
    match host.rename_session(&session_id, &title, &principal).await {
        Ok(true) => match host.session_summary(&session_id, &principal).await {
            Some(summary) => Json(summary).into_response(),
            None => not_found(),
        },
        Ok(false) => not_found(),
        Err(error) => invalid(&error.to_string()),
    }
}

async fn delete_session(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.delete_session(&session_id, &principal).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
        Err(error) => invalid(&error.to_string()),
    }
}

async fn upload_artifact(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };

    let mime_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream");
    match host
        .put_artifact_as(mime_type, &body, Some(&principal))
        .await
    {
        Ok(reference) => Json(json!({
            "id": reference.id,
            "mime_type": reference.mime_type,
            "bytes": reference.bytes
        }))
        .into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "accepted": false,
                "error": {"code": "ARTIFACT_FAILED", "message": error.to_string()}
            })),
        )
            .into_response(),
    }
}

async fn download_artifact(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(artifact_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.get_artifact_as(&artifact_id, Some(&principal)).await {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(error) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "accepted": false,
                "error": {"code": "ARTIFACT_NOT_FOUND", "message": error.to_string()}
            })),
        )
            .into_response(),
    }
}

async fn websocket(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let Some(permit) = host.try_connection() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    upgrade
        .protocols(["raep.v1"])
        .on_upgrade(move |socket| websocket_session(host, socket, permit, principal))
        .into_response()
}

#[allow(clippy::too_many_lines)]
async fn websocket_session(
    host: Arc<NorteCodeRaepHost>,
    mut socket: WebSocket,
    _permit: tokio::sync::OwnedSemaphorePermit,
    principal: String,
) {
    if send_frame(&mut socket, host.hello()).await.is_err() {
        return;
    }
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Text(text) => {
                if text.len() > host.max_frame_bytes() {
                    if send_frame(
                        &mut socket,
                        error_frame(&HostError::InvalidFrame(
                            "frame exceeds size limit".to_owned(),
                        )),
                    )
                    .await
                    .is_err()
                    {
                        return;
                    }
                    continue;
                }
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
                let future = tokio::time::timeout(
                    host.command_timeout(),
                    host.handle_as(frame, Some(&principal)),
                );
                tokio::pin!(future);
                let mut emitted = std::collections::HashSet::new();
                loop {
                    tokio::select! {
                        result = &mut future => {
                            match result {
                                Ok(Ok(frames)) => {
                                    for frame in frames {
                                        if emitted.insert(frame.id.clone())
                                            && send_frame(&mut socket, frame).await.is_err()
                                        {
                                            return;
                                        }
                                    }
                                }
                                Ok(Err(error)) => {
                                    if send_frame(&mut socket, error_frame(&error)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(_) => {
                                    if send_frame(
                                        &mut socket,
                                        error_frame(&HostError::Service(
                                            "command timed out".to_owned(),
                                        )),
                                    )
                                    .await
                                    .is_err()
                                    {
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

#[derive(serde::Deserialize)]
struct EventsQuery {
    resume_from_seq: Option<u64>,
}

async fn events(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<EventsQuery>,
) -> Response {
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
    let attach = Envelope {
        kind: FrameType::Standard(StandardFrameType::SessionAttach),
        payload: serde_json::to_value(SessionAttach {
            agent_id: host.agent_id().to_owned(),
            agent_override: None,
            create_if_missing: Some(false),
            mode: None,
            takeover: None,
            resume_from_seq,
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
    let mut initial = vec![host.hello()];
    initial.extend(match host.handle_as(attach, Some(&principal)).await {
        Ok(frames) => frames,
        Err(error) => vec![error_frame(&error)],
    });
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
        let _permit = &permit;
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
                Ok(_) => None,
                Err(BroadcastStreamRecvError::Lagged(lost)) => {
                    let mut frame = Envelope::new(
                        FrameType::Standard(StandardFrameType::Error),
                        json!({
                            "code": "REPLAY_TRUNCATED",
                            "message": format!(
                                "the event stream fell behind and dropped {lost} frames; reconnect from the last sequence number"
                            ),
                            "retryable": true,
                            "details": {"lost": lost}
                        }),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)),
                    );
                    frame.session_id = session_id;
                    Some(Ok::<Event, Infallible>(sse_event(frame)))
                }
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
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(mut frame): Json<Envelope<Value>>,
) -> Response {
    tracing::info!(session = %session_id, "raep command received");
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
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
    if matches!(
        &frame.kind,
        FrameType::Extension(name) if name == crate::NorteCodeExtension::Command.as_str()
    ) && serde_json::from_value::<norte_code_protocol::ClientRequest>(frame.payload.clone())
        .is_ok_and(|request| {
            matches!(
                request.kind,
                norte_code_protocol::ClientRequestKind::SendMessage { .. }
            )
        })
    {
        let host_for_turn = Arc::clone(&host);
        let principal_for_turn = principal.clone();
        tokio::spawn(async move {
            let _ = host_for_turn
                .handle_as(frame, Some(&principal_for_turn))
                .await;
        });
        return (
            StatusCode::ACCEPTED,
            Json(json!({"accepted": true, "events": []})),
        )
            .into_response();
    }
    match tokio::time::timeout(
        host.command_timeout(),
        host.handle_as(frame, Some(&principal)),
    )
    .await
    {
        Ok(Ok(events)) => (
            StatusCode::ACCEPTED,
            Json(json!({"accepted": true, "events": events})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({
                "accepted": false,
                "error": {"code": "TIMEOUT", "message": "command timed out"}
            })),
        )
            .into_response(),
        Ok(Err(error)) => (
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

#[allow(clippy::result_large_err)]
fn origin_guard(host: &NorteCodeRaepHost, headers: &HeaderMap) -> Result<(), Response> {
    if let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok())
        && !host.origin_allowed(origin)
    {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({
                "accepted": false,
                "error": {"code": "ORIGIN_NOT_ALLOWED", "message": "origin is not allowed"}
            })),
        )
            .into_response());
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn principal(host: &NorteCodeRaepHost, headers: &HeaderMap) -> Result<String, Response> {
    origin_guard(host, headers)?;
    if !host.authentication_enabled() {
        return Ok(host.anonymous_principal().to_owned());
    }
    let supplied = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    supplied
        .and_then(|token| host.authenticate_bearer(token))
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "accepted": false,
                    "error": {"code": "UNAUTHENTICATED", "message": "valid bearer token required"}
                })),
            )
                .into_response()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostConfig;
    use axum::body::Body;
    use axum::http::Request;
    use norte_code_agent::Service;
    use norte_code_protocol::{ClientRequest, ClientRequestKind, InMemoryStore};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_service() -> Arc<Service> {
        Arc::new(
            Service::new(
                norte_code_protocol::config::Config::default(),
                Arc::new(norte_code_protocol::catalog::Catalog::default()),
                None,
                Arc::new(norte_code_agent::VendorModels),
                Arc::new(InMemoryStore::new()),
                Arc::new(norte::InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        )
    }

    fn test_host(token: Option<&str>) -> Arc<NorteCodeRaepHost> {
        Arc::new(
            NorteCodeRaepHost::new(
                test_service(),
                HostConfig {
                    auth_token: token.map(str::to_owned),
                    ..HostConfig::default()
                },
            )
            .expect("host"),
        )
    }

    #[tokio::test]
    async fn health_and_info_are_public_when_authentication_is_enabled() {
        let app = router(test_host(Some("secret")));
        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/raep/v1/health/live")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(health.status(), StatusCode::OK);
        let body = axum::body::to_bytes(health.into_body(), 1024)
            .await
            .expect("health body");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).expect("health"),
            json!({"status": "live"})
        );

        let info = app
            .oneshot(
                Request::builder()
                    .uri("/raep/v1/info")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(info.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn protected_artifact_routes_require_the_configured_token() {
        let app = router(test_host(Some("secret")));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/artifacts")
                    .header("content-type", "text/plain")
                    .body(Body::from("hello"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn extension_commands_return_remote_telemetry_and_tools() {
        let app = router(test_host(None));
        let request = ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::ListTelemetry { session: None },
        };
        let frame = Envelope::new(
            FrameType::Extension(crate::NorteCodeExtension::Command.as_str().to_owned()),
            serde_json::to_value(request).expect("request"),
            1,
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/sessions/unused/commands")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&frame).expect("frame")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let body: Value = serde_json::from_slice(&body).expect("json");
        let events = body["events"].as_array().expect("events");
        assert_eq!(events[0]["payload"]["event"], "service_data");
        assert_eq!(events[0]["payload"]["kind"], "telemetry");

        let request = ClientRequest {
            request_id: 2,
            kind: ClientRequestKind::ListTools { session: None },
        };
        let frame = Envelope::new(
            FrameType::Extension(crate::NorteCodeExtension::Command.as_str().to_owned()),
            serde_json::to_value(request).expect("request"),
            2,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/sessions/unused/commands")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&frame).expect("frame")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let body: Value = serde_json::from_slice(&body).expect("json");
        let events = body["events"].as_array().expect("events");
        assert_eq!(events[0]["payload"]["kind"], "tool_list");
    }

    #[tokio::test]
    async fn browser_origin_must_be_on_the_allow_list() {
        let host = Arc::new(
            NorteCodeRaepHost::new(
                test_service(),
                HostConfig {
                    allowed_origins: vec!["https://app.example".to_owned()],
                    ..HostConfig::default()
                },
            )
            .expect("host"),
        );
        let app = router(host);
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/raep/v1/info")
                    .header("origin", "https://evil.example")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);

        let allowed = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/raep/v1/info")
                    .header("origin", "https://app.example")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn events_stream_starts_with_hello_and_reads_last_event_id() {
        let app = router(test_host(None));
        let session = create_session(&app).await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/raep/v1/sessions/{session}/events"))
                    .header("last-event-id", "0")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        let mut text = String::new();
        for _ in 0..10 {
            let chunk = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                futures_util::StreamExt::next(&mut body),
            )
            .await
            .expect("frame timed out")
            .expect("body ended early")
            .expect("body error");
            text.push_str(&String::from_utf8_lossy(&chunk));
            if text.contains("event: HELLO") && text.contains("event: SESSION_ATTACHED") {
                break;
            }
        }
        assert!(
            text.contains("event: HELLO"),
            "stream must open with HELLO: {text}"
        );
        assert!(
            text.contains("event: SESSION_ATTACHED"),
            "stream must attach after HELLO: {text}"
        );
    }

    #[tokio::test]
    async fn events_stream_accepts_resume_from_seq() {
        let app = router(test_host(None));
        let session = create_session(&app).await;

        // A known cursor is accepted and the stream opens; reading the body
        // is not needed to prove the query was understood.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/raep/v1/sessions/{session}/events?resume_from_seq=0"
                    ))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/raep/v1/sessions/{session}/events?resume_from_seq=abc"
                    ))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn session_resource_lists_reads_updates_and_deletes() {
        let app = router(test_host(None));
        let create = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/sessions")
                    .header("content-type", "application/json")
                    .header("idempotency-key", "create-1")
                    .body(Body::from(
                        r#"{"agent_id":"norte-code","title":"resource harness"}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(create.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(create.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let summary: Value = serde_json::from_slice(&body).expect("json");
        let session = summary["session_id"]
            .as_str()
            .expect("session id")
            .to_owned();
        assert_eq!(summary["title"], "resource harness");
        assert_eq!(summary["status"], "ACTIVE");
        assert!(summary["created_at"].as_i64().is_some());

        let repeat = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/sessions")
                    .header("content-type", "application/json")
                    .header("idempotency-key", "create-1")
                    .body(Body::from(r#"{"agent_id":"norte-code"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(repeat.status(), StatusCode::OK);
        let body = axum::body::to_bytes(repeat.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let repeat: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(repeat["session_id"], session);

        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/raep/v1/sessions")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(list.status(), StatusCode::OK);
        let body = axum::body::to_bytes(list.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let list: Value = serde_json::from_slice(&body).expect("json");
        assert!(
            list["sessions"]
                .as_array()
                .expect("sessions")
                .iter()
                .any(|entry| entry["session_id"] == session)
        );

        let read = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/sessions/{session}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(read.status(), StatusCode::OK);

        let patch = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/raep/v1/sessions/{session}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"renamed"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(patch.status(), StatusCode::OK);
        let body = axum::body::to_bytes(patch.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let patched: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(patched["title"], "renamed");

        let unsupported = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/raep/v1/sessions/{session}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"pinned":true}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(unsupported.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let delete = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/raep/v1/sessions/{session}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(delete.status(), StatusCode::NO_CONTENT);

        let missing = app
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/sessions/{session}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_session_is_hidden_from_another_principal() {
        let host = Arc::new(
            NorteCodeRaepHost::new(
                test_service(),
                HostConfig {
                    auth_principals: std::collections::HashMap::from([
                        ("token-a".to_owned(), "alice".to_owned()),
                        ("token-b".to_owned(), "bob".to_owned()),
                    ]),
                    ..HostConfig::default()
                },
            )
            .expect("host"),
        );
        let app = router(host);
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/sessions")
                    .header("authorization", "Bearer token-a")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"agent_id":"norte-code"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(created.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(created.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let summary: Value = serde_json::from_slice(&body).expect("json");
        let session = summary["session_id"]
            .as_str()
            .expect("session id")
            .to_owned();

        let stolen = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/sessions/{session}"))
                    .header("authorization", "Bearer token-b")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(stolen.status(), StatusCode::NOT_FOUND);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/raep/v1/sessions")
                    .header("authorization", "Bearer token-b")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(list.status(), StatusCode::OK);
        let body = axum::body::to_bytes(list.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let list: Value = serde_json::from_slice(&body).expect("json");
        assert!(
            list["sessions"].as_array().expect("sessions").is_empty(),
            "a principal must not see another principal's sessions"
        );
    }

    async fn create_session(app: &Router) -> String {
        let request = ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::CreateSession {
                title: Some("event harness".to_owned()),
            },
        };
        let frame = Envelope::new(
            FrameType::Extension(crate::NorteCodeExtension::Command.as_str().to_owned()),
            serde_json::to_value(request).expect("request"),
            1,
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/sessions/unused/commands")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&frame).expect("frame")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let body: Value = serde_json::from_slice(&body).expect("json");
        body["events"][0]["session_id"]
            .as_str()
            .expect("session id")
            .to_owned()
    }
}
