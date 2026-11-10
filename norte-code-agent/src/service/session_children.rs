//! Child sessions: the delegation tree and what it remembers.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use norte_code_protocol::costs::BudgetBook;
use norte_code_wire::{
    Phase as ProtocolPhase, ServerEvent, ServerEventKind, SessionInfo, SessionKey,
};

use crate::provider::ModelSpec as ProviderModelSpec;

use super::session_record::{
    ChildSessionMetadata, ChildTerminalState, MAX_HELD_DELEGATOR_MESSAGES, SessionRecord,
};
use super::steering::Steered;
use super::{Service, ServiceError};
use crate::provider::ModelSpec;

impl Service {
    pub(super) async fn create_session_child(
        &self,
        metadata: ChildSessionMetadata,
        spec: ModelSpec,
        disabled_tools: Vec<String>,
        budget: Arc<Mutex<BudgetBook>>,
    ) -> Result<SessionKey, ServiceError> {
        let ChildSessionMetadata {
            parent,
            tool_call_id,
            task_summary,
            lifetime,
            agent_profile,
            system_prompt,
            cancel,
        } = metadata;
        let key = self
            .create_session_with_settings(
                None,
                &system_prompt,
                Some(spec.clone()),
                disabled_tools.clone(),
                Some(agent_profile.clone()),
            )
            .await?;
        {
            let mut sessions = self.lock_sessions();
            let depth = sessions.get(&parent).map_or(0, |record| record.depth) + 1;
            if let Some(record) = sessions.get_mut(&key) {
                record.parent_client = Some(parent.clone());
                record.parent_tool_call_id = Some(tool_call_id.clone());
                record.child_terminal = None;
                record.agent_profile = Some(agent_profile.clone());
                record.depth = depth;
            }
        }
        self.persist(&key).await?;
        self.lock_budget_states().insert(key.clone(), budget);
        self.children.register(
            parent.clone(),
            super::delegator::Child::new(key.clone(), tool_call_id.clone()),
        );
        self.children.set_cancel(&parent, &key, Arc::clone(&cancel));
        self.emit_delegation(ServerEvent::new(
            0,
            Some(parent),
            ServerEventKind::ChildSessionStarted {
                child_session_id: key.as_str().to_owned(),
                agent_id: agent_profile,
                tool_call_id,
                lifetime,
                task_summary: task_summary.chars().take(512).collect(),
            },
        ));
        Ok(key)
    }

    /// The delegation depth of `key`: 0 for a session the client made.
    pub(super) fn depth_of(&self, key: &SessionKey) -> u32 {
        self.lock_sessions()
            .get(key)
            .map_or(0, |record| record.depth)
    }

    /// Whether `key`'s delegator is suspended (RFC 8.2.11).
    pub fn delegator_suspended(&self, key: &SessionKey) -> bool {
        self.lock_sessions()
            .get(key)
            .is_some_and(|record| record.delegator_suspended)
    }

    /// Suspends or releases `key`'s delegator (RFC 8.2.11), returning the
    /// new state and how many messages were held. Releasing flushes the
    /// queue into the session's steering: a held message is a message that
    /// still has to reach the child. The updated state is persisted.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::UnknownSession`] when `key` is not known or a
    /// persistence error when the updated state cannot be saved.
    pub async fn set_delegator_suspended(
        &self,
        key: &SessionKey,
        suspended: bool,
    ) -> Result<(bool, usize), ServiceError> {
        let service = self;
        let (parent, nid, held) = {
            let mut sessions = service.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.delegator_suspended = suspended;
            let held = if suspended {
                Vec::new()
            } else {
                std::mem::take(&mut record.delegator_queue)
            };
            (record.parent_client.clone(), record.id, held)
        };
        let queued = held.len();
        if let Some(parent) = parent {
            let _ = service.children.set_suspended(&parent, key, suspended);
        }
        for text in held {
            service
                .steering
                .push(nid, Steered::new(uuid::Uuid::new_v4().to_string(), text));
        }
        service.persist(key).await?;
        Ok((suspended, queued))
    }

