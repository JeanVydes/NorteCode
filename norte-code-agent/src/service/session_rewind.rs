//! Rewinding a session to an earlier turn.

use norte_code_wire::{Phase as ProtocolPhase, SessionKey};

use super::session_record::TurnBoundary;
use super::{Service, ServiceError};

impl Service {
    /// Moves a session's active head to a terminal turn while retaining the
    /// removed text history for explicit `include_rewound` reads.
    ///
    /// # Errors
    ///
    /// Returns an error when the turn is unknown, the session is blocked, the
    /// turn contains a tool call whose side effects cannot be reported, or
    /// durable storage fails.
    #[allow(clippy::too_many_lines)]
    pub async fn rewind_session(
        &self,
        key: &SessionKey,
        to_turn_id: &str,
        inclusive: bool,
        cancel_pending: bool,
        restore_workspace: bool,
    ) -> Result<Vec<String>, ServiceError> {
        if restore_workspace {
            return Err(ServiceError::History("SNAPSHOT_NOT_FOUND".to_owned()));
        }
        let to_turn = to_turn_id
            .strip_prefix("turn_")
            .unwrap_or(to_turn_id)
            .parse::<u64>()
            .map_err(|_| ServiceError::History("TURN_NOT_FOUND".to_owned()))?;
        let (session_id, phase, boundaries) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (record.id, record.phase, record.turn_boundaries.clone())
        };
        if phase != ProtocolPhase::Idle {
            if !cancel_pending || !self.cancel_turn(key) {
                return Err(ServiceError::History(
                    "REWIND_BLOCKED: active turn".to_owned(),
                ));
            }
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if self
                        .require_record(key)
                        .is_ok_and(|(_, _, phase)| phase == ProtocolPhase::Idle)
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| {
                ServiceError::History("REWIND_BLOCKED: turn cancellation timed out".to_owned())
            })?;
        }
        if self.jobs.any_running(session_id) {
            if !cancel_pending {
                return Err(ServiceError::History(
                    "REWIND_BLOCKED: running jobs".to_owned(),
                ));
            }
            self.jobs.cancel_session(session_id);
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while self.jobs.any_running(session_id) {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| {
                ServiceError::History("REWIND_BLOCKED: job cancellation timed out".to_owned())
            })?;
        }
        if self
            .children
            .of(key)
            .iter()
            .any(|child| !child.is_terminal())
        {
            return Err(ServiceError::History(
                "REWIND_BLOCKED: non-terminal child session".to_owned(),
            ));
        }
        let target = boundaries
            .iter()
            .find(|boundary| boundary.turn == to_turn)
            .ok_or_else(|| ServiceError::History("TURN_NOT_FOUND".to_owned()))?;
        let rewound: Vec<_> = boundaries
            .iter()
            .filter(|boundary| boundary.turn > to_turn || (inclusive && boundary.turn == to_turn))
            .cloned()
            .collect();
        if rewound.is_empty() {
            return Ok(Vec::new());
        }
        let rewound_ids: Vec<String> = rewound
            .iter()
            .map(|boundary| format!("turn_{}", boundary.turn))
            .collect();
        let history = self.session_history_with_turns(key).await?;
        let archived: Vec<_> = history
            .into_iter()
            .filter_map(|(turn, role, kind, text, timestamp)| {
                turn.filter(|turn| rewound_ids.contains(&format!("turn_{turn}")))
                    .map(|turn| (turn, role, kind, text, timestamp))
            })
            .collect();
        // Tool calls in the removed turns are reported, not refused.
        //
        // Refusing forever was the old rule, on the theory that a tool's
        // effects cannot be undone and the person should not silently lose
        // them. But no tool declares its side effects yet, so the rule blocked
        // every rewind that crossed a tool call and nothing recorded what was
        // skipped. The rewind now proceeds and names the calls it removed; the
        // host passes them back in `HISTORY_REWOUND.side_effects` so the reader
        // sees what happened outside the conversation: a file written, a
        // command run: and can undo it themselves.
        let kept_turn = boundaries
            .iter()
            .filter(|boundary| boundary.turn < to_turn || (!inclusive && boundary.turn == to_turn))
            .map(|boundary| boundary.turn)
            .max()
            .unwrap_or(0);
        let kept = boundaries
            .iter()
            .find(|boundary| boundary.turn == kept_turn);
        let source_head = kept.and_then(|boundary| boundary.head).or(target.head);
        let mut old_ids = match source_head {
            Some(head) => self.store.ancestors(head).await?,
            None => Vec::new(),
        };
        if kept.is_none() {
            old_ids.truncate(1);
        }
        let mut parent = None;
        let mut nodes = Vec::with_capacity(old_ids.len());
        let mut remapped = Vec::with_capacity(old_ids.len());
        for old_id in old_ids {
            let old = self.store.node(old_id).await?;
            let node = crate::runtime::Node::message(old.message, parent);
            parent = Some(node.id);
            remapped.push((old_id, node.id));
            nodes.push(node);
        }
        let current = self.store.load(session_id).await?;
        self.store
            .commit(crate::runtime::Commit {
                session_id,
                expected_version: Some(current.version),
                nodes,
                head: parent,
                phase: crate::runtime::Phase::Idle,
                turn: kept_turn,
                metadata: Some(current.metadata),
            })
            .await?;
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.turn_boundaries = boundaries
                .iter()
                .filter(|boundary| boundary.turn <= kept_turn)
                .filter_map(|boundary| {
                    let old_head = boundary.head?;
                    let head = remapped
                        .iter()
                        .find_map(|(old, new)| (*old == old_head).then_some(*new))?;
                    Some(TurnBoundary {
                        turn: boundary.turn,
                        session: session_id,
                        head: Some(head),
                    })
                })
                .collect();
            record.rewound_history.extend(archived);
            record.compacted_from = None;
        }
        self.persist(key).await?;
        Ok(rewound_ids)
    }

    /// The tool calls that ran in the turns `turn_ids`, as `(tool_call_id,
    /// tool_name, arguments)`.
    ///
    /// A rewind archives the text of the turns it removes; this answers which
    /// tools ran there so the client can tell the person what happened outside
    /// the conversation. Nothing here claims to know the effects themselves -
    /// no tool declares them yet: only that the call happened.
    #[must_use]
    pub fn rewound_tool_calls(
        &self,
        key: &SessionKey,
        turn_ids: &[String],
    ) -> Vec<norte_code_wire::SideEffect> {
        let sessions = self.lock_sessions();
        let Some(record) = sessions.get(key) else {
            return Vec::new();
        };
        record
            .rewound_history
            .iter()
            .filter(|(turn, _, kind, _, _)| {
                *kind == norte_code_wire::ReplayedKind::ToolCall
                    && turn_ids.contains(&format!("turn_{turn}"))
            })
            .map(|(_, _, _, text, _)| {
                let (name, arguments) = text.split_once(' ').unwrap_or((text.as_str(), ""));
                norte_code_wire::SideEffect {
                    tool_call_id: String::new(),
                    tool_name: name.trim().to_owned(),
                    side_effects: arguments.trim().to_owned(),
                }
            })
            .collect()
    }
}
