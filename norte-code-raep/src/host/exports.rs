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

impl NorteCodeRaepHost {
    pub(crate) async fn export(
        &self,
        frame: Envelope<Value>,
        principal: Option<&str>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: ExportRequest = serde_json::from_value(frame.payload)?;
        validate_export_request(&request)?;
        let service = self.service_for_session(&frame.session_id).await?;
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
        let session_agent_id = self
            .state
            .lock()
            .await
            .session_agents
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| self.config.agent_id.clone());
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
        {
            let mut state = self.state.lock().await;
            state
                .exports
                .retain(|_, export| export.expires_at > now_millis());
            if state.exports.len() >= 4 {
                return Err(HostError::InvalidFrame(
                    "EXPORT_STORE_FULL: active export limit reached".to_owned(),
                ));
            }
            state.exports.insert(
                export_id.clone(),
                StoredExport {
                    bytes,
                    owner: principal.map(str::to_owned),
                    expires_at,
                },
            );
        }
        Ok((export_id, expires_at, byte_count, digest))
    }
}
