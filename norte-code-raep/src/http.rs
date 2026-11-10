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
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use raep_types::{AttachMode, Envelope, FrameId, FrameType, SessionAttach, StandardFrameType};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{convert::Infallible, sync::Arc};
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    timeout::RequestBodyTimeoutLayer,
    trace::TraceLayer,
};

use crate::payloads::payload;
use crate::{HostError, NorteCodeRaepHost};

/// A health/live payload.
#[derive(Debug, serde::Serialize)]
struct LivePayload {
    status: &'static str,
}

/// An error body shared by every rejected HTTP response.
#[derive(Debug, serde::Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

/// A rejected-response payload.
#[derive(Debug, serde::Serialize)]
struct RejectedPayload {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted: Option<bool>,
    error: ErrorBody,
}

/// A telemetry failure payload.
#[derive(Debug, serde::Serialize)]
struct TelemetryFailedPayload {
    status: &'static str,
    error: ErrorBody,
}

/// An agent list payload.
#[derive(Debug, serde::Serialize)]
struct AgentListPayload {
    agents: Vec<Value>,
    next_cursor: Value,
}

/// A session list payload.
#[derive(Debug, serde::Serialize)]
struct SessionListPayload {
    sessions: Vec<Value>,
    next_cursor: Value,
}

// The child list is the shared shape, not a second struct with the same fields.
use norte_code_wire::ChildSessionSummary as ChildSessionPayload;

#[derive(Debug, serde::Serialize)]
struct ChildSessionListPayload {
    children: Vec<ChildSessionPayload>,
}

/// An artifact reference payload.
#[derive(Debug, serde::Serialize)]
struct ArtifactRefPayload<'a> {
    id: &'a str,
    mime_type: &'a str,
    bytes: u64,
}

/// A command response payload.
#[derive(Debug, serde::Serialize)]
struct CommandResponsePayload {
    accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    events: Option<Vec<Envelope<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody>,
}

/// How many frames a lagged subscriber lost.
#[derive(Debug, serde::Serialize)]
struct ReplayLostPayload {
    lost: u64,
    close_code: u16,
}

/// Builds the RAEP HTTP, WebSocket and SSE routes for a host.
pub fn router(host: Arc<NorteCodeRaepHost>) -> Router {
    let limits = HostConfigLimits::from_host(&host);
    let cors_host = Arc::clone(&host);
    Router::new()
        .route("/raep/v1/ws", get(websocket))
        .route("/raep/v1/info", get(info))
        .route("/raep/v1/telemetry", get(telemetry))
        .route(
            "/raep/v1/artifacts",
            post(upload_artifact).layer(RequestBodyLimitLayer::new(limits.max_artifact_bytes)),
        )
        .route(
            "/raep/v1/sessions/{session_id}/uploads",
            post(upload_session_upload)
                .layer(RequestBodyLimitLayer::new(limits.max_artifact_bytes)),
        )
        .route("/raep/v1/artifacts/{artifact_id}", get(download_artifact))
        .route("/raep/v1/exports/{export_id}", get(download_export))
        .route(
            "/raep/v1/imports",
            post(import_bundle).layer(RequestBodyLimitLayer::new(limits.max_export_bytes)),
        )
        .route(
            "/raep/v1/imports/{import_id}/commit",
            post(commit_import).layer(RequestBodyLimitLayer::new(limits.max_frame_bytes)),
        )
        .route(
            "/raep/v1/imports/{import_id}",
            axum::routing::delete(delete_import),
        )
        .route("/raep/v1/health/live", get(health_live))
        .route("/raep/v1/health/ready", get(health_ready))
        .route("/raep/v1/agents", get(list_agents))
        .route("/raep/v1/agents/{agent_id}", get(read_agent))
        .route("/raep/v1/agents/{agent_id}/versions", get(agent_versions))
        .route(
            "/raep/v1/sessions",
            get(list_sessions)
                .post(create_session)
                .layer(RequestBodyLimitLayer::new(limits.max_frame_bytes)),
        )
        .route(
            "/raep/v1/sessions/{session_id}",
            get(read_session)
                .patch(patch_session)
                .delete(delete_session)
                .layer(RequestBodyLimitLayer::new(limits.max_frame_bytes)),
        )
        .route(
            "/raep/v1/sessions/{session_id}/children",
            get(list_child_sessions),
        )
        .route("/raep/v1/sessions/{session_id}/events", get(events))
        .route(
            "/raep/v1/sessions/{session_id}/outputs/{output_id}",
            get(download_session_output),
        )
        .route(
            "/raep/v1/sessions/{session_id}/commands",
            post(commands).layer(RequestBodyLimitLayer::new(limits.max_frame_bytes)),
        )
        .route("/raep/v1/quota", get(quota))
        .layer(RequestBodyTimeoutLayer::new(limits.command_timeout))
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(move |origin, _| {
                    origin
                        .to_str()
                        .is_ok_and(|origin| cors_host.origin_allowed(origin))
                }))
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(host)
}

