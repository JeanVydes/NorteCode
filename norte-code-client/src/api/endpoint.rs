//! The closed set of HTTP endpoints the host serves.

use reqwest::Method;
use serde::Serialize;
use serde_json::Value;

use super::ApiError;

/// A `SESSION_CREATE` body.
#[derive(Clone, Debug, Serialize)]
pub struct CreateSessionRequest<'a> {
    /// The agent the session runs.
    pub agent_id: &'a str,
    /// Its label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<&'a str>,
    /// The model it starts on, applied before the first turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_override: Option<AgentOverride<'a>>,
}

/// The model a session starts on.
#[derive(Clone, Debug, Serialize)]
pub struct AgentOverride<'a> {
    /// The model reference.
    pub model: AgentModelRef<'a>,
    /// The provider that serves it.
    pub provider: &'a str,
}

/// The model half of an override.
#[derive(Clone, Debug, Serialize)]
pub struct AgentModelRef<'a> {
    /// The model id.
    pub id: &'a str,
}

/// A `PATCH /sessions/{id}` body.
#[derive(Clone, Debug, Serialize)]
pub struct RenameSessionRequest<'a> {
    /// The new label.
    pub title: &'a str,
}

/// The query of a telemetry read.
#[derive(Clone, Copy, Debug, Default)]
pub struct TelemetryQuery<'a> {
    /// Only the rows of this session, when named.
    pub session: Option<&'a str>,
}

/// The query of an event stream.
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionEventsRequest<'a> {
    /// The session to attach to.
    pub session: &'a str,
    /// The agent, when the client names one.
    pub agent: Option<&'a str>,
}

/// Every HTTP endpoint the host serves.
///
/// Data-bearing variants carry their request struct, so a path can never be
/// built with the wrong body and a new endpoint is one variant.
#[derive(Clone, Debug)]
pub enum Endpoint<'a> {
    /// Server description and negotiated protocol.
    Info,
    /// Liveness probe.
    HealthLive,
    /// Readiness probe.
    HealthReady,
    /// The principal's quota.
    Quota,
    /// Request telemetry.
    Telemetry(TelemetryQuery<'a>),
    /// The agents the host exposes.
    Agents,
    /// One agent.
    Agent {
        /// The agent id.
        id: &'a str,
    },
    /// The sessions the principal can see.
    Sessions,
    /// Creates a session.
    CreateSession(CreateSessionRequest<'a>),
    /// One session.
    Session {
        /// The session id.
        id: &'a str,
    },
    /// Renames a session.
    RenameSession {
        /// The session id.
        id: &'a str,
        /// The new title.
        request: RenameSessionRequest<'a>,
    },
    /// Deletes a session.
    DeleteSession {
        /// The session id.
        id: &'a str,
    },
    /// The children of a session.
    ChildSessions {
        /// The session id.
        id: &'a str,
    },
    /// The event stream of a session.
    SessionEvents(SessionEventsRequest<'a>),
    /// The command endpoint of a session.
    SessionCommands {
        /// The session id.
        id: &'a str,
    },
    /// The command endpoint of the default agent.
    Commands,
    /// Uploads one artifact into a session.
    SessionUpload {
        /// The session id.
        id: &'a str,
    },
    /// Uploads one artifact without a session.
    Artifacts,
    /// Downloads an artifact.
    Artifact {
        /// The artifact id.
        id: &'a str,
    },
    /// Downloads an exported bundle.
    Export {
        /// The export id.
        id: &'a str,
    },
    /// Imports a bundle.
    Imports,
    /// Commits an import.
    ImportCommit {
        /// The import id.
        id: &'a str,
    },
    /// Deletes an import without committing it.
    DeleteImport {
        /// The import id.
        id: &'a str,
    },
}

impl Endpoint<'_> {
    /// The HTTP method of the endpoint.
    #[must_use]
    pub fn method(&self) -> Method {
        match self {
            Self::CreateSession(_)
            | Self::SessionCommands { .. }
            | Self::Commands
            | Self::SessionUpload { .. }
            | Self::Artifacts
            | Self::Imports
            | Self::ImportCommit { .. } => Method::POST,
            Self::RenameSession { .. } => Method::PATCH,
            Self::DeleteSession { .. } | Self::DeleteImport { .. } => Method::DELETE,
            Self::Info
            | Self::HealthLive
            | Self::HealthReady
            | Self::Quota
            | Self::Telemetry(_)
            | Self::Agents
            | Self::Agent { .. }
            | Self::Sessions
            | Self::Session { .. }
            | Self::ChildSessions { .. }
            | Self::SessionEvents(_)
            | Self::Artifact { .. }
            | Self::Export { .. } => Method::GET,
        }
    }

    /// The path the endpoint lives at, relative to the server root.
    #[must_use]
    pub fn path(&self) -> String {
        match self {
            Self::Info => "/raep/v1/info".to_owned(),
            Self::HealthLive => "/raep/v1/health/live".to_owned(),
            Self::HealthReady => "/raep/v1/health/ready".to_owned(),
            Self::Quota => "/raep/v1/quota".to_owned(),
            Self::Telemetry(query) => query.session.map_or_else(
                || "/raep/v1/telemetry".to_owned(),
                |session| format!("/raep/v1/telemetry?session={}", encode(session)),
            ),
            Self::Agents => "/raep/v1/agents".to_owned(),
            Self::Agent { id } => format!("/raep/v1/agents/{}", encode(id)),
            Self::Sessions | Self::CreateSession(_) => "/raep/v1/sessions".to_owned(),
            Self::Session { id } | Self::RenameSession { id, .. } | Self::DeleteSession { id } => {
                format!("/raep/v1/sessions/{}", encode(id))
            }
            Self::ChildSessions { id } => {
                format!("/raep/v1/sessions/{}/children", encode(id))
            }
            Self::SessionEvents(request) => {
                let mut path = format!(
                    "/raep/v1/sessions/{}/events?create_if_missing=false",
                    encode(request.session)
                );
                if let Some(agent) = request.agent.filter(|agent| !agent.trim().is_empty()) {
                    path.push_str("&agent_id=");
                    path.push_str(&encode(agent));
                }
                path
            }
            Self::SessionCommands { id } => {
                format!("/raep/v1/sessions/{}/commands", encode(id))
            }
            Self::Commands => "/raep/v1/commands".to_owned(),
            Self::SessionUpload { id } => {
                format!("/raep/v1/sessions/{}/uploads", encode(id))
            }
            Self::Artifacts => "/raep/v1/artifacts".to_owned(),
            Self::Artifact { id } => format!("/raep/v1/artifacts/{}", encode(id)),
            Self::Export { id } => format!("/raep/v1/exports/{}", encode(id)),
            Self::Imports => "/raep/v1/imports".to_owned(),
            Self::ImportCommit { id } => format!("/raep/v1/imports/{}/commit", encode(id)),
            Self::DeleteImport { id } => format!("/raep/v1/imports/{}", encode(id)),
        }
    }

    /// The JSON body, when the endpoint carries one.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::InvalidResponse`] when a request struct cannot be
    /// serialized, which a closed struct cannot do.
    pub fn body(&self) -> Result<Option<Value>, ApiError> {
        let body = match self {
            Self::CreateSession(request) => Some(
                serde_json::to_value(request)
                    .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            ),
            Self::RenameSession { request, .. } => Some(
                serde_json::to_value(request)
                    .map_err(|error| ApiError::InvalidResponse(error.to_string()))?,
            ),
            _ => None,
        };
        Ok(body)
    }
}

/// Percent-encodes a path or query segment.
fn encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}
