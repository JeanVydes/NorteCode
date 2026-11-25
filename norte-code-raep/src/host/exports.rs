//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use crate::payloads::payload;
use norte_code_agent::Service;
use norte_code_wire::SessionKey;
use raep_types::{Envelope, StandardFrameType, WIRE_VERSION};
use serde_json::Value;
use std::collections::BTreeMap;

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

pub(super) fn purge_expired_exports(state: &mut super::stores::HostState) {
    let now = now_millis();
    let expired: Vec<_> = state
        .exports
        .iter()
        .filter(|(_, export)| export.expires_at <= now)
        .map(|(_, export)| export.path.clone())
        .collect();
    for path in expired {
        bundle_store::remove(&path);
    }
    state.exports.retain(|_, export| export.expires_at > now);
}

impl NorteCodeRaepHost {
    pub(crate) async fn export(
        &self,
        frame: Envelope<Value>,
        principal: Option<&str>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: ExportRequest = serde_json::from_value(frame.payload)?;
        validate_export_request(&request)?;
        let service = self.service_for_session(&frame.session_id)?;
        let bytes = self.build_text_bundle(&service, &frame.session_id).await?;
        let (export_id, expires_at, byte_count, digest) =
            self.store_export(bytes, principal).await?;
        let ready = self
            .emit(
                &frame.session_id,
                StandardFrameType::SessionExportReady,
                payload(ExportReadyPayload {
                    export_id: &export_id,
                    download_url: format!("/raep/v1/exports/{export_id}"),
                    bytes: byte_count,
                    sha256: &digest,
                    expires_at,
                    sessions: 1,
                    encrypted: false,
                    signed: false,
                }),
            )
            .await?;
        Ok(vec![ready])
    }
    pub(crate) async fn build_text_bundle(
        &self,
        service: &Service,
        session_id: &str,
    ) -> Result<Vec<u8>, HostError> {
        let key = SessionKey::new(session_id.to_owned());
        let session = service
            .sessions()
            .into_iter()
            .find(|session| session.session.as_str() == session_id)
            .ok_or_else(|| HostError::InvalidFrame("session not found".to_owned()))?;
        if session.phase != norte_code_wire::Phase::Idle || service.has_running_jobs(&key) {
            return Err(HostError::InvalidFrame(
                "SESSION_BUSY: export requires an idle session with no running jobs".to_owned(),
            ));
        }
        if !service
            .session_rewound_history_with_turns(&key)
            .map_err(|error| HostError::Service(error.to_string()))?
            .is_empty()
        {
            return Err(HostError::InvalidFrame(
                "SESSION_EXPORT cannot preserve rewound history on this host".to_owned(),
            ));
        }
        let session_agent_id = self.session_owner(session_id);
        let history = service
            .session_history_with_provenance(&key)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        let history_bytes = serialize_plain_text_history(history, self.config.max_export_bytes)?;
        let session_path = format!("sessions/{session_id}/session.json");
        let history_path = format!("sessions/{session_id}/history.jsonl");
        let agent = payload(BundleAgent {
            id: &session_agent_id,
        });
        let session_json = serde_json::to_vec(&PortableSessionFile {
            agent: BundleAgent {
                id: &session_agent_id,
            },
            title: &session.title,
            options: BTreeMap::new(),
        })?;
        let files = std::collections::BTreeMap::from([
            (session_path, session_json),
            (history_path.clone(), history_bytes),
        ]);
        let manifest = crate::bundle::BundleManifest {
            bundle_version: "1".to_owned(),
            raep_document_version: None,
            wire_version: Some(WIRE_VERSION.to_owned()),
            exported_at: u64::try_from(now_millis()).unwrap_or_default(),
            exporter: Some(payload(BundleExporter {
                server: &self.config.server_version,
            })),
            root_session_id: session_id.to_owned(),
            sessions: vec![crate::bundle::BundleSession {
                session_id: session_id.to_owned(),
                parent_session_id: None,
                agent,
                state: Some("IDLE".to_owned()),
                history_file: history_path,
                last_seq: Some(self.current_seq(session_id).await),
                usage_total: service.session_usage_total(&key),
            }],
            files: Vec::new(),
            excluded: vec![
                "children".to_owned(),
                "uploads".to_owned(),
                "job_output".to_owned(),
                "outputs".to_owned(),
                "workspace".to_owned(),
                "system_prompt".to_owned(),
                "credentials".to_owned(),
            ],
        };
        let bytes = crate::bundle::create_bundle(manifest, files)
            .map_err(|error| HostError::InvalidFrame(error.to_string()))?;
        if bytes.len() > self.config.max_export_bytes {
            return Err(HostError::InvalidFrame(
                "PAYLOAD_TOO_LARGE: export exceeds max_export_bytes".to_owned(),
            ));
        }
        Ok(bytes)
    }
    pub(crate) async fn store_export(
        &self,
        bytes: Vec<u8>,
        principal: Option<&str>,
    ) -> Result<(String, i64, usize, String), HostError> {
        let byte_count = bytes.len();
        let digest = crate::bundle::sha256(&bytes);
        let export_id = format!("exp_{}", uuid::Uuid::new_v4().simple());
        let expires_at = now_millis().saturating_add(
            i64::try_from(self.config.export_ttl_s.saturating_mul(1000)).unwrap_or(i64::MAX),
        );
        let path = bundle_store::write_bytes(&self.config.replay_dir.join("exports"), &bytes)?;
        {
            let mut state = self.state.lock().await;
            purge_expired_exports(&mut state);
            if state.exports.len() >= 4 {
                bundle_store::remove(&path);
                return Err(HostError::InvalidFrame(
                    "EXPORT_STORE_FULL: active export limit reached".to_owned(),
                ));
            }
            state.exports.insert(
                export_id.clone(),
                StoredExport {
                    path,
                    owner: principal.map(str::to_owned),
                    expires_at,
                },
            );
        }
        Ok((export_id, expires_at, byte_count, digest))
    }
}