struct HostConfigLimits {
    max_frame_bytes: usize,
    max_artifact_bytes: usize,
    max_export_bytes: usize,
    command_timeout: std::time::Duration,
}

impl HostConfigLimits {
    fn from_host(host: &NorteCodeRaepHost) -> Self {
        let info = host.info();
        Self {
            max_frame_bytes: info["limits"]["max_frame_bytes"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(1024 * 1024),
            max_artifact_bytes: host.max_artifact_bytes(),
            max_export_bytes: host.max_export_bytes(),
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
    Json(payload(LivePayload { status: "live" }))
}

async fn health_ready(State(host): State<Arc<NorteCodeRaepHost>>) -> Json<Value> {
    Json(host.readiness())
}

async fn quota(State(host): State<Arc<NorteCodeRaepHost>>, headers: HeaderMap) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    Json(host.quota(&principal).await).into_response()
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
            Json(payload(TelemetryFailedPayload {
                status: "error",
                error: ErrorBody {
                    code: "TELEMETRY_FAILED",
                    message: error.to_string(),
                },
            })),
        )
            .into_response(),
    }
}

fn not_found_message(message: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(payload(RejectedPayload {
            status: "error",
            accepted: Some(false),
            error: ErrorBody {
                code: "NOT_FOUND",
                message: message.to_owned(),
            },
        })),
    )
        .into_response()
}

fn not_found() -> Response {
    not_found_message("session not found")
}

fn invalid(message: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(payload(RejectedPayload {
            status: "error",
            accepted: Some(false),
            error: ErrorBody {
                code: "INVALID_FRAME",
                message: message.to_owned(),
            },
        })),
    )
        .into_response()
}

async fn list_agents(State(host): State<Arc<NorteCodeRaepHost>>, headers: HeaderMap) -> Response {
    if let Err(response) = principal(&host, &headers) {
        return response;
    }
    Json(payload(AgentListPayload {
        agents: host.agents(),
        next_cursor: Value::Null,
    }))
    .into_response()
}

async fn read_agent(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> Response {
    if let Err(response) = principal(&host, &headers) {
        return response;
    }
    match host.agent(&agent_id) {
        Some(agent) => Json(agent).into_response(),
        None => not_found_message("agent not found"),
    }
}

async fn agent_versions(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> Response {
    if let Err(response) = principal(&host, &headers) {
        return response;
    }
    match host.agent_versions(&agent_id) {
        Some(versions) => Json(versions).into_response(),
        None => not_found_message("agent not found"),
    }
}

async fn list_sessions(State(host): State<Arc<NorteCodeRaepHost>>, headers: HeaderMap) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    Json(payload(SessionListPayload {
        sessions: host.session_summaries(&principal).await,
        next_cursor: Value::Null,
    }))
    .into_response()
}

async fn list_child_sessions(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.child_sessions(&session_id, &principal).await {
        Some(children) => Json(payload(ChildSessionListPayload {
            children: children
                .into_iter()
                .map(|child| ChildSessionPayload {
                    session_id: child.session.to_string(),
                    title: child.title,
                    state: child.state,
                    ..ChildSessionPayload::default()
                })
                .collect(),
        }))
        .into_response(),
        None => not_found(),
    }
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
    let agent_id = body
        .get("agent_id")
        .and_then(Value::as_str)
        .filter(|agent| !agent.is_empty())
        .unwrap_or_else(|| host.agent_id());
    if host.agent(agent_id).is_none() {
        return invalid(&format!("unknown agent `{agent_id}`"));
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
    let session = match host.create_session(agent_id, title, &principal).await {
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
        Ok(reference) => Json(payload(ArtifactRefPayload {
            id: &reference.id,
            mime_type: &reference.mime_type,
            bytes: reference.bytes,
        }))
        .into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "ARTIFACT_FAILED",
                    message: error.to_string(),
                },
            })),
        )
            .into_response(),
    }
}

async fn upload_session_upload(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Bytes,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    if host
        .session_summary(&session_id, &principal)
        .await
        .is_none()
    {
        return not_found();
    }
    let Some(mime_type) = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
    else {
        return invalid("Content-Type is required for uploads");
    };
    if mime_type == "application/json" {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "UPLOAD_MODE_UNSUPPORTED",
                    message: "resumable uploads are not supported".to_owned(),
                },
            })),
        )
            .into_response();
    }
    match host
        .put_session_upload_as(&session_id, mime_type, &body, Some(&principal))
        .await
    {
        Ok(receipt) => (StatusCode::CREATED, Json(receipt)).into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "UPLOAD_REJECTED",
                    message: error.to_string(),
                },
            })),
        )
            .into_response(),
    }
}

