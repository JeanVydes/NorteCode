//! Loading, persisting and creating sessions.

use norte_code_wire::{Phase as ProtocolPhase, SessionKey};

use super::mapping::to_phase;
use super::session_record::{SessionRecord, imported_history_nodes, record_from};
use super::session_stats::StoredStats;
use super::{Service, ServiceError, StoredSession};

impl Service {
    /// Reads the sessions of the directory and repairs the ones a hard kill
    /// left behind.
    pub(super) async fn load_sessions(&self) -> Result<(), ServiceError> {
        let stored = self.directory.sessions().await?;
        let catalog = self.catalog();
        let mut records = Vec::with_capacity(stored.len());
        let mut repaired = Vec::new();
        for entry in stored {
            let key = SessionKey::new(entry.key.clone());
            self.plans
                .ensure_session_folder(&key)
                .await
                .map_err(ServiceError::Plan)?;
            let spec =
                // A session saved on a provider the catalogue does not list has
                // to reopen on it: going straight to the catalogue dropped every
                // such session onto the default model, so a `exa` session came
                // back as whatever the config's default was.
                match self.default_spec_for(
                    &self.lock_config(),
                    &catalog,
                    &entry.provider,
                    &entry.model,
                ) {
                    Ok(spec) => spec,
                    Err(error) => {
                        tracing::warn!(
                            session = %entry.key,
                            provider = %entry.provider,
                            model = %entry.model,
                            %error,
                            "stored session model is unavailable; skipping"
                        );
                        continue;
                    }
                };
            let (phase, needs_repair) = self.read_phase(&entry).await;
            self.approval_rules.load_session(
                entry.north,
                &entry.denied_tools,
                &entry.allowed_tools,
            );
            let record = record_from(entry, Some(spec), phase);
            records.push((key.clone(), record));
            if needs_repair {
                repaired.push(key);
            }
        }
        {
            let mut sessions = self.lock_sessions();
            for (key, record) in records {
                sessions.insert(key, record);
            }
        }
        for key in &repaired {
            self.persist(key).await?;
        }
        self.restore_child_registry();
        Ok(())
    }

