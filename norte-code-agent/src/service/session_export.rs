//! Exporting, importing and forking a session.

use chrono::Utc;

use norte_code_wire::{Phase as ProtocolPhase, SessionKey};

use crate::provider::ModelSpec as ProviderModelSpec;

use super::session_record::{SessionRecord, TurnBoundary};
use super::{Service, ServiceError};

impl Service {
    /// Returns the usage counters in the portable RAEP usage shape.
    #[must_use]
    pub fn session_usage_total(&self, key: &SessionKey) -> Option<serde_json::Value> {
        let sessions = self.lock_sessions();
        let record = sessions.get(key)?;
        Some(serde_json::json!({
            "input_tokens": record.input_tokens,
            "output_tokens": record.output_tokens,
            "cached_tokens": record.cache_read_tokens.saturating_add(record.cache_write_tokens),
            "cost": {
                "amount": record.cost.max(0.0),
                "currency": "USD",
            },
            "extra": {
                "requests": record.requests,
                "unpriced_requests": record.unpriced,
            },
        }))
    }

    /// Returns importer provenance stored with a portable session.
    #[must_use]
    pub fn session_imported_from(&self, key: &SessionKey) -> Option<serde_json::Value> {
        self.lock_sessions()
            .get(key)
            .and_then(|record| record.imported_from.clone())
    }

    /// Whether a portable session is waiting for its first controller attach.
    #[must_use]
    pub fn session_imported_suspended(&self, key: &SessionKey) -> bool {
        self.lock_sessions()
            .get(key)
            .is_some_and(|record| record.imported_suspended)
    }

    /// Activates an imported session when its first controller attaches.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is unknown or its directory record
    /// cannot be updated.
    #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
    pub(super) async fn export_session(
        &self,
        _request_id: u64,
        _session: &SessionKey,
        _include: Option<norte_code_wire::ExportInclude>,
        _quiesce: Option<norte_code_wire::ExportQuiesce>,
    ) -> Result<crate::EventStream, crate::ServiceError> {
        Err(crate::ServiceError::Unsupported(
            "session export not implemented",
        ))
    }

    #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
    pub(super) async fn import_session(
        &self,
        _request_id: u64,
        _bundle: serde_json::Value,
        _decryption_jwk: Option<serde_json::Value>,
    ) -> Result<crate::EventStream, crate::ServiceError> {
        Err(crate::ServiceError::Unsupported(
            "session import not implemented",
        ))
    }

