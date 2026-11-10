//! Reading a session's history, including the part a compaction replaced.

use norte::domain::message::Message;
use norte::domain::part::Part;
use norte::id::SessionId;

use norte_code_protocol::sqlite::SessionDirectory;
use norte_code_wire::{MessageRole, ReplayedKind, ServerEvent, ServerEventKind, SessionKey};

use crate::provider::ModelSpec;
use crate::runtime::Harness;

use super::compaction::used_context;
use super::turn_helpers::{replay_text, role_name};
use super::{Service, ServiceError};
/// The session's accumulated usage, as the event a resuming client reads.
///
/// The counters live in the session record and survive a compaction; the context
/// figure is recomputed from the history the model will actually see, so a
/// compaction lowers it while the sums keep growing.
impl Service {
    pub(super) async fn resume_stats(
        &self,
        request_id: u64,
        key: &SessionKey,
        engine: &Harness,
        nid: SessionId,
        spec: &ModelSpec,
    ) -> Option<ServerEvent> {
        let (input_tokens, output_tokens, cost_usd, requests, cache_read, cache_write) = {
            let sessions = self.lock_sessions();
            let record = sessions.get(key)?;
            (
                record.input_tokens,
                record.output_tokens,
                record.cost,
                record.requests,
                record.cache_read_tokens,
                record.cache_write_tokens,
            )
        };
        let context_used = engine
            .history(nid)
            .await
            .map_or(0, |history| used_context(&history));
        Some(ServerEvent::new(
            request_id,
            Some(key.clone()),
            ServerEventKind::SessionStats {
                input_tokens,
                output_tokens,
                cost_usd,
                requests,
                cache_read_tokens: cache_read,
                cache_write_tokens: cache_write,
                context_used,
                context_window: spec.capabilities.max_context,
            },
        ))
    }
}

/// The Norte sessions this one descends from through compaction, newest first.
///
/// A compaction replaces the live session and records only its immediate
/// parent, so the older links are read back from the stored sessions: each one
/// knows the parent it came from, and the directory keeps them all.
pub(super) async fn compaction_ancestors(
    directory: &dyn SessionDirectory,
    current: SessionId,
) -> Vec<SessionId> {
    let Ok(stored) = directory.sessions().await else {
        return Vec::new();
    };
    // Map each Norte session to the one it was compacted from.
    let mut parents: std::collections::HashMap<SessionId, SessionId> =
        std::collections::HashMap::new();
    for entry in stored {
        if let Some(parent) = entry.compacted_from {
            parents.insert(entry.north, parent);
        }
    }
    let mut chain = Vec::new();
    let mut cursor = current;
    // A guard against a corrupt store that points a session at itself.
    let mut seen = std::collections::HashSet::new();
    seen.insert(cursor);
    while let Some(parent) = parents.get(&cursor).copied() {
        if !seen.insert(parent) {
            break;
        }
        chain.push(parent);
        cursor = parent;
    }
    chain
}