    /// Puts a session no turn is streaming any more back to `Idle`.
    ///
    /// `Phase::Streaming` means "a turn is in flight": the engine commits it
    /// before it asks the model, and only a committed answer lowers it. A turn
    /// that ends without one: the provider failed with the retries spent, or
    /// the process was killed: leaves the phase behind, and nothing visible
    /// breaks, which is what makes it hard to see: the next turn starts from
    /// `Idle | Streaming` either way. What does break is that `/model` and
    /// compaction apply only at an idle boundary, so every later switch queues
    /// behind a boundary the session can no longer reach.
    ///
    /// Best effort: a store that cannot be written is logged and not raised,
    /// because the caller is already holding the failure this is called after.
    /// Returns whether the session was actually settled.
    pub(super) async fn settle_streaming_to_idle(&self, nid: norte::SessionId) -> bool {
        let Ok(session) = self.store.load(nid).await else {
            return false;
        };
        if session.phase != crate::runtime::Phase::Streaming {
            return false;
        }
        let commit = crate::runtime::Commit {
            session_id: nid,
            expected_version: Some(session.version),
            nodes: Vec::new(),
            // No nodes, so no head: `Commit::validate` requires a head to be one
            // of the commit's own nodes, and `None` means "leave it where it
            // is", which is what this wants: settling a phase does not move
            // the conversation.
            head: None,
            phase: crate::runtime::Phase::Idle,
            turn: session.turn,
            metadata: None,
        };
        match self.store.commit(commit).await {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(
                    session = %nid,
                    %error,
                    "could not settle a session whose turn ended to idle"
                );
                false
            }
        }
    }

    /// The phase the session is in, and whether it needs a repair.
    ///
    /// Norte's store is the authority on phase: a parked tool or approval
    /// committed it, while the directory snapshot may predate the park.
    /// `streaming` is the exception: no process is streaming a session it just
    /// loaded, so a hard kill left it behind. Put the store back to idle and
    /// rewrite the directory row.
    async fn read_phase(&self, entry: &StoredSession) -> (ProtocolPhase, bool) {
        let stored_phase = entry.phase;
        let phase = match self.store.load(entry.north).await {
            Ok(session) if session.phase == crate::runtime::Phase::Streaming => {
                // The store outlived the streaming that wrote it, so the turn
                // is not coming back in this process: settle it and tell the
                // caller to rewrite the directory row from the same fact.
                let _ = self.settle_streaming_to_idle(entry.north).await;
                return (ProtocolPhase::Idle, true);
            }
            Ok(session) => to_phase(session.phase),
            Err(error) => {
                tracing::warn!(
                    session = %entry.key,
                    %error,
                    "could not read the session phase; using the directory copy"
                );
                match stored_phase {
                    ProtocolPhase::AwaitingTool => ProtocolPhase::AwaitingTool,
                    _ => ProtocolPhase::Idle,
                }
            }
        };
        // A directory row that says `streaming` is stale whether or not the
        // store agreed, so it is rewritten either way.
        (phase, stored_phase == ProtocolPhase::Streaming)
    }

    /// The row the store keeps for one session.
    fn stored_session(
        record: &SessionRecord,
        key: &SessionKey,
        workdir: String,
    ) -> Result<StoredSession, ServiceError> {
        Ok(StoredSession {
            key: key.to_string(),
            north: record.id,
            title: record.title.clone(),
            model: record
                .spec
                .as_ref()
                .map(|s| s.model.clone())
                .unwrap_or_default(),
            provider: record
                .spec
                .as_ref()
                .map(|s| s.provider.clone())
                .unwrap_or_default(),
            created_at: record.created_at,
            phase: record.phase,
            stats: StoredStats {
                billed: record.billed,
                requests: record.requests,
                cost: record.cost,
                unpriced: record.unpriced,
                input_tokens: record.input_tokens,
                context_used: record.context_used,
                last_turn: record.last_turn,
                turn_boundaries: record.turn_boundaries.clone(),
                rewound_history: record
                    .rewound_history
                    .iter()
                    .cloned()
                    .map(Into::into)
                    .collect(),
                imported_from: record.imported_from.clone(),
                imported_suspended: record.imported_suspended,
                output_tokens: record.output_tokens,
                cache_read_tokens: record.cache_read_tokens,
                cache_write_tokens: record.cache_write_tokens,
                budget: record.budget.clone(),
                agent_profile: record.agent_profile.clone(),
                compaction_baseline: record.compaction_baseline,
                reasoning: record.reasoning.clone(),
                pending_reasoning: record.pending_reasoning.clone(),
                pending_budget: record.pending_budget.clone(),
                pending_steers: record.pending_steers.clone(),
                pending_provider: record.pending_provider.clone(),
                pending_model: record.pending_model.clone(),
                disabled_tools: record.disabled_tools.clone(),
                parent_client: record.parent_client.clone(),
                parent_tool_call_id: record.parent_tool_call_id.clone(),
                child_state: record.child_terminal,
                depth: record.depth,
                delegator_suspended: record.delegator_suspended,
                delegator_queue: record.delegator_queue.clone(),
                pinned_turn_ids: record.pinned_turn_ids.clone(),
            }
            .value()
            .map_err(|error| ServiceError::History(error.to_string()))?,
            denied_tools: record.denied_tools.clone(),
            allowed_tools: record.allowed_tools.clone(),
            compacted_from: record.compacted_from,
            // Where the process is working, which is where the tools read and
            // write: a session is about a tree, and resuming it from another
            // directory would point every tool at the wrong one.
            workdir: Some(workdir),
        })
    }

    /// Writes one session back to the directory.
    pub(super) async fn persist(&self, key: &SessionKey) -> Result<(), ServiceError> {
        let workdir = self.workspace_root.display().to_string();
        let stored = {
            let sessions = self.lock_sessions();
            sessions
                .get(key)
                .map(|record| Self::stored_session(record, key, workdir.clone()))
                .transpose()?
        };
        if let Some(stored) = stored {
            self.plans
                .ensure_session_folder(&SessionKey::new(stored.key.clone()))
                .await
                .map_err(ServiceError::Plan)?;
            self.directory.save_session(&stored).await?;
        }
        Ok(())
    }

    /// Starts a new session with the default model and the system prompt.
    pub(super) async fn create_session(
        &self,
        title: Option<String>,
    ) -> Result<SessionKey, ServiceError> {
        let spec = self
            .default_spec
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        self.create_session_with_settings(
            title,
            &self.system_prompt,
            spec,
            self.configured_disabled_tools(),
            Some(self.config().name),
        )
        .await
    }

    /// Creates a new, suspended session containing imported plain-text
    /// user/assistant history.
    ///
    /// Imported items retain their roles and carry provenance in node
    /// metadata. System prompts, tool calls, and media are intentionally not
    /// accepted by this path.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported roles, oversized input, or failures
    /// creating/persisting the session history.
    pub async fn import_plain_history(
        &self,
        title: Option<String>,
        items: Vec<super::ImportedHistoryItem>,
        imported_from: serde_json::Value,
        usage_total: Option<serde_json::Value>,
    ) -> Result<SessionKey, ServiceError> {
        const MAX_ITEMS: usize = 100_000;
        const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;
        let text_bytes = items
            .iter()
            .try_fold(0_usize, |total, item| total.checked_add(item.content.len()));
        if items.len() > MAX_ITEMS || text_bytes.is_none_or(|size| size > MAX_TEXT_BYTES) {
            return Err(ServiceError::History(
                "import history exceeds the item or text limit".to_owned(),
            ));
        }
        if items
            .iter()
            .any(|item| item.role != "user" && item.role != "assistant")
        {
            return Err(ServiceError::History(
                "import history supports only user and assistant roles".to_owned(),
            ));
        }

        let key = self.create_session(title).await?;
        let session_id = self
            .lock_sessions()
            .get(&key)
            .map(|record| record.id)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        let current = self.store.load(session_id).await?;
        let (nodes, parent, boundaries, current_turn) =
            imported_history_nodes(session_id, current.head, items);
        // The commit's head must be one of its own nodes. A bundle that
        // carries no history has nothing to commit, and committing an empty
        // node list with the session's existing head is rejected by the store
        //: so an export without messages could never be imported back.
        let existing_messages = match current.head {
            Some(head) => self.store.ancestors(head).await?.len(),
            None => 0,
        };
        let billed = existing_messages.saturating_add(nodes.len());
        if !nodes.is_empty() {
            self.store
                .commit(crate::runtime::Commit {
                    session_id,
                    expected_version: Some(current.version),
                    nodes,
                    head: parent,
                    phase: crate::runtime::Phase::Idle,
                    turn: current.turn,
                    metadata: None,
                })
                .await?;
        }
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.turn_boundaries = boundaries;
            record.last_turn = current_turn;
            record.imported_from = Some(imported_from);
            record.imported_suspended = true;
            record.billed = billed;
            if let Some(usage) = usage_total {
                record.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
                record.output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
                record.cache_read_tokens = usage["cached_tokens"].as_u64().unwrap_or(0);
                record.cost = usage["cost"]["amount"].as_f64().unwrap_or(0.0);
                record.requests = usage["extra"]["requests"].as_u64().unwrap_or(0);
                record.unpriced = usage["extra"]["unpriced_requests"].as_u64().unwrap_or(0);
            }
        }
        self.persist(&key).await?;
        Ok(key)
    }
}