pub(crate) fn validate_export_request(request: &ExportRequest) -> Result<(), HostError> {
    if request
        .quiesce
        .is_some_and(|policy| policy != QuiescePolicy::Reject)
    {
        return Err(HostError::InvalidFrame(
            "SESSION_EXPORT currently supports only quiesce=reject".to_owned(),
        ));
    }
    if request.wait_timeout_s.is_some() {
        return Err(HostError::InvalidFrame(
            "wait_timeout_s is only valid with quiesce=wait".to_owned(),
        ));
    }
    if request.encrypt.is_some() {
        return Err(HostError::InvalidFrame(
            "encrypted exports are not supported".to_owned(),
        ));
    }
    if request.include.children
        || request.include.uploads
        || request.include.job_output
        || request.include.outputs
        || request.include.workspace
    {
        return Err(HostError::InvalidFrame(
            "SESSION_EXPORT requires children, uploads, job_output, outputs, and workspace all false"
                .to_owned(),
        ));
    }
    Ok(())
}

pub(crate) type PortableHistoryEntry = (
    Option<u64>,
    norte_code_wire::MessageRole,
    norte_code_wire::ReplayedKind,
    String,
    i64,
    Option<Value>,
);

pub(crate) fn serialize_plain_text_history(
    history: Vec<PortableHistoryEntry>,
    max_bytes: usize,
) -> Result<Vec<u8>, HostError> {
    let mut bytes = Vec::new();
    for (turn, role, kind, content, timestamp, origin) in history {
        // The bundle carries text, and a conversation is more than text: the
        // model reasons, calls tools, and reads their results. Exporting only
        // refused those, so any real session could not be exported at all.
        // Each non-text part is written as the assistant text it already
        // renders as (`name {json}` for a call), and tool output is dropped
        // because it is reproducible from the call.
        let role = match (role, kind) {
            (norte_code_wire::MessageRole::User, norte_code_wire::ReplayedKind::Text) => {
                ExportHistoryRole::User
            }
            (norte_code_wire::MessageRole::Assistant, _) => ExportHistoryRole::Assistant,
            // A user row that is not text has no counterpart in the bundle's
            // two roles; tool output is reproducible from the call; and the
            // system prompt is not the conversation. All of them are dropped
            // rather than failing the whole export.
            _ => continue,
        };
        let item = HistoryExportItem {
            role,
            content: &content,
            turn_id: turn.map(|turn| format!("turn_{turn}")),
            timestamp,
            origin,
        };
        serde_json::to_writer(&mut bytes, &item)?;
        bytes.push(b'\n');
        if bytes.len() > max_bytes {
            return Err(HostError::InvalidFrame(
                "PAYLOAD_TOO_LARGE: export exceeds max_export_bytes".to_owned(),
            ));
        }
    }
    Ok(bytes)
}