async fn download_session_output(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path((session_id, output_id)): Path<(String, String)>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let Ok((bytes, mime_type)) = host
        .get_session_output_as(&session_id, &output_id, Some(&principal))
        .await
    else {
        return not_found();
    };
    let range = match headers
        .get(axum::http::header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => match parse_byte_range(value, bytes.len()) {
            Ok(range) => range,
            Err(()) => {
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [
                        ("accept-ranges", "bytes".to_owned()),
                        ("content-range", format!("bytes */{}", bytes.len())),
                    ],
                )
                    .into_response();
            }
        },
        None => None,
    };
    let Ok(content_type) = HeaderValue::from_str(&mime_type) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut response_headers = HeaderMap::new();
    response_headers.insert(axum::http::header::CONTENT_TYPE, content_type);
    response_headers.insert(
        axum::http::header::ACCEPT_RANGES,
        HeaderValue::from_static("bytes"),
    );
    match range {
        Some((start, end)) => {
            response_headers.insert(
                axum::http::header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{}", bytes.len()))
                    .expect("numeric content-range is a valid header"),
            );
            (
                StatusCode::PARTIAL_CONTENT,
                response_headers,
                Bytes::copy_from_slice(&bytes[start..=end]),
            )
                .into_response()
        }
        None => (StatusCode::OK, response_headers, Bytes::from(bytes)).into_response(),
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
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "ARTIFACT_NOT_FOUND",
                    message: error.to_string(),
                },
            })),
        )
            .into_response(),
    }
}

async fn download_export(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(export_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let Ok((bytes, _)) = host.get_export_as(&export_id, &principal).await else {
        return not_found();
    };
    let range = match headers
        .get(axum::http::header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => match parse_byte_range(value, bytes.len()) {
            Ok(range) => range,
            Err(()) => {
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [
                        ("accept-ranges", "bytes".to_owned()),
                        ("content-range", format!("bytes */{}", bytes.len())),
                    ],
                )
                    .into_response();
            }
        },
        None => None,
    };
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.raep.bundle+zip"),
    );
    response_headers.insert(
        axum::http::header::ACCEPT_RANGES,
        HeaderValue::from_static("bytes"),
    );
    match range {
        Some((start, end)) => {
            response_headers.insert(
                axum::http::header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{}", bytes.len()))
                    .expect("numeric content-range is a valid header"),
            );
            (
                StatusCode::PARTIAL_CONTENT,
                response_headers,
                Bytes::copy_from_slice(&bytes[start..=end]),
            )
                .into_response()
        }
        None => (StatusCode::OK, response_headers, Bytes::from(bytes)).into_response(),
    }
}

async fn import_bundle(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.preview_import_as(&body, &principal).await {
        Ok(report) => (StatusCode::CREATED, Json(report)).into_response(),
        Err(error) => invalid(&error.to_string()),
    }
}