impl Service {
    /// Reads the full replayable history of a session, including history
    /// retained across compaction boundaries.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is unknown or its history cannot be
    /// read from the conversation store.
    pub async fn session_history(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<(MessageRole, ReplayedKind, String)>, ServiceError> {
        Ok(self
            .session_history_with_turns(key)
            .await?
            .into_iter()
            .map(|(_, role, kind, text, _)| (role, kind, text))
            .collect())
    }

    /// Reads replayable history with the durable turn and timestamp metadata
    /// needed by RAEP history pages.
    ///
    /// # Errors
    ///
    /// Returns an error when the session or any persisted conversation node
    /// cannot be read.
    pub async fn session_history_with_turns(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<(Option<u64>, MessageRole, ReplayedKind, String, i64)>, ServiceError> {
        Ok(self
            .session_history_with_provenance(key)
            .await?
            .into_iter()
            .map(|(turn, role, kind, text, timestamp, _)| (turn, role, kind, text, timestamp))
            .collect())
    }

    /// Reads replayable history with imported provenance where it was stored.
    ///
    /// # Errors
    ///
    /// Returns an error when the session or any persisted conversation node
    /// cannot be read.
    #[allow(clippy::type_complexity)]
    pub async fn session_history_with_provenance(
        &self,
        key: &SessionKey,
    ) -> Result<
        Vec<(
            Option<u64>,
            MessageRole,
            ReplayedKind,
            String,
            i64,
            Option<serde_json::Value>,
        )>,
        ServiceError,
    > {
        let (nid, _, _) = self.require_record(key)?;
        let mut sessions = compaction_ancestors(self.directory.as_ref(), nid).await;
        sessions.reverse();
        sessions.push(nid);
        let boundaries = self
            .lock_sessions()
            .get(key)
            .map(|record| record.turn_boundaries.clone())
            .unwrap_or_default();
        let mut replayed = Vec::new();
        for session_id in sessions {
            let session = self.store.load(session_id).await?;
            let ids = if let Some(head) = session.head {
                self.store.ancestors(head).await?
            } else {
                Vec::new()
            };
            let turn_ends: Vec<(usize, u64)> = boundaries
                .iter()
                .filter(|boundary| boundary.session == session_id)
                .filter_map(|boundary| {
                    let head = boundary.head?;
                    ids.iter()
                        .position(|id| *id == head)
                        .map(|index| (index, boundary.turn))
                })
                .collect();
            for (index, id) in ids.into_iter().enumerate() {
                let node = self.store.node(id).await?;
                let turn = turn_ends
                    .iter()
                    .filter(|(end, _)| *end >= index)
                    .min_by_key(|(end, _)| *end)
                    .map(|(_, turn)| *turn);
                let provenance = node.metadata.get("provenance").cloned();
                for (kind, text) in replay_text(&node.message) {
                    replayed.push((
                        turn,
                        role_name(node.message.role),
                        kind,
                        text,
                        node.created_at.timestamp_millis(),
                        provenance.clone(),
                    ));
                }
            }
        }
        Ok(replayed)
    }

    /// Whether every persisted history part is plain text.
    ///
    /// # Errors
    ///
    /// Returns an error when the session graph cannot be read.
    pub async fn session_history_is_plain_text(
        &self,
        key: &SessionKey,
    ) -> Result<bool, ServiceError> {
        let (session_id, _, _) = self.require_record(key)?;
        let mut sessions = compaction_ancestors(self.directory.as_ref(), session_id).await;
        sessions.push(session_id);
        for session_id in sessions {
            let session = self.store.load(session_id).await?;
            let Some(head) = session.head else {
                continue;
            };
            for node_id in self.store.ancestors(head).await? {
                let node = self.store.node(node_id).await?;
                if node
                    .message
                    .parts
                    .iter()
                    .any(|part| !matches!(part, Part::Text(_)))
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Reads the text history removed from the active model context by rewind.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is unknown.
    #[allow(clippy::type_complexity)]
    pub fn session_rewound_history_with_turns(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<(Option<u64>, MessageRole, ReplayedKind, String, i64)>, ServiceError> {
        self.require_record(key)?;
        Ok(self
            .lock_sessions()
            .get(key)
            .map(|record| {
                record
                    .rewound_history
                    .iter()
                    .cloned()
                    .map(|(turn, role, kind, text, timestamp)| {
                        (Some(turn), role, kind, text, timestamp)
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// The most recent messages a resume replays, so a client that starts blank can
/// show where the conversation was.
const REPLAY_MESSAGES: usize = 100;

/// A task as the tool's own state holds it, from the wire's copy.
pub(super) fn to_todo_item(
    info: &norte_code_wire::TodoInfo,
) -> norte_code_protocol::tools::TodoItem {
    use norte_code_protocol::tools::TodoStatus as Stored;
    use norte_code_wire::TodoStatus as Wire;
    norte_code_protocol::tools::TodoItem {
        content: info.content.clone(),
        status: match info.status {
            Wire::Pending => Stored::Pending,
            Wire::InProgress => Stored::InProgress,
            Wire::Completed => Stored::Completed,
        },
    }
}

/// A task as the wire reports it, from the tool state's own copy.
pub(super) fn from_todo_item(
    item: &norte_code_protocol::tools::TodoItem,
) -> norte_code_wire::TodoInfo {
    use norte_code_protocol::tools::TodoStatus as Stored;
    use norte_code_wire::TodoStatus as Wire;
    norte_code_wire::TodoInfo {
        content: item.content.clone(),
        status: match item.status {
            Stored::Pending => Wire::Pending,
            Stored::InProgress => Wire::InProgress,
            Stored::Completed => Wire::Completed,
        },
    }
}

/// The task list the session's last `todo` call left, read back from its call.
///
/// The list lives in the model's own call, which is committed with the rest of
/// the conversation, so it survives a process: this is the read that puts it
/// back. The newest call wins: the model sends the complete list every time -
/// and a call whose arguments do not parse is skipped rather than guessed at,
/// which is what the tool would do.
pub(super) fn restored_todos(history: &[Message]) -> Vec<norte_code_wire::TodoInfo> {
    use norte_code_wire::{TodoInfo, TodoStatus};
    let mut latest = Vec::new();
    for message in history {
        for call in message.tool_calls() {
            if call.name != "todo" {
                continue;
            }
            let Some(todos) = call.arguments.get("todos").and_then(|it| it.as_array()) else {
                continue;
            };
            let mut items = Vec::with_capacity(todos.len());
            let mut ok = true;
            for todo in todos {
                let (Some(content), Some(status)) = (
                    todo.get("content").and_then(|it| it.as_str()),
                    todo.get("status").and_then(|it| it.as_str()),
                ) else {
                    ok = false;
                    break;
                };
                let status = match status {
                    "pending" => TodoStatus::Pending,
                    "in_progress" => TodoStatus::InProgress,
                    "completed" => TodoStatus::Completed,
                    _ => {
                        ok = false;
                        break;
                    }
                };
                items.push(TodoInfo {
                    content: content.to_owned(),
                    status,
                });
            }
            if ok {
                latest = items;
            }
        }
    }
    latest
}

/// The recent messages of a session, as replay events for a client that is
/// starting blank.
///
/// A compaction seeds a fresh Norte session with the summary and the last few
/// messages, so the live history alone is short: a session that has compacted
/// would replay a handful of messages where the user expects the transcript.
/// The history is therefore reassembled from the chain of sessions this one was
/// compacted from, newest first, until [`REPLAY_MESSAGES`] are collected.
///
/// A compaction boundary is never crossed, which is what the user asked for: a
/// session compacted at message 50 replays from there, not from before the
/// summary that replaced the earlier turns.
pub(super) async fn replay_events(
    request_id: u64,
    key: &SessionKey,
    engine: &Harness,
    nid: SessionId,
    ancestors: &[SessionId],
) -> Vec<ServerEvent> {
    let Ok(mut history) = engine.history(nid).await else {
        return Vec::new();
    };
    // Walk back through the compaction chain, prepending each earlier session's
    // tail until there is enough to fill the window.
    for ancestor in ancestors {
        if history.len() >= REPLAY_MESSAGES {
            break;
        }
        if let Ok(earlier) = engine.history(*ancestor).await {
            let mut combined = earlier;
            combined.extend(history);
            history = combined;
        }
    }
    let start = history.len().saturating_sub(REPLAY_MESSAGES);
    let mut out: Vec<ServerEvent> = Vec::new();
    let mut index = 0_u64;
    for message in &history[start..] {
        let parts = replay_text(message);
        for (kind, text) in parts {
            index = index.saturating_add(1);
            out.push(ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::MessageReplayed {
                    index,
                    role: role_name(message.role),
                    kind,
                    text,
                },
            ));
        }
    }
    out
}