    /// Returns whether the imported session was activated.
    ///
    /// # Errors
    /// Returns an error when the session is unknown.
    pub async fn activate_imported_session(&self, key: &SessionKey) -> Result<bool, ServiceError> {
        let was_suspended = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            let was_suspended = record.imported_suspended;
            record.imported_suspended = false;
            was_suspended
        };
        if was_suspended {
            self.persist(key).await?;
        }
        Ok(was_suspended)
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn create_session_with_settings(
        &self,
        title: Option<String>,
        system_prompt: &str,
        stored_spec: Option<ProviderModelSpec>,
        disabled_tools: Vec<String>,
        agent_profile: Option<String>,
    ) -> Result<SessionKey, ServiceError> {
        // A run with no configured model still needs an engine to create the
        // session, so the first catalogue model is borrowed for that alone. The
        // session itself carries no spec (`None`): it is not a model the person
        // chose, and a message sent on it is refused until one is.
        let engine_spec = stored_spec.clone().unwrap_or_else(|| {
            // Fallback for a session created without a configured model: borrow
            // the first catalogue entry to build the engine, never to serve a
            // request.
            let catalog = self.catalog();
            let provider = catalog
                .providers()
                .keys()
                .next()
                .cloned()
                .unwrap_or_default();
            let model = catalog
                .models_of(&provider)
                .next()
                .map(|m| m.id.clone())
                .unwrap_or_default();
            // Best effort: resolve through the catalogue; if it fails the engine
            // build below will surface the error.
            self.default_spec_for(&self.lock_config(), &catalog, &provider, &model)
                .unwrap_or_else(|_| ProviderModelSpec {
                    provider: provider.clone(),
                    model: model.clone(),
                    endpoint: String::new(),
                    api_key: norte_code_protocol::config::ApiKey::new(String::new()),
                    schema: norte_code_wire::catalog::WireSchema::OpenAI,
                    auth: None,
                    capabilities: norte_code_protocol::catalog::capabilities(
                        &norte_code_wire::catalog::Model::default(),
                        norte_code_wire::catalog::WireSchema::OpenAI,
                    ),
                    headers: std::collections::BTreeMap::new(),
                })
        });
        // A session can outlive the rest of the engine: it is nothing but a
        // row and a system message. When no model is configured the borrowed
        // spec cannot build an engine, so fall back to a direct store commit
        // and there is no history appended.
        let engine = self.engine(&engine_spec, None).ok();
        let session = if let Some(engine) = &engine {
            let session = engine.create().await?;
            let _ = engine
                .append_history(
                    session.id,
                    vec![norte::Message::system(system_prompt.to_owned())],
                )
                .await;
            session
        } else {
            let session = crate::runtime::Session::new();
            let _ = self
                .store
                .commit(crate::runtime::Commit {
                    session_id: session.id,
                    expected_version: None,
                    nodes: Vec::new(),
                    head: None,
                    phase: crate::runtime::Phase::Idle,
                    turn: 0,
                    metadata: None,
                })
                .await;
            session
        };
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let record = SessionRecord {
            id: session.id,
            title,
            spec: stored_spec,
            pending_provider: None,
            pending_model: None,
            created_at: Utc::now(),
            phase: ProtocolPhase::Idle,
            billed: 0,
            requests: 0,
            cost: 0.0,
            unpriced: 0,
            input_tokens: 0,
            context_used: 0,
            last_turn: 0,
            turn_boundaries: Vec::new(),
            rewound_history: Vec::new(),
            imported_from: None,
            imported_suspended: false,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            budget: self.configured_session_budget(),
            // Frozen here, at the one moment the prefix is not yet cached: the
            // tool declarations are part of it, so the set a session starts
            // with is the set it keeps.
            disabled_tools,
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            compacted_from: None,
            compacting: false,
            compaction_baseline: 0,
            reasoning: None,
            pending_reasoning: None,
            pending_budget: None,
            pinned_turn_ids: std::collections::HashSet::new(),
            pending_steers: Vec::new(),
            parent_client: None,
            parent_tool_call_id: None,
            child_terminal: None,
            agent_profile,
            depth: 0,
            delegator_suspended: false,
            delegator_queue: Vec::new(),
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
        Ok(key)
    }

    /// Copies a session into a new one, under a new key.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn fork_session(
        &self,
        from: &SessionKey,
        title: Option<String>,
        from_turn_id: Option<&str>,
        inclusive: bool,
        carry_usage: bool,
    ) -> Result<(SessionKey, Option<String>), ServiceError> {
        let (
            source,
            spec,
            agent_profile,
            disabled_tools,
            last_turn,
            boundaries,
            source_phase,
            usage,
            budget,
        ) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(from)
                .ok_or_else(|| ServiceError::UnknownSession(from.to_string()))?;
            (
                record.id,
                record.spec.clone(),
                record.agent_profile.clone(),
                record.disabled_tools.clone(),
                record.last_turn,
                record.turn_boundaries.clone(),
                record.phase,
                (
                    record.billed,
                    record.requests,
                    record.cost,
                    record.unpriced,
                    record.input_tokens,
                    record.context_used,
                    record.output_tokens,
                    record.cache_read_tokens,
                    record.cache_write_tokens,
                ),
                record.budget.clone(),
            )
        };
        let spec = spec.ok_or_else(|| ServiceError::UnknownSession(from.to_string()))?;
        let requested_turn = match from_turn_id {
            Some(id) => Some(
                id.strip_prefix("turn_")
                    .unwrap_or(id)
                    .parse::<u64>()
                    .map_err(|_| ServiceError::History("TURN_NOT_FOUND".to_owned()))?,
            ),
            None => boundaries.iter().map(|boundary| boundary.turn).max(),
        };
        let requested_turn = requested_turn.ok_or_else(|| {
            ServiceError::History("TURN_NOT_FOUND: no terminal turn is available".to_owned())
        })?;
        let target = boundaries
            .iter()
            .find(|boundary| boundary.turn == requested_turn)
            .ok_or_else(|| {
                if requested_turn > last_turn && !matches!(source_phase, ProtocolPhase::Idle) {
                    ServiceError::History("TURN_NOT_ACTIVE".to_owned())
                } else {
                    ServiceError::History("TURN_NOT_FOUND".to_owned())
                }
            })?;
        let included_boundary = if inclusive {
            Some(target)
        } else {
            boundaries
                .iter()
                .filter(|boundary| boundary.turn < requested_turn)
                .max_by_key(|boundary| boundary.turn)
        };
        let effective_turn = included_boundary.map_or(0, |boundary| boundary.turn);
        let first_boundary = boundaries.iter().min_by_key(|boundary| boundary.turn);
        let source_session = included_boundary
            .or(first_boundary)
            .map_or(source, |boundary| boundary.session);
        let source_state = self.store.load(source_session).await?;
        let mut source_nodes = if let Some(head) = included_boundary.and_then(|b| b.head) {
            self.store.ancestors(head).await?
        } else if let Some(head) = first_boundary.and_then(|boundary| boundary.head) {
            self.store
                .ancestors(head)
                .await?
                .into_iter()
                .take(1)
                .collect()
        } else {
            Vec::new()
        };
        let new_id = norte::SessionId::new();
        let mut parent = None;
        let mut nodes = Vec::with_capacity(source_nodes.len());
        let mut remapped = Vec::with_capacity(source_nodes.len());
        for old_id in source_nodes.drain(..) {
            let old_node = self.store.node(old_id).await?;
            let node = crate::runtime::Node::message(old_node.message, parent);
            parent = Some(node.id);
            remapped.push((old_id, node.id));
            nodes.push(node);
        }
        let turn_boundaries = boundaries
            .iter()
            .filter(|boundary| boundary.turn <= effective_turn)
            .filter_map(|boundary| {
                let head = boundary.head?;
                let new_head = remapped
                    .iter()
                    .find_map(|(old, new)| (*old == head).then_some(*new))?;
                Some(TurnBoundary {
                    turn: boundary.turn,
                    session: new_id,
                    head: Some(new_head),
                })
            })
            .collect();
        let imported = self
            .store
            .commit(crate::runtime::Commit {
                session_id: new_id,
                expected_version: None,
                nodes,
                head: parent,
                phase: crate::runtime::Phase::Idle,
                turn: effective_turn,
                metadata: Some(source_state.metadata),
            })
            .await
            .map_err(ServiceError::from)?;
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let (
            billed,
            requests,
            cost,
            unpriced,
            input_tokens,
            context_used,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        ) = if carry_usage {
            usage
        } else {
            (0, 0, 0.0, 0, 0, 0, 0, 0, 0)
        };
        let record = SessionRecord {
            id: imported.id,
            title,
            spec: Some(spec),
            agent_profile,
            pending_provider: None,
            pending_model: None,
            created_at: Utc::now(),
            phase: ProtocolPhase::Idle,
            billed,
            requests,
            cost,
            unpriced,
            input_tokens,
            context_used,
            last_turn: effective_turn,
            turn_boundaries,
            rewound_history: Vec::new(),
            imported_from: None,
            imported_suspended: false,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            budget: if carry_usage {
                budget
            } else {
                self.configured_session_budget()
            },
            // A fork copies the conversation, so it copies the tool set the
            // conversation was held in: a branch that quietly gained a tool the
            // source did not have would answer the same history differently.
            disabled_tools,
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            // A fork is a new branch, not a compaction of the source.
            compacted_from: None,
            compacting: false,
            compaction_baseline: 0,
            reasoning: None,
            pending_reasoning: None,
            pending_budget: None,
            pinned_turn_ids: std::collections::HashSet::new(),
            pending_steers: Vec::new(),
            parent_client: None,
            parent_tool_call_id: None,
            child_terminal: None,
            depth: 0,
            delegator_suspended: false,
            delegator_queue: Vec::new(),
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
        self.plans
            .copy_session(source, imported.id)
            .await
            .map_err(ServiceError::Plan)?;
        Ok((key, Some(format!("turn_{requested_turn}"))))
    }
}