async fn commit_import(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(import_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let request = match serde_json::from_value::<crate::host::ImportCommitRequest>(body) {
        Ok(request) => request,
        Err(error) => return invalid(&error.to_string()),
    };
    match host.commit_import_as(&import_id, request, &principal).await {
        Ok(result) => Json(result).into_response(),
        Err(crate::host::HostError::InvalidFrame(message)) if message == "IMPORT_NOT_FOUND" => {
            not_found_message("import not found or expired")
        }
        Err(error) => invalid(&error.to_string()),
    }
}

async fn delete_import(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(import_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    if host.delete_import_as(&import_id, &principal).await {
        StatusCode::NO_CONTENT.into_response()
    } else {
        not_found_message("import not found or expired")
    }
}

fn parse_byte_range(value: &str, total: usize) -> Result<Option<(usize, usize)>, ()> {
    let Some(range) = value.strip_prefix("bytes=") else {
        return Err(());
    };
    if range.contains(',') || total == 0 {
        return Err(());
    }
    let (start, end) = range.split_once('-').ok_or(())?;
    let (start, end) = if start.is_empty() {
        let suffix = end.parse::<usize>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (total.saturating_sub(suffix), total - 1)
    } else {
        let start = start.parse::<usize>().map_err(|_| ())?;
        let end = if end.is_empty() {
            total - 1
        } else {
            end.parse::<usize>().map_err(|_| ())?.min(total - 1)
        };
        (start, end)
    };
    if start >= total || start > end {
        return Err(());
    }
    Ok(Some((start, end)))
}

async fn websocket(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let supports_raep_v1 = headers
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|protocol| protocol.trim() == "raep.v1")
        });
    if !supports_raep_v1 {
        return StatusCode::UPGRADE_REQUIRED.into_response();
    }
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let Some(permit) = host.try_connection() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    upgrade
        .protocols(["raep.v1"])
        .max_message_size(host.max_frame_bytes())
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
    let mut attached_sessions = std::collections::HashSet::new();
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_secs(20),
        std::time::Duration::from_secs(20),
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_pong = tokio::time::Instant::now();
    'connection: loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(Ok(message)) = message else { break };
                match message {
            Message::Text(text) => {
                if text.len() > host.max_frame_bytes() {
                    close_socket(&mut socket, 1009, "RAEP frame exceeds size limit").await;
                    break 'connection;
                }
                let Ok(frame) = serde_json::from_str::<Envelope<Value>>(&text) else {
                    close_socket(&mut socket, 4400, "malformed RAEP JSON frame").await;
                    break 'connection;
                };
                if frame.version != raep_types::WIRE_VERSION {
                    close_socket(&mut socket, 4426, "unsupported RAEP wire version").await;
                    break 'connection;
                }
                let mut session_hint = frame.session_id.clone();
                let request_id = frame.id.clone();
                let is_attach = frame.kind == FrameType::Standard(StandardFrameType::SessionAttach);
                let is_detach = matches!(
                    &frame.kind,
                    FrameType::Standard(
                        StandardFrameType::SessionDetach | StandardFrameType::SessionClose
                    )
                );
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
                                        if is_attach
                                            && frame.kind
                                                == FrameType::Standard(StandardFrameType::SessionAttached)
                                        {
                                            attached_sessions.insert(frame.session_id.clone());
                                            session_hint.clone_from(&frame.session_id);
                                        }
                                        if emitted.insert(frame.id.clone())
                                            && send_frame(&mut socket, frame).await.is_err()
                                        {
                                            break 'connection;
                                        }
                                    }
                                    if is_detach {
                                        attached_sessions.remove(&session_hint);
                                    }
                                }
                                Ok(Err(error)) => {
                                    if send_frame(
                                        &mut socket,
                                        correlated_error_frame(
                                            &error,
                                            Some(request_id),
                                            session_hint.clone(),
                                        ),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        break 'connection;
                                    }
                                }
                                Err(_) => {
                                    let error = HostError::Service("command timed out".to_owned());
                                    if send_frame(
                                        &mut socket,
                                        correlated_error_frame(
                                            &error,
                                            Some(request_id),
                                            session_hint.clone(),
                                        ),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        break 'connection;
                                    }
                                }
                            }
                            break;
                        }
                        result = live.recv() => {
                            if let Ok(frame) = result
                                && frame.session_id == session_hint
                                && emitted.insert(frame.id.clone())
                                && send_frame(&mut socket, frame).await.is_err()
                            {
                                break 'connection;
                            }
                        }
                    }
                }
            }
            Message::Ping(payload) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    break 'connection;
                }
            }
            Message::Pong(_) => last_pong = tokio::time::Instant::now(),
            Message::Binary(_) => {
                close_socket(&mut socket, 4400, "RAEP requires text JSON frames").await;
                break 'connection;
            }
            Message::Close(_) => break 'connection,
                }
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() >= std::time::Duration::from_secs(60) {
                    close_socket(&mut socket, 1001, "websocket heartbeat timed out").await;
                    break 'connection;
                }
                if socket.send(Message::Ping(axum::body::Bytes::new())).await.is_err() {
                    break 'connection;
                }
            }
        }
    }
    for session_id in attached_sessions {
        let mut frame = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionDetach),
            payload(EmptyPayload {}),
            0,
        );
        frame.session_id = session_id.clone();
        if let Err(error) = host.handle_as(frame, Some(&principal)).await {
            tracing::warn!(session = %session_id, %error, "failed to detach closed WebSocket session");
        }
    }
}

async fn close_socket(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
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
    agent_id: Option<String>,
    mode: Option<AttachMode>,
    create_if_missing: Option<bool>,
    takeover: Option<bool>,
    attach: Option<String>,
}

#[derive(serde::Serialize)]
struct EmptyPayload {}

struct StreamAttachmentGuard {
    host: Arc<NorteCodeRaepHost>,
    principal: String,
    session_id: String,
}

impl Drop for StreamAttachmentGuard {
    fn drop(&mut self) {
        let host = Arc::clone(&self.host);
        let principal = self.principal.clone();
        let session_id = self.session_id.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let mut frame = Envelope::new(
                    FrameType::Standard(StandardFrameType::SessionDetach),
                    payload(EmptyPayload {}),
                    0,
                );
                frame.session_id = session_id.clone();
                if let Err(error) = host.handle_as(frame, Some(&principal)).await {
                    tracing::warn!(session = %session_id, %error, "failed to detach closed SSE session");
                }
            });
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn events(
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
            principal,
            session_id: stream_session_id.clone(),
        });
    let seen = Arc::new(std::sync::Mutex::new(
        initial
            .iter()
            .map(|frame| frame.id.clone())
            .collect::<std::collections::HashSet<_>>(),
    ));
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
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "INVALID_FRAME",
                    message: "session_id does not match path".to_owned(),
                },
            })),
        )
            .into_response();
    }
    let request_id = frame.id.clone();
    let request_session_id = frame.session_id.clone();
    // A message starts a turn, and a turn can outlive the request that started
    // it: a turn parked on `ask_user` does not end until a person answers, so
    // waiting for it to finish made the client's own request time out while
    // the turn was running perfectly well. `USER_INPUT` and `SEND_MESSAGE` are
    // answered the moment the input is accepted; everything after that arrives
    // on the event stream, which is where the client is already listening.
    let starts_a_turn = match &frame.kind {
        FrameType::Standard(StandardFrameType::UserInput) => true,
        FrameType::Extension(name) if name == crate::NorteCodeExtension::Command.as_str() => {
            serde_json::from_value::<norte_code_wire::ClientRequest>(frame.payload.clone())
                .is_ok_and(|request| {
                    matches!(
                        request.kind,
                        norte_code_wire::ClientRequestKind::SendMessage { .. }
                    )
                })
        }
        _ => false,
    };
    if starts_a_turn {
        let host_for_turn = Arc::clone(&host);
        let principal_for_turn = principal.clone();
        tokio::spawn(async move {
            let _ = host_for_turn
                .handle_as(frame, Some(&principal_for_turn))
                .await;
        });
        return (
            StatusCode::ACCEPTED,
            Json(payload(CommandResponsePayload {
                accepted: true,
                events: Some(Vec::new()),
                error: None,
            })),
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
            Json(payload(CommandResponsePayload {
                accepted: true,
                events: Some(events),
                error: None,
            })),
        )
            .into_response(),
        Err(_) => {
            let error = HostError::Service("command timed out".to_owned());
            (
                StatusCode::REQUEST_TIMEOUT,
                Json(correlated_error_frame(
                    &error,
                    Some(request_id),
                    request_session_id,
                )),
            )
                .into_response()
        }
        Ok(Err(error)) => {
            let status = error_status(&error);
            (
                status,
                Json(correlated_error_frame(
                    &error,
                    Some(request_id),
                    request_session_id,
                )),
            )
                .into_response()
        }
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
    correlated_error_frame(error, None, String::new())
}

