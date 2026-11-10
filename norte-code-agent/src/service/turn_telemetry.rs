//! Recording what a turn's requests cost and used.

use chrono::Utc;

use norte::Usage;
use norte::id::SessionId;
use norte_code_protocol::sqlite::RequestStatus;
use norte_code_wire::SessionKey;

use super::Service;
use super::mapping::reported_cost;
use crate::provider::ModelSpec;

impl Service {
    /// Takes the requests the telemetry layer finished since the last look.
    pub(super) fn telemetry_for(
        &self,
        key: &SessionKey,
        nid: SessionId,
    ) -> Vec<norte_code_protocol::sqlite::RequestRecord> {
        let mut records = self.telemetry.take(nid);
        records.sort_by_key(|record| record.started_at);
        for record in &mut records {
            record.session = key.to_string();
        }
        records
    }

    pub(super) async fn record_request(
        &self,
        key: &SessionKey,
        record: &norte_code_protocol::sqlite::RequestRecord,
    ) {
        if let Err(error) = self.directory.record_request(record).await {
            tracing::warn!(session = %key, %error, "could not record request telemetry");
        }
    }

    pub(super) fn request_from_usage(
        &self,
        key: &SessionKey,
        nid: SessionId,
        turn: u64,
        usage: &Usage,
    ) -> norte_code_protocol::sqlite::RequestRecord {
        // Both halves come from the session's own spec, which carries them
        // apart. They used to be recovered by splitting the label, which put an
        // empty provider on the row whenever a model id carried a slash of its
        // own: the label is for people to read, not for the host to take apart.
        let (provider, model) = self.lock_sessions().get(key).map_or_else(
            || (String::new(), String::new()),
            |record| {
                (
                    record
                        .spec
                        .as_ref()
                        .map(|s| s.provider.clone())
                        .unwrap_or_default(),
                    record
                        .spec
                        .as_ref()
                        .map(|s| s.model.clone())
                        .unwrap_or_default(),
                )
            },
        );
        norte_code_protocol::sqlite::RequestRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session: key.to_string(),
            north_session: nid.to_string(),
            turn,
            provider,
            model,
            endpoint: None,
            started_at: Utc::now(),
            duration_ms: 0,
            first_token_ms: None,
            last_token_ms: None,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            estimated_input_tokens: None,
            cache_hit: usage.cache_read_tokens.unwrap_or(0) > 0,
            tool_calls: Vec::new(),
            reasoning: None,
            sandbox: self.config().tools.confinement.sandbox.enabled,
            sandbox_network: self.config().tools.confinement.sandbox.network,
            params: serde_json::Value::Null,
            extensions: serde_json::Value::Null,
            metadata: serde_json::json!({"source": "history"}),
            cost_usd: reported_cost(usage),
            finish_reason: None,
            status: RequestStatus::Ok,
            error: None,
            error_kind: None,
            retry_after_ms: None,
        }
    }

    /// The `provider/model` a session currently uses.
    pub(super) fn model_label(&self, key: &SessionKey) -> String {
        self.lock_sessions()
            .get(key)
            .map_or_else(String::new, |record| {
                record
                    .spec
                    .as_ref()
                    .map(ModelSpec::label)
                    .unwrap_or_default()
            })
    }
}
