//! Axum bindings for the Norte Code RAEP host.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use raep_types::{Envelope, FrameId, FrameType, StandardFrameType};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::sync::Arc;
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    timeout::RequestBodyTimeoutLayer,
    trace::TraceLayer,
};

use crate::host::turn_handles::TurnKey;
use crate::payloads::payload;
use crate::{HostError, NorteCodeRaepHost};

mod artifacts;
mod events;
mod resources;
mod seen_frames;
mod websocket;

use artifacts::{
    commit_import, delete_import, download_artifact, download_export, download_session_output,
    import_bundle, upload_artifact, upload_session_upload,
};
use resources::{
    agent_versions, create_session, delete_session, list_agents, list_child_sessions,
    list_sessions, patch_session, read_agent, read_session,
};
use websocket::{agent_commands, commands, websocket};

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
        .route("/raep/v1/sessions/{session_id}/events", get(events::events))
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
