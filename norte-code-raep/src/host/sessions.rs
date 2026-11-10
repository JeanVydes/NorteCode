//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use crate::payloads::payload;
use norte_code_agent::Service;
use norte_code_wire::SessionTitleSource;
use norte_code_wire::{ClientRequestKind, SessionKey};
use raep_types::{AttachMode, Envelope, FrameType, SessionAttach, StandardFrameType};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

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
    pub(crate) fn service_for_agent(&self, agent_id: &str) -> Result<Arc<Service>, HostError> {
        let requested = if agent_id.is_empty() {
            &self.config.agent_id
        } else {
            agent_id
        };
        self.services
            .get(requested)
            .cloned()
            .ok_or_else(|| HostError::InvalidFrame(format!("unknown agent `{requested}`")))
    }
    pub(crate) async fn service_for_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<Service>, HostError> {
        if let Some(agent_id) = self.agent_for_session(session_id).await {
            return self.service_for_agent(&agent_id);
        }
        Ok(Arc::clone(&self.service))
    }
    pub(crate) async fn agent_for_session(&self, session_id: &str) -> Option<String> {
        if let Some(agent_id) = self
            .state
            .lock()
            .await
            .session_agents
            .get(session_id)
            .cloned()
        {
            return Some(agent_id);
        }
        self.services
            .iter()
            .find(|(_, service)| {
                service
                    .sessions()
                    .iter()
                    .any(|session| session.session.as_str() == session_id)
            })
            .map(|(agent_id, _)| agent_id.clone())
    }
    /// Returns the RAEP `session_summary` for every session `principal` may see.
    ///
    /// A session with no recorded owner is visible to everyone; a session with
    /// an owner is visible only to that principal.
    pub async fn session_summaries(&self, principal: &str) -> Vec<Value> {
        let owners = {
            let state = self.state.lock().await;
            state.session_owners.clone()
        };
        let now = now_millis();
        // Every agent has its own service over the same directory, so each of
        // them lists every session: flat-mapping them showed one session twice
        // — once per agent — and the sidebar carried a phantom copy of each.
        // A session belongs to exactly one key, so it is listed once.
        let mut seen = std::collections::BTreeSet::new();
        let mut live = Vec::new();
        for service in self.services.values() {
            for info in service.list_live_sessions().await {
                if seen.insert(info.session.as_str().to_owned()) {
                    live.push(info);
                }
            }
        }
        live.into_iter()
            .filter(|info| {
                owners
                    .get(info.session.as_str())
                    .is_none_or(|owner| owner == principal)
            })
            .map(|info| self.session_summary_value(&info, now))
            .collect()
    }
    /// Returns one `session_summary`.
    ///
    /// `None` means the session does not exist or belongs to another principal,
    /// which the HTTP resource answers as `404` rather than leaking existence.
    pub async fn session_summary(&self, session_id: &str, principal: &str) -> Option<Value> {
        let owner = self
            .state
            .lock()
            .await
            .session_owners
            .get(session_id)
            .cloned();
        if owner.is_some_and(|owner| owner != principal) {
            return None;
        }
        let service = self.service_for_session(session_id).await.ok()?;
        let info = service
            .sessions()
            .into_iter()
            .find(|info| info.session.as_str() == session_id)?;
        Some(self.session_summary_value(&info, now_millis()))
    }
    /// Returns child sessions only when the principal owns the parent session.
    pub async fn child_sessions(
        &self,
        parent_session_id: &str,
        principal: &str,
    ) -> Option<Vec<norte_code_agent::service::ChildSessionSummary>> {
        self.session_summary(parent_session_id, principal).await?;
        let service = self.service_for_session(parent_session_id).await.ok()?;
        Some(service.children_of(&SessionKey::new(parent_session_id)))
    }
    pub(crate) fn session_summary_value(
        &self,
        info: &norte_code_wire::SessionInfo,
        now: i64,
    ) -> Value {
        let created_at = info
            .created_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map_or(now, |value| value.timestamp_millis());
        let agent_id = self
            .services
            .iter()
            .find(|(_, service)| {
                service
                    .sessions()
                    .iter()
                    .any(|session| session.session == info.session)
            })
            .map_or(self.config.agent_id.as_str(), |(agent_id, _)| {
                agent_id.as_str()
            });
        let spec = self
            .services
            .values()
            .find_map(|service| service.session_spec(&info.session));
        payload(norte_code_wire::SessionSummary {
            session_id: info.session.as_str().to_owned(),
            title: info.title.clone(),
            title_source: SessionTitleSource::User,
            labels: BTreeMap::new(),
            pinned: false,
            archived: false,
            status: if self
                .services
                .values()
                .any(|service| service.session_imported_suspended(&info.session))
            {
                norte_code_wire::SessionStatus::Suspended
            } else {
                norte_code_wire::SessionStatus::Active
            },
            agent: Some(norte_code_wire::AgentRef {
                id: agent_id.to_owned(),
                version: self.config.server_version.clone(),
            }),
            model: spec.as_ref().map(|spec| norte_code_wire::AgentRef {
                id: spec.model.clone(),
                version: String::new(),
            }),
            provider: spec.as_ref().map(|spec| spec.provider.clone()),
            reasoning: info.reasoning.clone(),
            parent_session_id: info.parent.as_ref().map(ToString::to_string),
            created_at: Some(created_at),
            updated_at: Some(now),
            last_activity_at: Some(now),
            turn_count: Some(0),
            has_pending: Some(false),
        })
    }
    /// Creates a session through the management API and records its owner.
    ///
    /// # Errors
    ///
    /// Returns a service error when the session cannot be created.
    pub async fn create_session(
        &self,
        agent_id: &str,
        title: Option<String>,
        principal: &str,
    ) -> Result<String, HostError> {
        let key = self
            .service_for_agent(agent_id)?
            .start_session(title)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.claim_session(key.as_str(), principal).await?;
        Ok(key.to_string())
    }
    /// Renames a session through the management API.
    ///
    /// Returns `false` when the session is unknown or belongs to another
    /// principal.
    ///
    /// # Errors
    ///
    /// Returns a service error when the rename cannot be persisted.
    pub async fn rename_session(
        &self,
        session_id: &str,
        title: &str,
        principal: &str,
    ) -> Result<bool, HostError> {
        if self.session_summary(session_id, principal).await.is_none() {
            return Ok(false);
        }
        self.service_for_session(session_id)
            .await?
            .rename(&SessionKey::new(session_id), title)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        Ok(true)
    }
    /// Deletes a session through the management API.
    ///
    /// Returns `false` when the session is unknown or belongs to another
    /// principal.
    ///
    /// # Errors
    ///
    /// Returns a service error when the delete is rejected.
    pub async fn delete_session(
        &self,
        session_id: &str,
        principal: &str,
    ) -> Result<bool, HostError> {
        if self.session_summary(session_id, principal).await.is_none() {
            return Ok(false);
        }
        self.submit_service(
            session_id,
            ClientRequestKind::DeleteSession {
                session: SessionKey::new(session_id),
            },
        )
        .await?;
        self.state.lock().await.attached.remove(session_id);
        Ok(true)
    }
    /// Returns the session a previous `Idempotency-Key` created.
    pub async fn session_for_create_key(&self, key: &str) -> Option<String> {
        self.state
            .lock()
            .await
            .session_create_keys
            .get(key)
            .cloned()
    }
    /// Remembers the session created for an `Idempotency-Key`.
    ///
    /// The map keeps the same bound as the command idempotency cache so a
    /// client that sends a fresh key on every create cannot grow it without
    /// limit.
    pub async fn remember_create_key(&self, key: &str, session_id: &str) {
        let mut state = self.state.lock().await;
        state
            .session_create_keys
            .insert(key.to_owned(), session_id.to_owned());
        state.session_create_order.push_back(key.to_owned());
        while state.session_create_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.session_create_order.pop_front() {
                state.session_create_keys.remove(&oldest);
            }
        }
    }
    pub(crate) async fn claim_session(
        &self,
        session_id: &str,
        principal: &str,
    ) -> Result<(), HostError> {
        let mut state = self.state.lock().await;
        match state.session_owners.get(session_id) {
            Some(owner) if owner != principal => Err(HostError::InvalidFrame(
                "session belongs to another principal".to_owned(),
            )),
            Some(_) => Ok(()),
            None => {
                state
                    .session_owners
                    .insert(session_id.to_owned(), principal.to_owned());
                Ok(())
            }
        }
    }
    /// Returns replay frames strictly after `last_seq`.
    ///
    /// # Errors
    ///
    /// Returns an error when the session has no retained replay window or the
    /// requested sequence predates the retained window.
    pub async fn replay_after(
        &self,
        session_id: &str,
        last_seq: u64,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let state = self.state.lock().await;
        let frames = state
            .replay
            .get(session_id)
            .ok_or_else(|| HostError::Replay("session replay window is missing".to_owned()))?;
        if let Some(first) = frames.front().and_then(|frame| frame.seq)
            && last_seq.saturating_add(1) < first
        {
            return Err(HostError::Replay(
                "requested sequence is older than the retained window".to_owned(),
            ));
        }
        Ok(frames
            .iter()
            .filter(|frame| frame.seq.is_some_and(|seq| seq > last_seq))
            .cloned()
            .collect())
    }
    pub(crate) async fn load_persisted_replay(&self, session_id: &str) -> Result<(), HostError> {
        let Some(store) = &self.replay_store else {
            return Ok(());
        };
        if self.state.lock().await.loaded_replay.contains(session_id) {
            return Ok(());
        }
        let persisted = store.load(session_id).await?;
        let mut state = self.state.lock().await;
        let mut max_seq = 0;
        let replayed = persisted
            .into_iter()
            .filter(|frame| {
                if let Some(seq) = frame.seq {
                    max_seq = max_seq.max(seq);
                    true
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        if max_seq > 0 {
            state
                .sequences
                .entry(session_id.to_owned())
                .and_modify(|current| *current = (*current).max(max_seq))
                .or_insert(max_seq);
        }
        let replay = state.replay.entry(session_id.to_owned()).or_default();
        replay.extend(replayed);
        while replay.len() > self.config.replay_capacity {
            replay.pop_front();
        }
        let turn_states = replay
            .iter()
            .filter(|frame| {
                matches!(
                    frame.kind,
                    FrameType::Standard(StandardFrameType::TurnStateChanged)
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        for frame in turn_states {
            if let Some(turn_id) = &frame.turn_id {
                let key = (session_id.to_owned(), turn_id.clone());
                if state.turn_request_fingerprints.contains_key(&key) {
                    state.turn_request_frames.insert(key, frame);
                }
            }
        }
        state.loaded_replay.insert(session_id.to_owned());
        Ok(())
    }
    pub(crate) async fn load_persisted_idempotency(
        &self,
        session_id: &str,
    ) -> Result<(), HostError> {
        let Some(store) = &self.replay_store else {
            return Ok(());
        };
        if self
            .state
            .lock()
            .await
            .loaded_idempotency
            .contains(session_id)
        {
            return Ok(());
        }
        let records = store.load_idempotency(session_id).await?;
        let mut state = self.state.lock().await;
        if !state.loaded_idempotency.insert(session_id.to_owned()) {
            return Ok(());
        }
        for record in records {
            let frame_key = (session_id.to_owned(), record.frame_id);
            state.idempotency.insert(
                frame_key.clone(),
                CachedRequest {
                    fingerprint: record.fingerprint,
                    response: record.response.clone(),
                },
            );
            state.idempotency_order.push_back(frame_key);
            if let (Some(turn_id), Some(turn_fingerprint)) =
                (record.turn_id, record.turn_fingerprint)
            {
                let turn_key = (session_id.to_owned(), turn_id.clone());
                state
                    .turn_request_fingerprints
                    .insert(turn_key.clone(), turn_fingerprint);
                if let Some(frame) = record
                    .response
                    .iter()
                    .rev()
                    .find(|frame| frame.turn_id.as_ref() == Some(&turn_id))
                {
                    state
                        .turn_request_frames
                        .insert(turn_key.clone(), frame.clone());
                }
                state.turn_request_order.push_back(turn_key);
                if let Some(service_turn_id) = record.service_turn_id {
                    state
                        .turn_aliases
                        .insert((session_id.to_owned(), service_turn_id), turn_id);
                    state
                        .turn_alias_order
                        .push_back((session_id.to_owned(), service_turn_id));
                }
            }
        }
        while state.idempotency_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.idempotency_order.pop_front() {
                state.idempotency.remove(&oldest);
            }
        }
        while state.turn_request_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.turn_request_order.pop_front() {
                state.turn_request_fingerprints.remove(&oldest);
                state.turn_request_frames.remove(&oldest);
            }
        }
        while state.turn_alias_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.turn_alias_order.pop_front() {
                state.turn_aliases.remove(&oldest);
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn attach(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let attach: SessionAttach = serde_json::from_value(frame.payload)?;
        let client_capabilities = attach.capabilities.clone().unwrap_or_default();
        let job_turn_policy = parse_job_turn_policy(attach.options.as_ref())?;
        let mode = attach.mode.unwrap_or(AttachMode::Controller);
        if mode == AttachMode::Observer && job_turn_policy.is_some() {
            return Err(HostError::InvalidFrame(
                "FORBIDDEN: observer attachments cannot configure job_turn_policy".to_owned(),
            ));
        }
        let agent_id = if attach.agent_id.is_empty() {
            self.config.agent_id.clone()
        } else {
            attach.agent_id.clone()
        };
        let service = self.service_for_agent(&agent_id)?;
        let (session_id, created) = if frame.session_id.is_empty() {
            let key = service
                .start_session(None)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?;
            (key.to_string(), true)
        } else {
            (frame.session_id.clone(), false)
        };
        let exists = created
            || self
                .service_for_session(&session_id)
                .await
                .ok()
                .and_then(|service| service.session_title(&SessionKey::new(session_id.clone())))
                .is_some()
            || service
                .session_title(&SessionKey::new(session_id.clone()))
                .is_some()
            || service
                .session_spec(&SessionKey::new(session_id.clone()))
                .is_some()
            || service
                .sessions()
                .iter()
                .any(|session| session.session.as_str() == session_id);
        // A session this host cannot find is not created here, whatever
        // `create_if_missing` says: the id was chosen by the client, and
        // binding a fresh engine session to it would resurrect a deleted
        // session — and fail later with a store error naming an internal id.
        // A new session is made by attaching with an empty id, or by POSTing
        // one; reusing an id that is gone is "not found".
        if !exists {
            return Err(HostError::InvalidFrame("session not found".to_owned()));
        }
        // One controller at a time. Attaching again on a session this host
        // already considers attached is the reload case — the browser
        // reopens the stream before the old connection's detach lands — so
        // the new attachment takes over instead of leaving the client with a
        // 409 and no way forward. A named `takeover: false` is still
        // honoured as "do not displace anyone".
        let already_attached = self.state.lock().await.attached.contains(&session_id);
        if already_attached && attach.takeover == Some(false) {
            return Err(HostError::InvalidFrame(
                "SESSION_BUSY: another client is already attached to this session".to_owned(),
            ));
        }
        if already_attached {
            tracing::info!(
                session = %session_id,
                "taking over an existing attachment on re-attach"
            );
        }
        // `agent_mismatch` (RFC 6): for an existing session, the requested
        // agent must equal the session's current agent.
        if exists && !created {
            let current_agent = self
                .agent_for_session(&session_id)
                .await
                .unwrap_or_else(|| self.config.agent_id.clone());
            if current_agent != agent_id {
                return Err(HostError::InvalidFrame(format!(
                    "agent_mismatch: session runs agent `{current_agent}`"
                )));
            }
        }
        // `agent_override` on attach (RFC 6/7): same fragment as in
        // `SESSION_CONFIGURE`. Applied through the same translation, after
        // the session is bound but before `SESSION_ATTACHED` is emitted.
        let mut override_frames = Vec::new();
        if let Some(ref override_value) = attach.agent_override {
            let decoded = decode_override(override_value)?;
            validate_override(decoded.0.as_ref(), decoded.1.as_ref(), decoded.2.as_ref())?;
            let session = SessionKey::new(session_id.clone());
            if let Some((provider, model_id)) = decoded.1 {
                override_frames.extend(
                    self.submit_service(
                        &session_id,
                        ClientRequestKind::SetModel {
                            session: session.clone(),
                            provider,
                            model: model_id,
                        },
                    )
                    .await?,
                );
            }
            if let Some(reasoning) = decoded.0 {
                override_frames.extend(
                    self.submit_service(
                        &session_id,
                        ClientRequestKind::SetReasoning {
                            session: session.clone(),
                            scope: norte_code_wire::ReasoningScope::Session,
                            reasoning,
                        },
                    )
                    .await?,
                );
            }
            if let Some(budget) = decoded.2 {
                override_frames.extend(
                    self.submit_service(
                        &session_id,
                        ClientRequestKind::SetBudget {
                            session: session.clone(),
                            budget,
                        },
                    )
                    .await?,
                );
            }
        }
        self.state
            .lock()
            .await
            .session_agents
            .insert(session_id.clone(), agent_id);
        self.load_persisted_replay(&session_id).await?;
        {
            let mut state = self.state.lock().await;
            state.attached.insert(session_id.clone());
            state
                .attachment_modes
                .insert(session_id.clone(), mode.clone());
            state
                .client_capabilities
                .insert(session_id.clone(), client_capabilities.clone());
            state.sequences.entry(session_id.clone()).or_insert(0);
            state.replay.entry(session_id.clone()).or_default();
        }
        let (replayed, replay_meta) = match attach.resume_from_seq {
            Some(after) => match self.replay_after(&session_id, after).await {
                Ok(frames) => {
                    let to_seq = frames
                        .iter()
                        .filter_map(|frame| frame.seq)
                        .max()
                        .unwrap_or(after);
                    (
                        frames,
                        ReplayMeta {
                            from_seq: Some(after.saturating_add(1)),
                            to_seq: Some(to_seq),
                            truncated: false,
                        },
                    )
                }
                Err(HostError::Replay(_)) => (
                    Vec::new(),
                    ReplayMeta {
                        from_seq: Some(after.saturating_add(1)),
                        to_seq: Some(after),
                        truncated: true,
                    },
                ),
                Err(error) => return Err(error),
            },
            None => (
                Vec::new(),
                ReplayMeta {
                    from_seq: None,
                    to_seq: None,
                    truncated: false,
                },
            ),
        };
        let last_seq = self.current_seq(&session_id).await;
        let attached = self
            .emit(
                &session_id,
                StandardFrameType::SessionAttached,
                payload(SessionAttachedPayload {
                    created,
                    mode,
                    last_seq,
                    replay: replay_meta,
                    negotiated_capabilities: negotiated_capabilities(
                        self.uploads_enabled(),
                        &client_capabilities,
                    ),
                    agent: session_agent_summary(
                        &service,
                        &self.config.agent_id,
                        &self.config.server_version,
                        &SessionKey::new(session_id.clone()),
                        self.uploads_enabled(),
                    ),
                }),
            )
            .await?;
        let session_key = SessionKey::new(session_id.clone());
        let imported_from = service.session_imported_from(&session_key);
        service
            .activate_imported_session(&session_key)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        if !service.set_job_turn_policy(
            &session_key,
            job_turn_policy.unwrap_or(norte_code_wire::JobTurnPolicy::Always),
        ) {
            return Err(HostError::InvalidFrame(
                "session not found while applying job_turn_policy".to_owned(),
            ));
        }
        let title = service.session_title(&SessionKey::new(session_id.clone()));
        let history = service
            .session_history_with_provenance(&SessionKey::new(session_id.clone()))
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        let history_start = history.len().saturating_sub(20);
        let history_summary = history
            .into_iter()
            .skip(history_start)
            .map(|(turn, role, kind, text, timestamp, origin)| {
                history_item_value(HistoryItemView {
                    turn,
                    role,
                    kind,
                    text,
                    timestamp,
                    rewound: false,
                    origin,
                    max_content_bytes: Some(2 * 1024),
                })
            })
            .collect();
        let state = self
            .emit(
                &session_id,
                StandardFrameType::SessionState,
                payload(SessionStatePayload {
                    status: SessionStateStatus::Active,
                    title: title.as_deref(),
                    history_summary,
                    imported_from: imported_from.as_ref(),
                }),
            )
            .await?;
        let mut output = vec![attached];
        output.extend(replayed);
        output.push(state);
        output.extend(override_frames);
        Ok(output)
    }
}