fn correlated_error_frame(
    error: &HostError,
    reference: Option<FrameId>,
    session_id: String,
) -> Envelope<Value> {
    let code = error_code(error);
    let retryable = matches!(error, HostError::SessionBusy);
    let mut frame = Envelope::new(
        FrameType::Standard(StandardFrameType::Error),
        payload(raep_types::ErrorPayload {
            code: code.to_owned(),
            message: error.to_string(),
            retryable: Some(retryable),
            details: None,
        }),
        0,
    );
    frame.reference = reference;
    frame.session_id = session_id;
    frame
}

fn error_code(error: &HostError) -> &'static str {
    match error {
        HostError::UnknownType(_) => "UNKNOWN_TYPE",
        HostError::IdempotencyConflict(_) => "IDEMPOTENCY_CONFLICT",
        HostError::SessionBusy => "SESSION_BUSY",
        HostError::TurnNotActive => "TURN_NOT_ACTIVE",
        HostError::InvalidFrame(message) if message.starts_with("SESSION_BUSY:") => "SESSION_BUSY",
        HostError::InvalidFrame(message) if message.starts_with("TURN_NOT_ACTIVE:") => {
            "TURN_NOT_ACTIVE"
        }
        HostError::InvalidFrame(message) if message == "JOB_NOT_FOUND" => "JOB_NOT_FOUND",
        HostError::InvalidFrame(message) if message == "JOB_NOT_RUNNING" => "JOB_NOT_RUNNING",
        HostError::InvalidFrame(message) if message == "JOB_OUTPUT_EXPIRED" => "JOB_OUTPUT_EXPIRED",
        HostError::InvalidFrame(message) if message == "session not found" => "SESSION_NOT_FOUND",
        HostError::InvalidFrame(message) if message.contains("wire version") => {
            "UNSUPPORTED_VERSION"
        }
        HostError::InvalidFrame(_) => "INVALID_FRAME",
        HostError::Service(_) => "UNAVAILABLE",
        HostError::Replay(_) | HostError::ReplayJournal(_) => "REPLAY_TRUNCATED",
        HostError::Serialization(_) | HostError::Artifact(_) => "INTERNAL",
    }
}