    /// Holds a message for `key` while its delegator is suspended (RFC
    /// 8.2.11). Returns how many messages are held now.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::UnknownSession`] when `key` is not known,
    /// [`ServiceError::Delegation`] when the held-message limit is reached, or
    /// a persistence error when the queue cannot be saved.
    pub async fn hold_delegator_message(
        &self,
        key: &SessionKey,
        text: String,
    ) -> Result<usize, ServiceError> {
        let (parent, held) = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            if record.delegator_queue.len() >= MAX_HELD_DELEGATOR_MESSAGES {
                return Err(ServiceError::Delegation(format!(
                    "DELEGATOR_SUSPENDED: at most {MAX_HELD_DELEGATOR_MESSAGES} messages may be queued"
                )));
            }
            record.delegator_queue.push(text.clone());
            (record.parent_client.clone(), record.delegator_queue.len())
        };
        if let Some(parent) = parent {
            let _ = self.children.hold(&parent, key, text);
        }
        self.persist(key).await?;
        Ok(held)
    }

    /// The sessions the client can see.
    /// The sessions the store can still open.
    ///
    /// A record can outlive its stored session: a delete that removed the
    /// row but not the in-memory record, or a database restored from an older
    /// copy. Such a record lists fine and then fails on the first attach with
    /// a store error naming an id nobody sent, which is a ghost the client
    /// cannot clear. They are pruned here, once, and never listed again.
    pub async fn list_live_sessions(&self) -> Vec<SessionInfo> {
        let records = {
            let sessions = self.lock_sessions();
            sessions
                .iter()
                .map(|(key, record)| (key.clone(), record.id))
                .collect::<Vec<_>>()
        };
        let mut dead = Vec::new();
        for (key, id) in &records {
            // Only a store that says the session does not exist proves the
            // record is an orphan. Any other failure: a busy database, a
            // decode problem: is a reason to keep the session, not to delete
            // it.
            if let Err(error) = self.store.load(*id).await
                && error.to_string().contains("not found")
            {
                dead.push(key.clone());
            }
        }
        if !dead.is_empty() {
            {
                let mut sessions = self.lock_sessions();
                for key in &dead {
                    sessions.remove(key);
                }
            }
            for key in &dead {
                tracing::warn!(session = %key, "dropped a session with no stored state");
                if let Err(error) = self.directory.delete_session(key.as_str()).await {
                    tracing::warn!(session = %key, %error, "could not drop the orphaned session row");
                }
            }
        }
        self.list_sessions()
    }

    pub(super) fn list_sessions(&self) -> Vec<SessionInfo> {
        self.lock_sessions()
            .iter()
            .map(|(key, record)| SessionInfo {
                session: key.clone(),
                title: record.title.clone(),
                provider: record.spec.as_ref().map(|s| s.provider.clone()),
                model: record.spec.as_ref().map(|s| s.model.clone()),
                created_at: Some(record.created_at.to_rfc3339()),
                phase: record.phase,
                reasoning: record.reasoning.clone(),
                parent: record.parent_client.clone(),
            })
            .collect()
    }

    /// Gives the session a new title.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::UnknownSession`] when the session key is not known.
    pub async fn rename(&self, key: &SessionKey, title: &str) -> Result<(), ServiceError> {
        {
            let mut lock = self.lock_sessions();
            let record = lock
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.title = Some(title.to_owned());
        }
        self.persist(key).await
    }

    /// Names the session after the first message it holds, truncated.
    ///
    /// The automatic title is the first user message cut to 32 characters, set
    /// once; this is the same rule, asked again for a session that has no title
    /// yet. Nothing is sent to a model.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Name`] when the session is unknown, its history
    /// cannot be read, or it holds no user message to name it after.
    pub async fn retitle_session(&self, key: &SessionKey) -> Result<String, ServiceError> {
        let (nid, spec, _) = self.require_record(key)?;
        let spec = spec.ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        let history = self.engine(&spec, Some(key))?.history(nid).await?;
        let first = history
            .iter()
            .find(|message| message.role == norte::Role::User)
            .map(norte::Message::text)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| {
                ServiceError::Name("the session has no message to be named after".to_owned())
            })?;
        let title: String = first.trim().chars().take(32).collect::<String>();
        let title = title.trim_end().to_owned();
        if title.is_empty() {
            return Err(ServiceError::Name(
                "the first message names nothing".to_owned(),
            ));
        }
        self.rename(key, &title).await?;
        Ok(title)
    }

    /// Forgets the session and its row.
    pub(super) async fn delete(&self, key: &SessionKey) -> Result<(), ServiceError> {
        let parent = self
            .lock_sessions()
            .get(key)
            .and_then(|record| record.parent_client.clone());
        if !self.lock_sessions().contains_key(key) {
            return Err(ServiceError::UnknownSession(key.to_string()));
        }
        self.plans
            .remove_session_folder(key)
            .await
            .map_err(ServiceError::Plan)?;
        self.lock_sessions()
            .remove(key)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        if let Some(parent) = parent {
            self.children.remove(&parent, key);
        }
        self.directory.delete_session(key.as_str()).await?;
        Ok(())
    }

    /// The record of a session, with the model and the phase it is in.
    pub(super) fn require_record(
        &self,
        key: &SessionKey,
    ) -> Result<(norte::SessionId, Option<ProviderModelSpec>, ProtocolPhase), ServiceError> {
        self.lock_sessions()
            .get(key)
            .map(|record| (record.id, record.spec.clone(), record.phase))
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))
    }

    /// The session a client named, which may be a prefix of its id.
    ///
    /// The footer names a session by the first eight characters of its id, and
    /// that is not enough to ask for it back: `resume 801605a1` answered
    /// "session `801605a1` is not known" while the footer on screen showed
    /// exactly those eight. A name that matches one session resolves to it; one
    /// that matches several is an error naming them, because silently taking
    /// the first would continue the wrong conversation; and one that matches
    /// none stays unknown, so the message people already know is unchanged.
    pub(super) fn resolve_key(&self, key: &SessionKey) -> Result<SessionKey, ServiceError> {
        let sessions = self.lock_sessions();
        if sessions.contains_key(key) {
            return Ok(key.clone());
        }
        let prefix = key.as_str();
        let matches: Vec<&SessionKey> = sessions
            .keys()
            .filter(|candidate| candidate.as_str().starts_with(prefix))
            .collect();
        match matches.as_slice() {
            [only] => Ok((*only).clone()),
            [] => Err(ServiceError::UnknownSession(prefix.to_owned())),
            many => Err(ServiceError::AmbiguousSession {
                prefix: prefix.to_owned(),
                matches: many.iter().map(ToString::to_string).collect(),
            }),
        }
    }

    /// The sessions the service holds.
    pub(super) fn lock_sessions(&self) -> MutexGuard<'_, BTreeMap<SessionKey, SessionRecord>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The children of `parent`: sessions whose `parent_client` is `parent`,
    /// oldest first. Used by `list_agents` and the delegator tools, which
    /// address a child by the id `spawn_agent` returned.
    pub fn children_of(&self, parent: &SessionKey) -> Vec<super::ChildSessionSummary> {
        let sessions = self.lock_sessions();
        let mut children: Vec<_> = sessions
            .iter()
            .filter(|(_, record)| record.parent_client.as_ref() == Some(parent))
            .map(|(key, record)| super::ChildSessionSummary {
                session: key.clone(),
                title: record.title.clone(),
                state: match record.child_terminal {
                    Some(ChildTerminalState::Failed) => super::ChildSessionState::Failed,
                    Some(ChildTerminalState::Stopped) => super::ChildSessionState::Stopped,
                    None => match record.phase {
                        ProtocolPhase::Streaming => super::ChildSessionState::Running,
                        ProtocolPhase::AwaitingTool => super::ChildSessionState::AwaitingApproval,
                        ProtocolPhase::Idle | _ => super::ChildSessionState::Idle,
                    },
                },
            })
            .collect();
        children.sort_by_key(|child| child.session.as_str().to_owned());
        children
    }

    /// Rebuilds the in-memory child registry from the persisted
    /// `parent_client` links (point 2 of the delegation cleanup).
    ///
    /// The registry is the tools' live view (`spawn_agent` registers there);
    /// the session records are the durable one. After a restart only the
    /// records remain, so the registry is rebuilt from them: a child that
    /// survived the restart is still inspectable, messageable and stoppable
    /// by its parent. The registry is rebuilt, not merged, so a child that
    /// was deleted while down does not reappear.
    pub fn restore_child_registry(&self) {
        let links = {
            let sessions = self.lock_sessions();
            sessions
                .iter()
                .filter_map(|(key, record)| {
                    record.parent_client.as_ref().map(|parent| {
                        (
                            parent.clone(),
                            key.clone(),
                            record.parent_tool_call_id.clone().unwrap_or_default(),
                            record.delegator_suspended,
                            record.delegator_queue.clone(),
                            matches!(record.phase, ProtocolPhase::Idle),
                            record.child_terminal,
                        )
                    })
                })
                .collect::<Vec<_>>()
        };
        self.children.clear();
        for (parent, child, tool_call_id, suspended, held, finished, terminal) in links {
            let mut entry = super::delegator::Child::new(child, tool_call_id);
            entry.suspended = suspended;
            entry.held = held;
            entry.state = match terminal {
                Some(ChildTerminalState::Failed) => super::delegator::ChildState::Failed,
                Some(ChildTerminalState::Stopped) => super::delegator::ChildState::Stopped,
                None if finished => super::delegator::ChildState::Idle,
                None => super::delegator::ChildState::Running,
            };
            self.children.register(parent, entry);
        }
    }
}
