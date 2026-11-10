//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl NorteCodeRaepHost {
    /// History page request (RFC 16): one page of the session's history.
    ///
    /// The service answers history reads as `MessageReplayed` events; the
    /// host gathers those into a stable, cursor-paginated history page.
    pub(crate) async fn history_get(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: HistoryGetRequest = serde_json::from_value(frame.payload.clone())?;
        let before_turn = request
            .before_turn_id
            .as_deref()
            .map(|id| id.strip_prefix("turn_").unwrap_or(id).parse::<u64>())
            .transpose()
            .map_err(|_| HostError::InvalidFrame("before_turn_id must name a turn".to_owned()))?;
        let limit = request.limit.unwrap_or(50);
        if !(1..=100).contains(&limit) {
            return Err(HostError::InvalidFrame(
                "HISTORY_GET limit must be between 1 and 100".to_owned(),
            ));
        }
        let active = self
            .service
            .session_history_with_provenance(&SessionKey::new(frame.session_id.clone()))
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        // The system prompt is the agent's own preamble, not something either
        // side said in the conversation: a client that draws it shows the
        // person a wall of instructions before their first message. It stays
        // in the engine's history; it just does not belong in a transcript.
        let mut history: Vec<_> = active
            .into_iter()
            .filter(|(_, role, _, _, _, _)| {
                !matches!(
                    role,
                    norte_code_wire::MessageRole::System | norte_code_wire::MessageRole::Developer
                )
            })
            .map(|(turn, role, kind, text, timestamp, origin)| {
                ((turn, role, kind, text, timestamp), false, origin)
            })
            .collect();
        if request.include_rewound {
            let rewound = self
                .service
                .session_rewound_history_with_turns(&SessionKey::new(frame.session_id.clone()))
                .map_err(|error| HostError::Service(error.to_string()))?;
            history.extend(
                rewound
                    .into_iter()
                    .map(|item| (item, true, Option::<Value>::None)),
            );
        }
        history.sort_by_key(|((turn, _, _, _, timestamp), rewound, _)| {
            (turn.unwrap_or_default(), *timestamp, *rewound)
        });
        if let Some(before) = before_turn
            && !history
                .iter()
                .any(|((turn, _, _, _, _), _, _)| *turn == Some(before))
        {
            return Err(HostError::InvalidFrame(format!(
                "TURN_NOT_FOUND: `{}` is not in this session's indexed history",
                request.before_turn_id.as_deref().unwrap_or_default()
            )));
        }
        let mut eligible: Vec<_> = history
            .into_iter()
            .filter(|((turn, _, _, _, _), _, _)| match (before_turn, turn) {
                (Some(before), Some(turn)) => *turn < before,
                (Some(_), None) => false,
                (None, _) => true,
            })
            .collect();
        let limit = usize::try_from(limit).unwrap_or(100);
        let has_more = eligible.len() > limit;
        if has_more {
            eligible.drain(..eligible.len() - limit);
        }
        let items: Vec<HistorySummaryItem> = eligible
            .into_iter()
            .map(|((turn, role, kind, text, timestamp), rewound, origin)| {
                history_item_value(HistoryItemView {
                    turn,
                    role,
                    kind,
                    text,
                    timestamp,
                    rewound,
                    origin,
                    max_content_bytes: None,
                })
            })
            .collect();
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::HistoryPage,
                payload(HistoryPagePayload { items, has_more }),
            )
            .await?,
        ])
    }
    pub(crate) async fn rewind(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: RewindRequest = serde_json::from_value(frame.payload.clone())?;
        let restore_workspace = request.workspace == Some(WorkspaceRewindPolicy::Restore);
        let rewound_turn_ids = self
            .service
            .rewind_session(
                &SessionKey::new(frame.session_id.clone()),
                &request.to_turn_id,
                request.inclusive,
                request.cancel_pending,
                restore_workspace,
            )
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        let side_effects = self
            .service
            .rewound_tool_calls(
                &SessionKey::new(frame.session_id.clone()),
                &rewound_turn_ids,
            )
            .into_iter()
            .map(|effect| HistorySideEffect {
                tool_call_id: effect.tool_call_id,
                tool_name: effect.tool_name,
                // No tool declares its effects yet, and the honest reading of
                // "we do not know" is the widest one: the call may have
                // touched the workspace. The name and arguments travel with
                // it so the reader can judge.
                side_effects: raep_types::ToolSideEffects::Local,
            })
            .collect();
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::HistoryRewound,
                payload(HistoryRewoundPayload {
                    to_turn_id: request.to_turn_id,
                    inclusive: request.inclusive,
                    rewound_turn_ids,
                    side_effects,
                    workspace_restored: false,
                }),
            )
            .await?,
        ])
    }
    /// Session fork (RFC 18.1): copy a session's history into a new root
    /// session. Maps onto `ForkSession`; the new session id comes back on
    /// the `SESSION_FORKED` event.
    pub(crate) async fn fork(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: ForkRequest = serde_json::from_value(frame.payload.clone())?;
        self.submit_service(
            &frame.session_id,
            ClientRequestKind::ForkSession {
                session: SessionKey::new(frame.session_id.clone()),
                title: request.title,
                from_turn_id: request.from_turn_id,
                inclusive: request.inclusive,
                carry_usage: request.carry_usage,
                agent_id: request.agent_id,
                include: request.include,
            },
        )
        .await
    }
    /// Session compaction (RFC 18.3): trigger context compaction now.
    /// Maps onto `Compact`; the `HISTORY_COMPACTED` notification follows.
    pub(crate) async fn compact(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        self.submit_service(
            &frame.session_id,
            ClientRequestKind::Compact {
                session: SessionKey::new(frame.session_id.clone()),
            },
        )
        .await
    }
    /// Context pins (RFC 18.5): pin or unpin turns so they stay verbatim
    /// through compaction. The pins are held in the host's in-memory
    /// state (up to `max_pinned_turns` = 20) and persisted through the
    /// [`PinStore`] when one is configured; without a store they do not
    /// survive a restart. The CLI does not pin; this is so another
    /// client (desktop) can.
    pub(crate) async fn context_pin(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let pinned = frame
            .payload
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let turn_ids: Vec<String> = frame
            .payload
            .get("turn_ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if turn_ids.is_empty() {
            return Err(HostError::InvalidFrame(
                "CONTEXT_PIN requires turn_ids".to_owned(),
            ));
        }
        let mut state = self.state.lock().await;
        let pins = state
            .pinned_turns
            .entry(frame.session_id.clone())
            .or_default();
        for id in &turn_ids {
            if pinned {
                if pins.contains(id) {
                    continue;
                }
                if pins.len() >= 20 {
                    return Err(HostError::InvalidFrame(
                        "PAYLOAD_TOO_LARGE: too many pinned turns".to_owned(),
                    ));
                }
                pins.push(id.clone());
            } else {
                pins.retain(|p| p != id);
            }
        }
        let pinned_turn_ids = pins.clone();
        drop(state);
        if let Some(store) = self.pin_store.clone() {
            store.save(&frame.session_id, &pinned_turn_ids).await?;
        }
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::ContextUpdated,
                payload(ContextUpdatedPayload { pinned_turn_ids }),
            )
            .await?,
        ])
    }
}
