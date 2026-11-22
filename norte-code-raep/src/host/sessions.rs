//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use crate::payloads::payload;
use norte_code_agent::Service;
use norte_code_wire::SessionTitleSource;
use norte_code_wire::{ClientRequestKind, SessionKey};
use raep_types::{AttachMode, Envelope, SessionAttach, StandardFrameType};
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
    pub(crate) fn service_for_session(&self, session_id: &str) -> Result<Arc<Service>, HostError> {
        let agent_id = self.agent_for_session(session_id);
        self.service_for_agent(&agent_id)
    }
    pub(crate) fn agent_for_session(&self, session_id: &str) -> String {
        self.session_owner(session_id)
    }
    /// The agent that created the session, when the services know.
    ///
    /// A service whose own record says it made the session wins. Otherwise the
    /// store is the tie-breaker: in a multi-agent host every service sees the
    /// shared records, so a stored profile is only trusted when it names the
    /// service's key; an in-memory service with its own store remaining unique
    /// is also honoured.
    pub(crate) fn session_owner(&self, session_id: &str) -> String {
        let key = norte_code_wire::SessionKey::new(session_id);
        let stored = |service: &Arc<Service>| service.stored_agent_profile(&key);
        if let Some(agent_id) = self.services.iter().find_map(|(agent_id, service)| {
            stored(service)
                .flatten()
                .filter(|profile| profile == agent_id)
                .map(|_| agent_id.clone())
        }) {
            return agent_id;
        }
        let holders = self
            .services
            .iter()
            .filter(|(_, service)| stored(service).is_some())
            .map(|(agent_id, _)| agent_id.to_owned())
            .collect::<Vec<_>>();
        if holders.len() == 1 {
            let [agent_id] = <[String; 1]>::try_from(holders).expect("one holder");
            return agent_id;
        }
        self.services
            .iter()
            .find_map(|(_, service)| stored(service))
            .flatten()
            .unwrap_or_else(|| self.config.agent_id.clone())
    }
    /// The RAEP `session_summary` for every session `principal` may see,
    /// newest first: what was last worked on is what was last updated, so
    /// the list reads in the order you worked.
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
        by_recent_first(&mut live);
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
        let service = self.service_for_session(session_id).ok()?;
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
        let service = self.service_for_session(parent_session_id).ok()?;
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
        // The update time is the last turn the session finished; a session
        // that has never run one honestly shows its creation moment instead
        // of pretending "now".
        let updated_at = info
            .updated_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map_or(created_at, |value| value.timestamp_millis());
        let turn_count = info.turn_count.unwrap_or(0);
        let agent_id = self.session_owner(&info.session.to_string());
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
                id: agent_id.clone(),
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
            updated_at: Some(updated_at),
            last_activity_at: Some(updated_at),
            turn_count: Some(turn_count),
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
        agent_override: Option<&Value>,
        principal: &str,
    ) -> Result<String, HostError> {
        let key = self
            .service_for_agent(agent_id)?
            .start_session(title)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.claim_session(key.as_str(), principal).await?;
        if let Some(value) = agent_override {
            self.apply_agent_override(key.as_str(), value).await?;
        }
        Ok(key.to_string())
    }
    /// Applies a `SESSION_CONFIGURE`-shaped override to a session.
    ///
    /// # Errors
    ///
    /// Returns an error when the override is malformed or the service refuses
    /// one of the settings it names.
    pub(crate) async fn apply_agent_override(
        &self,
        session_id: &str,
        value: &Value,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let decoded = decode_override(value)?;
        validate_override(decoded.0.as_ref(), decoded.1.as_ref(), decoded.2.as_ref())?;
        let session = SessionKey::new(session_id);
        let mut frames = Vec::new();
        if let Some((provider, model_id)) = decoded.1 {
            frames.extend(
                self.submit_service(
                    session_id,
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
            frames.extend(
                self.submit_service(
                    session_id,
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
            frames.extend(
                self.submit_service(session_id, ClientRequestKind::SetBudget { session, budget })
                    .await?,
            );
        }
        Ok(frames)
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
        self.service_for_session(session_id)?
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
        self.replay.remove(session_id);
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
    /// A request older than what the session's file still holds comes back
    /// empty and truncated, so the client reloads history instead of
    /// pretending the stream is whole.
    ///
    /// # Errors
    ///
    /// Returns an error when the replay file cannot be read.
    pub(crate) fn replay_after(&self, session_id: &str, last_seq: u64) -> ReplayWindow {
        self.replay.after(session_id, last_seq)
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
            let current_agent = self.agent_for_session(&session_id);
            if current_agent != agent_id {
                return Err(HostError::InvalidFrame(format!(
                    "agent_mismatch: session runs agent `{current_agent}`"
                )));
            }
        }
        // `agent_override` on attach (RFC 6/7): same fragment as in
        // `SESSION_CONFIGURE`. Applied through the same translation, after
        // the session is bound but before `SESSION_ATTACHED` is emitted.
        let override_frames = match attach.agent_override.as_ref() {
            Some(value) => self.apply_agent_override(&session_id, value).await?,
            None => Vec::new(),
        };
        if let Some(plugin_host) = &self.plugin_host {
            plugin_host.set_session(&session_id, &agent_id);
        }
        self.state
            .lock()
            .await
            .session_agents
            .insert(session_id.clone(), agent_id);
        let resumed = self.replay.last_seq(&session_id).unwrap_or(0);
        {
            let mut state = self.state.lock().await;
            state.attached.insert(session_id.clone());
            state
                .attachment_modes
                .insert(session_id.clone(), mode.clone());
            state
                .client_capabilities
                .insert(session_id.clone(), client_capabilities.clone());
            if !state.sequences.contains_key(&session_id) {
                state.sequences.insert(session_id.clone(), resumed);
            }
        }
        let (replayed, replay_meta) = match attach.resume_from_seq {
            Some(after) => {
                let window = self.replay_after(&session_id, after);
                let to_seq = window
                    .frames
                    .iter()
                    .filter_map(|frame| frame.seq)
                    .max()
                    .unwrap_or(after);
                (
                    window.frames,
                    ReplayMeta {
                        from_seq: Some(after.saturating_add(1)),
                        to_seq: Some(to_seq),
                        truncated: window.truncated,
                    },
                )
            }
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
        // The frames the client missed come first: the attach frame that
        // follows names the window it just replayed.
        let mut output = replayed;
        output.push(attached);
        output.push(state);
        output.extend(override_frames);
        Ok(output)
    }
}

/// The moment the list should sort by: the last time anything touched the
/// session's head, or its creation when it was never run.
fn recency(info: &norte_code_wire::SessionInfo) -> i64 {
    chrono::DateTime::parse_from_rfc3339(info.updated_at.as_deref().unwrap_or_default())
        .map_or_else(
            |_| {
                info.created_at
                    .as_deref()
                    .and_then(|created| chrono::DateTime::parse_from_rfc3339(created).ok())
                    .map_or(i64::MIN, |value| value.timestamp_millis())
            },
            |value| value.timestamp_millis(),
        )
}

/// Newest first; two sessions never touched keep creation order.
fn by_recent_first(sessions: &mut [norte_code_wire::SessionInfo]) {
    sessions.sort_by_key(|info| std::cmp::Reverse(recency(info)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(updated_at: Option<&str>, created_at: &str) -> norte_code_wire::SessionInfo {
        norte_code_wire::SessionInfo {
            session: norte_code_wire::SessionKey::new("session-1"),
            title: None,
            reasoning: None,
            provider: None,
            model: None,
            created_at: Some(created_at.to_owned()),
            updated_at: updated_at.map(str::to_owned),
            turn_count: None,
            phase: norte_code_wire::Phase::Idle,
            parent: None,
        }
    }

    #[test]
    fn sessions_list_by_most_recently_updated() {
        let mut sessions = vec![
            info(Some("2026-11-01T00:00:00Z"), "2026-11-05T00:00:00Z"),
            info(Some("2026-11-14T00:00:00Z"), "2026-11-05T00:00:00Z"),
            info(None, "2026-11-03T00:00:00Z"),
        ];
        by_recent_first(&mut sessions);
        let order: Vec<_> = sessions
            .iter()
            .map(|session| {
                session
                    .updated_at
                    .clone()
                    .unwrap_or_else(|| session.created_at.clone().expect("created"))
            })
            .collect();

        assert_eq!(
            order,
            vec![
                "2026-11-14T00:00:00Z".to_owned(),
                "2026-11-03T00:00:00Z".to_owned(),
                "2026-11-01T00:00:00Z".to_owned(),
            ]
        );
    }
}
