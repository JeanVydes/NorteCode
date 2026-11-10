//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use crate::NorteCodeExtension;
use crate::payloads::payload;
use norte_code_wire::{ClientRequest, ClientRequestKind, SessionKey};
use raep_types::{Envelope, FrameType, StandardFrameType, WIRE_VERSION};
use serde_json::Value;
use std::collections::VecDeque;

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::capabilities::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::payloads::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::stores::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;

impl NorteCodeRaepHost {
    /// Returns the `agent_summary` for every agent this host serves.
    #[must_use]
    pub fn agents(&self) -> Vec<Value> {
        let mut ids = self.services.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let spec = self
                    .services
                    .get(&id)
                    .and_then(|service| service.default_spec());
                payload(norte_code_wire::AgentSummary {
                    id: id.clone(),
                    version: self.config.server_version.clone(),
                    capabilities: agent_capabilities(spec.as_ref(), self.uploads_enabled()),
                    provider: spec.as_ref().map(|spec| norte_code_wire::AgentRef {
                        id: spec.provider.clone(),
                        version: String::new(),
                    }),
                    model: spec.as_ref().map(|spec| norte_code_wire::AgentRef {
                        id: spec.model.clone(),
                        version: String::new(),
                    }),
                })
            })
            .collect()
    }
    /// Returns one `agent_summary` and its latest version, or `None` when the
    /// agent is unknown.
    #[must_use]
    pub fn agent(&self, agent_id: &str) -> Option<Value> {
        let service = self.services.get(agent_id)?;
        let spec = service.default_spec();
        Some(payload(AgentDetail {
            agent: norte_code_wire::AgentSummary {
                id: agent_id.to_owned(),
                version: self.config.server_version.clone(),
                capabilities: agent_capabilities(spec.as_ref(), self.uploads_enabled()),
                provider: spec.as_ref().map(|spec| norte_code_wire::AgentRef {
                    id: spec.provider.clone(),
                    version: String::new(),
                }),
                model: spec.as_ref().map(|spec| norte_code_wire::AgentRef {
                    id: spec.model.clone(),
                    version: String::new(),
                }),
            },
            latest_version: AgentVersion {
                version: &self.config.server_version,
                created_at: 0,
                deprecated: false,
            },
        }))
    }
    /// Returns the versions an agent has, or `None` when the agent is unknown.
    #[must_use]
    pub fn agent_versions(&self, agent_id: &str) -> Option<Value> {
        self.services.get(agent_id)?;
        Some(payload(AgentVersions {
            versions: vec![AgentVersion {
                version: &self.config.server_version,
                created_at: 0,
                deprecated: false,
            }],
        }))
    }
    /// Returns quota for one principal.
    ///
    /// Norte Code has no tenant quota store yet, so the answer is derived
    /// from what the host knows: sessions the principal owns, turns, and
    /// replay frames. The limits are the host's own configured limits;
    /// `used`/`remaining` come from in-memory counters.
    #[must_use]
    pub async fn quota(&self, principal: &str) -> Value {
        let (sessions, replay_frames) = {
            let state = self.state.lock().await;
            let sessions = state
                .session_owners
                .values()
                .filter(|owner| *owner == principal)
                .count() as u64;
            let frames = state.replay.values().map(VecDeque::len).sum::<usize>() as u64;
            (sessions, frames)
        };
        let session_limit = 100_u64;
        let frame_limit = self.config.replay_capacity as u64;
        payload(norte_code_wire::QuotaReport {
            tenant: principal.to_owned(),
            period: norte_code_wire::QuotaPeriod { start: 0, end: 0 },
            dimensions: vec![
                norte_code_wire::QuotaDimension {
                    name: norte_code_wire::QuotaDimensionName::Sessions,
                    limit: session_limit,
                    used: sessions,
                    remaining: session_limit.saturating_sub(sessions),
                },
                norte_code_wire::QuotaDimension {
                    name: norte_code_wire::QuotaDimensionName::ReplayBufferEvents,
                    limit: frame_limit,
                    used: replay_frames,
                    remaining: frame_limit.saturating_sub(replay_frames),
                },
            ],
        })
    }
    /// Returns the RAEP handshake frame.
    #[must_use]
    pub fn hello(&self) -> Envelope<Value> {
        Envelope::new(
            standard(StandardFrameType::Hello),
            payload(HelloPayload {
                protocol: ProtocolRef {
                    name: "RAEP",
                    version: WIRE_VERSION,
                },
                server_version: &self.config.server_version,
                wire_versions: vec![WIRE_VERSION],
                capabilities: advertised_capabilities(self.uploads_enabled()),
                extensions: vec![NorteCodeExtension::Command.as_str()],
                limits: LimitsRef {
                    max_frame_bytes: self.config.max_frame_bytes,
                    replay_buffer_events: self.config.replay_capacity,
                    max_concurrent_jobs: self.config.max_concurrent_jobs,
                    job_output_retention_s: self.config.job_output_retention_s,
                    max_artifact_bytes: self.config.max_artifact_bytes,
                    max_upload_bytes: self.config.max_artifact_bytes,
                    max_inline_bytes: 0,
                    max_output_part_bytes: self.output_part_bytes_limit(),
                    max_prompt_bytes: 256 * 1024,
                    max_parts: 16,
                    max_queued_inputs: self.config.max_queued_inputs,
                    max_export_bytes: self.config.max_export_bytes,
                    export_ttl_s: self.config.export_ttl_s,
                },
                heartbeat_s: 20,
                draining: false,
                deprecations: Vec::new(),
            }),
            now_millis(),
        )
    }
    /// Returns discovery information for an HTTP info route.
    #[must_use]
    pub fn info(&self) -> Value {
        let default_model = match self
            .services
            .get(&self.config.agent_id)
            .and_then(|service| service.default_spec())
        {
            Some(spec) => payload(ModelRef {
                provider: &spec.provider,
                model: &spec.model,
            }),
            None => Value::Null,
        };
        payload(InfoPayload {
            protocol: ProtocolRef {
                name: "RAEP",
                version: WIRE_VERSION,
            },
            server_version: &self.config.server_version,
            wire_versions: vec![WIRE_VERSION],
            capabilities: advertised_capabilities(self.uploads_enabled()),
            extensions: vec![NorteCodeExtension::Command.as_str()],
            limits: LimitsRef {
                max_frame_bytes: self.config.max_frame_bytes,
                replay_buffer_events: self.config.replay_capacity,
                max_concurrent_jobs: self.config.max_concurrent_jobs,
                job_output_retention_s: self.config.job_output_retention_s,
                max_artifact_bytes: self.config.max_artifact_bytes,
                max_upload_bytes: self.config.max_artifact_bytes,
                max_inline_bytes: 0,
                max_output_part_bytes: self.output_part_bytes_limit(),
                max_prompt_bytes: 256 * 1024,
                max_parts: 16,
                max_queued_inputs: self.config.max_queued_inputs,
                max_export_bytes: self.config.max_export_bytes,
                export_ttl_s: self.config.export_ttl_s,
            },
            agents: self.services.keys().cloned().collect::<Vec<_>>(),
            default_agent: &self.config.agent_id,
            default_model,
        })
    }
    /// Returns readiness and basic operational counters for health probes.
    #[must_use]
    pub fn readiness(&self) -> Value {
        payload(ReadinessPayload {
            status: ReadinessStatus::Ready,
            agent_id: &self.config.agent_id,
            agents: self.services.len(),
            replay: self.replay_store.is_some(),
            artifacts: self.artifact_store.is_some(),
            limits: ReadinessLimits {
                max_connections: self.config.max_connections,
                command_timeout_ms: self.config.command_timeout_ms,
            },
        })
    }
    /// Returns a structured telemetry report for operational dashboards.
    ///
    /// The command remains the source of truth for the records. This method
    /// adds stable aggregate counters without changing the streaming wire
    /// event consumed by existing clients.
    ///
    /// # Errors
    ///
    /// Returns an error when the telemetry command cannot be routed or its
    /// response cannot be serialized.
    pub async fn telemetry_report(
        &self,
        session: Option<&str>,
        principal: &str,
    ) -> Result<Value, HostError> {
        let session_key = session.map(SessionKey::new);
        let request = ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::ListTelemetry {
                session: session_key,
            },
        };
        let frame = Envelope::new(
            FrameType::Extension(NorteCodeExtension::Command.as_str().to_owned()),
            serde_json::to_value(request)?,
            now_millis(),
        );
        let frames = self.handle_as(frame, Some(principal)).await?;
        // The telemetry records are the service's own `RequestRecord` struct
        // inside the canonical `service_data` payload: decode them through
        // the type instead of reading token counters by string key.
        let records: Vec<norte_code_protocol::sqlite::RequestRecord> = frames
            .iter()
            .find_map(|frame| {
                let event: CanonicalServiceData =
                    serde_json::from_value(frame.payload.clone()).ok()?;
                (event.kind == ServiceDataKind::Telemetry).then_some(event.data)
            })
            .and_then(|data| serde_json::from_value(data).ok())
            .unwrap_or_default();
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut cache_read_tokens = 0_u64;
        let mut cache_write_tokens = 0_u64;
        let mut errors = 0_u64;
        let mut cancelled = 0_u64;
        let mut duration_ms = 0_u64;
        for row in &records {
            input_tokens += row.input_tokens.unwrap_or(0);
            output_tokens += row.output_tokens.unwrap_or(0);
            cache_read_tokens += row.cache_read_tokens.unwrap_or(0);
            cache_write_tokens += row.cache_write_tokens.unwrap_or(0);
            duration_ms += row.duration_ms;
            match row.status {
                norte_code_protocol::sqlite::RequestStatus::Error => errors += 1,
                norte_code_protocol::sqlite::RequestStatus::Cancelled => cancelled += 1,
                norte_code_protocol::sqlite::RequestStatus::Ok => {}
            }
        }
        let request_count = records.len();
        Ok(payload(norte_code_wire::TelemetryReport {
            session: session.map(str::to_owned),
            records: records.iter().map(Into::into).collect(),
            summary: norte_code_wire::TelemetrySummary {
                requests: u64::try_from(request_count).unwrap_or(u64::MAX),
                errors,
                cancelled,
                duration_ms,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
            },
        }))
    }
}
