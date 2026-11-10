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
        .route(
            "/raep/v1/commands",
            post(agent_commands).layer(RequestBodyLimitLayer::new(limits.max_frame_bytes)),
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
    let agent_override = body.get("agent_override").filter(|value| !value.is_null());
    let session = match host
        .create_session(agent_id, title, agent_override, &principal)
        .await
    {
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
    let result = tokio::time::timeout(
        host.command_timeout(),
        host.handle_as(frame, Some(&principal)),
    )
    .await;
    command_reply(result, request_id, request_session_id)
}

/// Handles a command that is not scoped to a session.
///
/// Catalogue writes, plugin operations, skills and MCP listing belong to the
/// agent, and a client used to reach them through a placeholder session id
/// that existed nowhere. This route takes the frame as it is: a kind that does
/// need a session answers with the service's own error.
async fn agent_commands(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Json(frame): Json<Envelope<Value>>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let request_id = frame.id.clone();
    let request_session_id = frame.session_id.clone();
    let result = tokio::time::timeout(
        host.command_timeout(),
        host.handle_as(frame, Some(&principal)),
    )
    .await;
    command_reply(result, request_id, request_session_id)
}

fn command_reply(
    result: Result<Result<Vec<Envelope<Value>>, HostError>, tokio::time::error::Elapsed>,
    request_id: raep_types::FrameId,
    request_session_id: String,
) -> Response {
    match result {
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
#[path = "http_tests.rs"]
mod tests;
