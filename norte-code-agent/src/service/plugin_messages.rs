use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use norte::domain::part::Part;
use norte::id::{CallId, SessionId};
use norte_code_plugin_api::{HostEffects, UserMessageDraft};
use norte_code_protocol::config::{
    Config, PluginMessagesConfig, PluginPolicies, allows_user_messages,
};
use norte_code_wire::{PluginId, PluginMessageRejection};

/// One accepted draft waiting for the interceptor to commit it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PendingPluginMessage {
    pub(super) plugin_id: PluginId,
    pub(super) call_id: CallId,
    pub(super) parts: Vec<Part>,
    pub(super) len: usize,
}

/// One refusal the interceptor reports on the host transcript.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PendingPluginRejection {
    pub(super) plugin_id: PluginId,
    pub(super) call_id: CallId,
    pub(super) reason: PluginMessageRejection,
}

/// Everything one session has waiting from plugin tool results.
#[derive(Debug, Default)]
pub(super) struct SessionPluginMessages {
    pub(super) pending: VecDeque<PendingPluginMessage>,
    pub(super) rejected: VecDeque<PendingPluginRejection>,
}

/// The policy snapshot an engine uses for plugin messages.
#[derive(Clone, Debug)]
pub(super) struct PluginMessagePolicy {
    pub(super) config: PluginMessagesConfig,
    plugins: PluginPolicies,
}

impl PluginMessagePolicy {
    pub(super) fn new(config: &Config) -> Self {
        Self {
            config: config.tools.plugin_messages.clone(),
            plugins: config.plugins.clone(),
        }
    }

    fn rejection(&self, plugin_id: &PluginId) -> Option<PluginMessageRejection> {
        if !self.config.enabled {
            Some(PluginMessageRejection::Disabled)
        } else if !allows_user_messages(&self.plugins, plugin_id) {
            Some(PluginMessageRejection::PluginNotAllowed)
        } else {
            None
        }
    }
}

/// Drafts and refusals from plugin tool results, one queue per session.
#[derive(Debug, Default)]
pub(super) struct PluginMessages {
    sessions: Mutex<BTreeMap<SessionId, SessionPluginMessages>>,
    handled: Mutex<BTreeSet<(SessionId, CallId)>>,
}

impl PluginMessages {
    /// Accepts the effects of one plugin tool call.
    pub(super) fn enqueue(
        &self,
        session: SessionId,
        plugin_id: &PluginId,
        call_id: CallId,
        effects: Vec<HostEffects>,
        policy: &PluginMessagePolicy,
    ) {
        if effects.is_empty() {
            return;
        }
        if let Some(reason) = policy.rejection(plugin_id) {
            self.reject(session, plugin_id, call_id, reason);
            return;
        }
        if !self.mark_handled(session, call_id) {
            return;
        }
        let mut pending = VecDeque::new();
        let mut rejected = VecDeque::new();
        for effect in effects {
            for draft in effect.user_messages {
                match Self::accept(plugin_id, call_id, draft, &policy.config) {
                    Ok(message) => pending.push_back(message),
                    Err(reason) => rejected.push_back(PendingPluginRejection {
                        plugin_id: plugin_id.clone(),
                        call_id,
                        reason,
                    }),
                }
            }
        }
        let mut sessions = self.lock();
        let entry = sessions.entry(session).or_default();
        entry.pending.extend(pending);
        entry.rejected.extend(rejected);
    }

    /// Records one refusal for the host transcript.
    pub(super) fn reject(
        &self,
        session: SessionId,
        plugin_id: &PluginId,
        call_id: CallId,
        reason: PluginMessageRejection,
    ) {
        if !self.mark_handled(session, call_id) {
            return;
        }
        self.lock()
            .entry(session)
            .or_default()
            .rejected
            .push_back(PendingPluginRejection {
                plugin_id: plugin_id.clone(),
                call_id,
                reason,
            });
    }

    /// Takes everything waiting for a session.
    pub(super) fn take(&self, session: SessionId) -> SessionPluginMessages {
        self.lock().remove(&session).unwrap_or_default()
    }

    fn mark_handled(&self, session: SessionId, call_id: CallId) -> bool {
        self.handled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((session, call_id))
    }

    fn accept(
        plugin_id: &PluginId,
        call_id: CallId,
        draft: UserMessageDraft,
        config: &PluginMessagesConfig,
    ) -> Result<PendingPluginMessage, PluginMessageRejection> {
        let parts = draft.into_parts();
        if !config.allow_media && parts.iter().any(|part| part.as_media().is_some()) {
            return Err(PluginMessageRejection::MediaNotAllowed);
        }
        let len = parts
            .iter()
            .map(|part| match part {
                Part::Text(text) => text.text.len(),
                Part::Media(media) => media.inline_len(),
                _ => 0,
            })
            .sum();
        if len > config.max_bytes {
            return Err(PluginMessageRejection::TooLarge);
        }
        Ok(PendingPluginMessage {
            plugin_id: plugin_id.clone(),
            call_id,
            parts,
            len,
        })
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<SessionId, SessionPluginMessages>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