fn error_status(error: &HostError) -> StatusCode {
    match error {
        HostError::UnknownType(_) => StatusCode::BAD_REQUEST,
        HostError::IdempotencyConflict(_) | HostError::SessionBusy | HostError::TurnNotActive => {
            StatusCode::CONFLICT
        }
        HostError::InvalidFrame(message) if message.starts_with("SESSION_BUSY:") => {
            StatusCode::CONFLICT
        }
        HostError::InvalidFrame(message) if message.contains("wire version") => {
            StatusCode::UPGRADE_REQUIRED
        }
        HostError::InvalidFrame(message) if message == "session not found" => StatusCode::NOT_FOUND,
        HostError::Service(_) => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

#[allow(clippy::result_large_err)]
fn origin_guard(host: &NorteCodeRaepHost, headers: &HeaderMap) -> Result<(), Response> {
    if let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok())
        && !host.origin_allowed(origin)
    {
        return Err((
            StatusCode::FORBIDDEN,
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "ORIGIN_NOT_ALLOWED",
                    message: "origin is not allowed".to_owned(),
                },
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
                Json(payload(RejectedPayload {
                    status: "error",
                    accepted: Some(false),
                    error: ErrorBody {
                        code: "UNAUTHENTICATED",
                        message: "valid bearer token required".to_owned(),
                    },
                })),
            )
                .into_response()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlobArtifactStore, HostConfig, MemoryUploadIndex};
    use axum::body::Body;
    use axum::http::Request;
    use norte_code_agent::Service;
    use norte_code_protocol::InMemoryStore;
    use norte_code_wire::{ClientRequest, ClientRequestKind};
    use raep_types::{Envelope, FrameType, StandardFrameType};
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

    #[test]
    fn byte_ranges_handle_closed_open_and_suffix_forms() {
        assert_eq!(parse_byte_range("bytes=2-4", 10), Ok(Some((2, 4))));
        assert_eq!(parse_byte_range("bytes=7-", 10), Ok(Some((7, 9))));
        assert_eq!(parse_byte_range("bytes=-3", 10), Ok(Some((7, 9))));
        assert!(parse_byte_range("bytes=10-", 10).is_err());
        assert!(parse_byte_range("bytes=0-1,4-5", 10).is_err());
    }

    #[tokio::test]
    async fn export_download_is_authenticated_range_capable_bundle() {
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
            .expect("authenticated host"),
        );
        let attached = host
            .handle_as(
                Envelope::new(
                    FrameType::Standard(StandardFrameType::SessionAttach),
                    json!({"agent_id": "norte-code", "create_if_missing": true}),
                    1,
                ),
                Some("alice"),
            )
            .await
            .expect("attach");
        let session_id = attached[0].session_id.clone();
        let mut export = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionExport),
            json!({"include": {
                "children": false,
                "uploads": false,
                "job_output": false,
                "outputs": false,
                "workspace": false
            }}),
            2,
        );
        export.session_id = session_id;
        let ready = host.handle_as(export, Some("alice")).await.expect("export");
        let export_id = ready[0].payload["export_id"].as_str().expect("export id");
        let app = router(Arc::clone(&host));
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/exports/{export_id}"))
                    .body(Body::empty())
                    .expect("unauthorized request"),
            )
            .await
            .expect("unauthorized response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let wrong_owner = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/exports/{export_id}"))
                    .header("authorization", "Bearer token-b")
                    .body(Body::empty())
                    .expect("wrong-owner request"),
            )
            .await
            .expect("wrong-owner response");
        assert_eq!(wrong_owner.status(), StatusCode::NOT_FOUND);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/exports/{export_id}"))
                    .header("range", "bytes=0-9")
                    .header("authorization", "Bearer token-a")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_RANGE],
            format!("bytes 0-9/{}", ready[0].payload["bytes"])
        );
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("range body");
        assert_eq!(body.len(), 10);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn text_bundle_preview_commit_restores_history_as_a_new_session() {
        let service = test_service();
        let source_key = service
            .import_plain_history(
                Some("source".to_owned()),
                vec![
                    norte_code_agent::service::ImportedHistoryItem {
                        role: "user".to_owned(),
                        content: "question".to_owned(),
                        turn_id: Some("turn_1".to_owned()),
                        timestamp: Some(1),
                        origin: Some(json!({"kind": "user"})),
                    },
                    norte_code_agent::service::ImportedHistoryItem {
                        role: "assistant".to_owned(),
                        content: "answer".to_owned(),
                        turn_id: Some("turn_1".to_owned()),
                        timestamp: Some(2),
                        origin: Some(json!({"kind": "assistant"})),
                    },
                ],
                json!({"session_id": "original"}),
                Some(json!({
                    "input_tokens": 11,
                    "output_tokens": 7,
                    "cached_tokens": 3,
                    "cost": {"amount": 0.25, "currency": "USD"},
                    "extra": {"requests": 2}
                })),
            )
            .await
            .expect("seed source history");
        let host = Arc::new(
            NorteCodeRaepHost::new(
                service.clone(),
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
        let source_id = source_key.to_string();
        let mut attach_source = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionAttach),
            json!({"agent_id": "norte-code", "create_if_missing": false}),
            1,
        );
        attach_source.session_id = source_id.clone();
        host.handle_as(attach_source, Some("alice"))
            .await
            .expect("attach source");
        let mut export = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionExport),
            json!({"include": {
                "children": false,
                "uploads": false,
                "job_output": false,
                "outputs": false,
                "workspace": false
            }}),
            2,
        );
        export.session_id = source_id.clone();
        let ready = host.handle_as(export, Some("alice")).await.expect("export");
        let export_id = ready[0].payload["export_id"].as_str().expect("export id");
        let (bundle, _) = host
            .get_export_as(export_id, "alice")
            .await
            .expect("bundle");
        let app = router(Arc::clone(&host));
        let preview = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/imports")
                    .header("content-type", "application/vnd.raep.bundle+zip")
                    .header("authorization", "Bearer token-a")
                    .body(Body::from(bundle))
                    .expect("preview request"),
            )
            .await
            .expect("preview response");
        assert_eq!(preview.status(), StatusCode::CREATED);
        let preview: Value = serde_json::from_slice(
            &axum::body::to_bytes(preview.into_body(), 1024 * 1024)
                .await
                .expect("preview body"),
        )
        .expect("preview JSON");
        let import_id = preview["import_id"].as_str().expect("import id");
        assert_eq!(preview["agents"][0]["available"], true);

        let wrong_owner_delete = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/raep/v1/imports/{import_id}"))
                    .header("authorization", "Bearer token-b")
                    .body(Body::empty())
                    .expect("wrong-owner delete request"),
            )
            .await
            .expect("wrong-owner delete response");
        assert_eq!(wrong_owner_delete.status(), StatusCode::NOT_FOUND);

        let wrong_owner_commit = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/raep/v1/imports/{import_id}/commit"))
                    .header("authorization", "Bearer token-b")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("wrong-owner commit request"),
            )
            .await
            .expect("wrong-owner commit response");
        assert_eq!(wrong_owner_commit.status(), StatusCode::NOT_FOUND);

        let committed = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/raep/v1/imports/{import_id}/commit"))
                    .header("authorization", "Bearer token-a")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("commit request"),
            )
            .await
            .expect("commit response");
        assert_eq!(committed.status(), StatusCode::OK);
        let result: Value = serde_json::from_slice(
            &axum::body::to_bytes(committed.into_body(), 1024 * 1024)
                .await
                .expect("commit body"),
        )
        .expect("commit JSON");
        let imported_id = result["session_id"].as_str().expect("new session id");
        assert_ne!(imported_id, source_id);
        assert_eq!(result["id_map"][source_id.clone()], imported_id);
        assert_eq!(
            service
                .session_usage_total(&norte_code_wire::SessionKey::new(imported_id))
                .expect("carried usage"),
            json!({
                "input_tokens": 11,
                "output_tokens": 7,
                "cached_tokens": 3,
                "cost": {"amount": 0.25, "currency": "USD"},
                "extra": {"requests": 2, "unpriced_requests": 0}
            })
        );
        assert_eq!(
            host.session_summary(imported_id, "alice")
                .await
                .expect("suspended import summary")["status"],
            "SUSPENDED"
        );

        let mut attach = Envelope::new(
            FrameType::Standard(StandardFrameType::SessionAttach),
            json!({"agent_id": "norte-code", "create_if_missing": false}),
            3,
        );
        attach.session_id = imported_id.to_owned();
        let state = host
            .handle_as(attach, Some("alice"))
            .await
            .expect("attach imported session");
        assert_eq!(state[1].payload["imported_from"]["session_id"], source_id);
        let mut history_request = Envelope::new(
            FrameType::Standard(StandardFrameType::HistoryGet),
            json!({"limit": 10}),
            4,
        );
        history_request.session_id = imported_id.to_owned();
        let history = host
            .handle_as(history_request, Some("alice"))
            .await
            .expect("imported history");
        assert_eq!(history[0].payload["items"].as_array().unwrap().len(), 2);
        assert_eq!(history[0].payload["items"][0]["content"], "question");
        assert_eq!(history[0].payload["items"][0]["origin"]["kind"], "imported");
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
    async fn session_uploads_are_authenticated_and_scoped() {
        let blobs = Arc::new(norte::InMemoryBlobStore::new());
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
            .expect("authenticated host")
            .with_artifact_store(Arc::new(BlobArtifactStore::new(
                Arc::clone(&blobs) as Arc<dyn norte::BlobStore>
            )))
            .with_upload_index(Arc::new(MemoryUploadIndex::default())),
        );
        let attached = host
            .handle_as(
                Envelope::new(
                    FrameType::Standard(StandardFrameType::SessionAttach),
                    json!({"agent_id": "norte-code", "create_if_missing": true}),
                    1,
                ),
                Some("alice"),
            )
            .await
            .expect("attach");
        let session_id = &attached[0].session_id;
        let app = router(host);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/raep/v1/sessions/{session_id}/uploads"))
                    .header("authorization", "Bearer token-a")
                    .header("content-type", "audio/wav")
                    .body(Body::from(b"audio bytes".to_vec()))
                    .expect("upload request"),
            )
            .await
            .expect("upload response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("upload body");
        let receipt: Value = serde_json::from_slice(&body).expect("upload receipt");
        assert!(
            receipt["upload_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("upl_"))
        );
        assert_eq!(receipt["mime_type"], "audio/wav");
        assert_eq!(receipt["bytes"], 11);

        let wrong_owner = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/raep/v1/sessions/{session_id}/uploads"))
                    .header("authorization", "Bearer token-b")
                    .header("content-type", "audio/wav")
                    .body(Body::from(b"audio bytes".to_vec()))
                    .expect("upload request"),
            )
            .await
            .expect("upload response");
        assert_eq!(wrong_owner.status(), StatusCode::NOT_FOUND);

        let missing_session = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/sessions/unknown/uploads")
                    .header("authorization", "Bearer token-a")
                    .header("content-type", "audio/wav")
                    .body(Body::from(b"audio bytes".to_vec()))
                    .expect("upload request"),
            )
            .await
            .expect("upload response");
        assert_eq!(missing_session.status(), StatusCode::NOT_FOUND);
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
        let preflight = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/raep/v1/sessions")
                    .header("origin", "https://app.example")
                    .header("access-control-request-method", "POST")
                    .header(
                        "access-control-request-headers",
                        "authorization,content-type",
                    )
                    .body(Body::empty())
                    .expect("preflight request"),
            )
            .await
            .expect("preflight response");
        assert_eq!(preflight.status(), StatusCode::OK);
        assert_eq!(
            preflight.headers().get("access-control-allow-origin"),
            Some(&HeaderValue::from_static("https://app.example"))
        );

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
        assert_eq!(
            allowed.headers().get("access-control-allow-origin"),
            Some(&HeaderValue::from_static("https://app.example"))
        );
    }

    #[tokio::test]
    async fn websocket_requires_the_raep_v1_subprotocol() {
        let app = router(test_host(None));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/raep/v1/ws")
                    .header("connection", "upgrade")
                    .header("upgrade", "websocket")
                    .header("sec-websocket-version", "13")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
    }

    #[tokio::test]
    async fn events_stream_starts_with_hello_and_reads_last_event_id() {
        let app = router(test_host(None));
        let session = create_session(&app).await;
        let attach = SessionAttach {
            agent_id: "norte-code".to_owned(),
            agent_override: None,
            create_if_missing: Some(false),
            mode: Some(AttachMode::Observer),
            takeover: None,
            resume_from_seq: Some(0),
            capabilities: None,
            client_info: None,
            options: None,
        };
        let encoded_attach =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&attach).expect("serialize attach"));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/raep/v1/sessions/{session}/events?mode=controller&attach={encoded_attach}"
                    ))
                    .header("accept", "text/event-stream")
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
        assert!(
            text.contains("retry: 2000"),
            "stream must configure the reconnect delay: {text}"
        );
        assert!(
            text.contains(": keepalive"),
            "stream must start with the keepalive comment: {text}"
        );
        assert!(
            text.contains("\"mode\":\"observer\""),
            "encoded attach payload must take precedence over individual query parameters: {text}"
        );
        drop(body);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let reattached = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/raep/v1/sessions/{session}/events"))
                    .header("accept", "text/event-stream")
                    .body(Body::empty())
                    .expect("reattach request"),
            )
            .await
            .expect("reattach response");
        assert_eq!(
            reattached.status(),
            StatusCode::OK,
            "closing an SSE stream must release its session attachment"
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
                    .header("accept", "text/event-stream")
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
                    .header("accept", "text/event-stream")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn events_stream_can_allocate_a_session() {
        let app = router(test_host(None));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/raep/v1/sessions/new/events?agent_id=norte-code")
                    .header("accept", "text/event-stream")
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
            if text.contains("event: SESSION_ATTACHED") {
                break;
            }
        }
        let attached = text
            .split("event: SESSION_ATTACHED\n")
            .nth(1)
            .and_then(|event| event.lines().find(|line| line.starts_with("data: ")))
            .and_then(|data| serde_json::from_str::<Value>(&data[6..]).ok())
            .expect("SESSION_ATTACHED envelope");
        assert!(
            attached["session_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "allocation must return the new session id: {attached}"
        );
    }

    #[tokio::test]
    async fn session_creation_uses_the_requested_agent() {
        let host = Arc::new(
            NorteCodeRaepHost::with_agents(
                std::collections::HashMap::from([
                    ("norte-code".to_owned(), test_service()),
                    ("alternate".to_owned(), test_service()),
                ]),
                HostConfig {
                    agent_id: "norte-code".to_owned(),
                    ..HostConfig::default()
                },
            )
            .expect("multi-agent host"),
        );
        let app = router(host);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/raep/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "agent_id": "alternate",
                            "title": "Agent-specific session"
                        })
                        .to_string(),
                    ))
                    .expect("create request"),
            )
            .await
            .expect("create response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        let session: Value = serde_json::from_slice(&body).expect("session response");
        assert_eq!(session["agent"]["id"], "alternate");
        assert_eq!(session["title"], "Agent-specific session");
    }

    #[tokio::test]
    async fn agent_resource_lists_reads_and_versions() {
        let app = router(test_host(None));
        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/raep/v1/agents")
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
            list["agents"]
                .as_array()
                .expect("agents")
                .iter()
                .any(|agent| agent["id"] == "norte-code")
        );

        let read = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/raep/v1/agents/norte-code")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(read.status(), StatusCode::OK);
        let body = axum::body::to_bytes(read.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let agent: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(agent["agent"]["id"], "norte-code");

        let versions = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/raep/v1/agents/norte-code/versions")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(versions.status(), StatusCode::OK);
        let body = axum::body::to_bytes(versions.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let versions: Value = serde_json::from_slice(&body).expect("json");
        assert_ne!(versions["versions"].as_array().expect("versions").len(), 0);

        let missing = app
            .oneshot(
                Request::builder()
                    .uri("/raep/v1/agents/absent")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
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

    #[tokio::test]
    async fn child_session_list_is_scoped_to_parent_owner() {
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

        let children = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/sessions/{session}/children"))
                    .header("authorization", "Bearer token-a")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(children.status(), StatusCode::OK);
        let body = axum::body::to_bytes(children.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let children: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(children["children"], json!([]));

        let hidden_children = app
            .oneshot(
                Request::builder()
                    .uri(format!("/raep/v1/sessions/{session}/children"))
                    .header("authorization", "Bearer token-b")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(hidden_children.status(), StatusCode::NOT_FOUND);
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
