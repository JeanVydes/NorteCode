//! Accepts, queues, and starts RAEP user input turns.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;

struct ResolvedContent {
    text: String,
    attachments: Vec<norte_code_wire::Attachment>,
    upload_ids: Vec<String>,
}

impl NorteCodeRaepHost {
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn user_input(
        &self,
        frame: Envelope<Value>,
        principal: Option<&str>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let turn_id = frame.turn_id.clone().ok_or_else(|| {
            HostError::InvalidFrame("USER_INPUT requires envelope.turn_id".to_owned())
        })?;
        let turn_fingerprint = crate::bundle::sha256(&serde_json::to_vec(&frame.payload)?);
        let turn_key = (frame.session_id.clone(), turn_id.clone());
        {
            let state = self.state.lock().await;
            if let Some(previous_fingerprint) = state.turn_request_fingerprints.get(&turn_key) {
                if previous_fingerprint != &turn_fingerprint {
                    return Err(HostError::IdempotencyConflict(
                        "turn_id was reused with different input".to_owned(),
                    ));
                }
                return Ok(state
                    .turn_request_frames
                    .get(&turn_key)
                    .cloned()
                    .into_iter()
                    .collect());
            }
        }
        let input: AcceptedUserInput = serde_json::from_value(frame.payload)?;
        validate_user_input(&input)?;
        let accepted_content = match (&input.prompt, &input.content) {
            (Some(text), None) => vec![AcceptedContentPart {
                kind: AcceptedPartKind::Text,
                text: Some(text.clone()),
                ..AcceptedContentPart::default()
            }],
            (None, Some(content)) => content.clone(),
            _ => {
                return Err(HostError::InvalidFrame(
                    "prompt or non-empty content is required".to_owned(),
                ));
            }
        };
        let session = SessionKey::new(frame.session_id.clone());
        let service = self.service_for_session(&frame.session_id)?;
        let owner = if let Some(principal) = principal {
            Some(principal.to_owned())
        } else {
            self.state
                .lock()
                .await
                .session_owners
                .get(&frame.session_id)
                .cloned()
        };
        let owner = owner.as_deref();
        let session_spec = service.session_spec(&session);
        let supported = supported_input_modalities(&service, &session);
        let client_capabilities = self
            .state
            .lock()
            .await
            .client_capabilities
            .get(&frame.session_id)
            .cloned()
            .unwrap_or_default();
        let requested_output = input
            .output
            .clone()
            .unwrap_or_else(|| vec![raep_types::Modality::Text]);
        for modality in &requested_output {
            if modality == &raep_types::Modality::Text {
                continue;
            }
            if !self.uploads_enabled()
                || !client_capabilities
                    .features
                    .contains(&raep_types::ClientFeature::OutputParts)
                || !client_capabilities.render.contains(modality)
            {
                return Err(HostError::InvalidFrame(format!(
                    "UNSUPPORTED_MODALITY: client has not negotiated output.parts and {modality:?} rendering"
                )));
            }
            let supported_by_model = session_spec.as_ref().is_some_and(|spec| {
                spec.capabilities
                    .modalities
                    .supports_output(to_norte_modality(modality))
            });
            if !supported_by_model {
                return Err(HostError::InvalidFrame(format!(
                    "UNSUPPORTED_MODALITY: the effective agent cannot produce {modality:?}"
                )));
            }
        }
        let resolved = self
            .resolve_input_content(
                &frame.session_id,
                owner,
                input.prompt,
                input.content,
                session_spec.as_ref(),
                &supported,
            )
            .await?;
        let ResolvedContent {
            text,
            attachments,
            upload_ids,
        } = resolved;
        let queue_policy = if input.replaces_turn_id.is_some() {
            // `queue_policy` does not apply while `replaces_turn_id` is
            // present (RFC 18.2): the input is a rewind-and-re-run in one
            // step, not a queued input. The rewind is applied below before
            // any queueing decision, and the input then runs immediately.
            raep_types::QueuePolicy::Reject
        } else {
            input
                .queue_policy
                .clone()
                .unwrap_or(raep_types::QueuePolicy::Reject)
        };
        // A steer reaches a request in flight, so it carries none of the
        // per-turn shaping the other policies honor.
        if queue_policy == raep_types::QueuePolicy::Steer
            && (input.overrides.is_some()
                || input.response_format.is_some()
                || input.metadata.is_some()
                || requested_output
                    .iter()
                    .any(|modality| modality != &raep_types::Modality::Text))
        {
            return Err(HostError::InvalidFrame(
                "steer accepts text and media only; send overrides, response_format, output parts and metadata on a regular input".to_owned(),
            ));
        }
        // Per-turn `overrides` (RFC 12.4): translate the known fields onto
        // the turn's own reasoning/budget. Model changes are not a per-turn
        // concept in the service, so a model override here is rejected.
        let (override_reasoning, override_model, override_budget) = match input.overrides.clone() {
            Some(ref value) => {
                let decoded = decode_override(value)?;
                validate_override(decoded.0.as_ref(), decoded.1.as_ref(), decoded.2.as_ref())?;
                decoded
            }
            None => (None, None, None),
        };
        if override_model.is_some() {
            return Err(HostError::InvalidFrame(
                "model override is not supported on USER_INPUT; use SESSION_CONFIGURE".to_owned(),
            ));
        }
        // `response_format` (RFC 12.6): carried to the service as
        // `SendMessage.response_format`. See `validate_response_format_shape`
        // for the shape validation and the capability check below.
        let response_format = input
            .response_format
            .clone()
            .map(|format| {
                Ok::<_, HostError>(norte_code_wire::ResponseFormat {
                    kind: match format.kind {
                        ResponseFormatKind::Text => norte_code_wire::ResponseFormatType::Text,
                        ResponseFormatKind::JsonSchema => {
                            norte_code_wire::ResponseFormatType::JsonSchema
                        }
                    },
                    schema: format.schema.clone(),
                    strict: format.strict,
                })
            })
            .transpose()?;
        let queued_turn = QueuedTurn {
            turn_id: turn_id.clone(),
            text,
            attachments,
            budget: override_budget,
            reasoning: override_reasoning,
            metadata: input.metadata.clone(),
            output: requested_output,
            response_format,
            replaces_turn_id: input.replaces_turn_id.clone(),
        };
        let (queued_position, steer_in_flight) = {
            let mut state = self.state.lock().await;
            if let Some(previous_fingerprint) = state.turn_request_fingerprints.get(&turn_key) {
                if previous_fingerprint != &turn_fingerprint {
                    return Err(HostError::IdempotencyConflict(
                        "turn_id was reused with different input".to_owned(),
                    ));
                }
                return Ok(state
                    .turn_request_frames
                    .get(&turn_key)
                    .cloned()
                    .into_iter()
                    .collect());
            }
            let occupied = state.turns.is_occupied(&frame.session_id);
            if occupied && queue_policy == raep_types::QueuePolicy::Reject {
                return Err(HostError::SessionBusy);
            }
            // A steer does not queue: it reaches the turn in flight directly,
            // so the queue's capacity does not bound it either.
            let steer_in_flight = occupied && queue_policy == raep_types::QueuePolicy::Steer;
            let queued = state.turns.queue_len(&frame.session_id);
            if occupied && !steer_in_flight && queued >= self.config.max_queued_inputs {
                return Err(HostError::SessionBusy);
            }
            state
                .turn_request_fingerprints
                .insert(turn_key.clone(), turn_fingerprint);
            state.turn_request_order.push_back(turn_key.clone());
            while state.turn_request_order.len() > self.config.idempotency_capacity {
                if let Some(oldest) = state.turn_request_order.pop_front() {
                    state.turn_request_fingerprints.remove(&oldest);
                    state.turn_request_frames.remove(&oldest);
                }
            }
            if occupied {
                if steer_in_flight {
                    (None, true)
                } else {
                    (
                        Some(state.turns.enqueue(
                            &frame.session_id,
                            queued_turn.clone(),
                            queue_policy == raep_types::QueuePolicy::Interrupt,
                        )),
                        false,
                    )
                }
            } else {
                state.turns.reserve(&frame.session_id, turn_id.clone());
                state.turns.pending_push(&frame.session_id, turn_id.clone());
                if queued_turn
                    .output
                    .iter()
                    .any(|modality| !matches!(modality, raep_types::Modality::Text))
                {
                    state
                        .turns
                        .set_pending_output(&frame.session_id, queued_turn.output.clone());
                }
                (None, false)
            }
        };
        let outcome: Result<Vec<Envelope<Value>>, HostError> = async {
            // `replaces_turn_id` (RFC 18.2): `SESSION_REWIND` to that turn with
            // `inclusive: true` and `cancel_pending: false`, then run the new
            // input as a new turn, in one step. `queue_policy` does not apply
            // (it is forced to `Reject` above), so this input always runs
            // immediately and never occupies the queue. Every hook here runs
            // before queuing: the rewind is a history mutation, and the input
            // that follows it must see the rewound history.
            let mut output_for_rewind: Option<Envelope<Value>> = None;
            if let Some(ref replaces_turn_id) = queued_turn.replaces_turn_id {
                let rewound_turn_ids = self
                    .service
                    .rewind_session(
                        &SessionKey::new(frame.session_id.clone()),
                        replaces_turn_id,
                        true,
                        false,
                        false,
                    )
                    .await
                    .map_err(|error| HostError::Service(error.to_string()))?;
                // The rewind removes `turns > replaces_turn_id` (inclusive) and
                // keeps `turns < replaces_turn_id`. Those removals are the
                // `rewound_turn_ids` the `HistoryRewound` payload reports.
                // There is a side-effect dusting in the rewind's wash: nothing
                // is dehydrated here — the service records no side-effect
                // metadata for removed tool calls, so `side_effects` is empty
                // and `workspace_restored` is false, matching the standalone
                // `SESSION_REWIND` handler.
                output_for_rewind = Some(
                    self.emit(
                        &frame.session_id,
                        StandardFrameType::HistoryRewound,
                        payload(HistoryRewoundPayload {
                            to_turn_id: replaces_turn_id.clone(),
                            inclusive: true,
                            rewound_turn_ids,
                            side_effects: Vec::new(),
                            workspace_restored: false,
                        }),
                    )
                    .await?,
                );
            }
            if !upload_ids.is_empty()
                && let Some(index) = &self.upload_index
                && let Err(error) = index
                    .mark_referenced(&frame.session_id, owner, &upload_ids)
                    .await
            {
                self.remove_turn_request(&frame.session_id, &turn_id).await;
                return Err(error);
            }
            let accepted = match self
                .emit_with_turn_id(
                    &frame.session_id,
                    StandardFrameType::UserInputAccepted,
                    payload(UserInputAcceptedPayload {
                        turn_id: turn_id.clone(),
                        origin: UserInputOrigin {
                            kind: UserInputOriginKind::User,
                        },
                        content: accepted_content,
                        position: queued_position,
                        replaces_turn_id: queued_turn.replaces_turn_id.clone(),
                    }),
                    turn_id.clone(),
                )
                .await
            {
                Ok(frame) => frame,
                Err(error) => {
                    self.remove_turn_request(&frame.session_id, &turn_id).await;
                    return Err(error);
                }
            };
            if queued_position.is_some() {
                let mut output = Vec::new();
                if let Some(rewind_frame) = output_for_rewind {
                    output.push(rewind_frame);
                }
                output.push(accepted);
                output.push(
                    self.emit_with_turn_id(
                        &frame.session_id,
                        StandardFrameType::TurnStateChanged,
                        payload(QueuedTurnStatePayload {
                            state: raep_types::TurnState::Queued,
                        }),
                        turn_id.clone(),
                    )
                    .await?,
                );
                if queue_policy == raep_types::QueuePolicy::Interrupt
                    && let Err(error) = self
                        .submit_service(
                            &frame.session_id,
                            ClientRequestKind::CancelTurn {
                                session: SessionKey::new(frame.session_id.clone()),
                            },
                        )
                        .await
                {
                    let mut state = self.state.lock().await;
                    state.turns.remove_queued(&frame.session_id, &turn_id);
                    drop(state);
                    self.remove_turn_request(&frame.session_id, &turn_id).await;
                    return Err(error);
                }
                return Ok(output);
            }
            if steer_in_flight {
                let outcome = service
                    .steer_input(
                        0,
                        &session,
                        turn_id.as_str(),
                        queued_turn.text.clone(),
                        queued_turn.attachments.clone(),
                    )
                    .await;
                match outcome {
                    Ok(norte_code_agent::SteerOutcome::Injected) => {
                        // The steer joins the turn in flight; the frames it
                        // produces surface on that turn's event stream, which
                        // the request that started the turn is collecting.
                        return Ok(vec![accepted]);
                    }
                    Ok(norte_code_agent::SteerOutcome::Started(events)) => {
                        // The turn the steer started belongs to this session's
                        // aliasing too: the pending entry must exist before the
                        // events are collected, so the TurnStarted aliases the
                        // turn id the client chose. The book still shows the
                        // finished turn it replaced; `activate` overwrites it.
                        {
                            let mut state = self.state.lock().await;
                            state.turns.pending_push(&frame.session_id, turn_id.clone());
                        }
                        let events = self
                            .collect_service_events(&frame.session_id, events)
                            .await?;
                        let mut output = vec![accepted];
                        output.extend(events);
                        return Ok(output);
                    }
                    Err(error) => {
                        self.remove_turn_request(&frame.session_id, &turn_id).await;
                        return Err(HostError::Service(error.to_string()));
                    }
                }
            }
            let submission = self
                .submit_service(
                    &frame.session_id,
                    ClientRequestKind::SendMessage {
                        session: Some(SessionKey::new(frame.session_id.clone())),
                        text: queued_turn.text,
                        attachments: queued_turn.attachments,
                        budget: queued_turn.budget,
                        reasoning: queued_turn.reasoning,
                        output: Vec::new(),
                        response_format: queued_turn.response_format,
                        metadata: queued_turn.metadata,
                        replay: false,
                    },
                )
                .await;
            let mut output = Vec::new();
            if let Some(rewind_frame) = output_for_rewind {
                output.push(rewind_frame);
            }
            output.push(accepted);
            match submission {
                Ok(events) => output.extend(events),
                Err(error) => {
                    let mut state = self.state.lock().await;
                    state.turns.release(&frame.session_id, &turn_id);
                    drop(state);
                    self.remove_turn_request(&frame.session_id, &turn_id).await;
                    return Err(error);
                }
            }
            Ok(output)
        }
        .await;
        if outcome.is_err() {
            self.release_turn(&frame.session_id, &turn_id).await;
        }
        outcome
    }
    #[allow(clippy::too_many_lines)]
    async fn resolve_input_content(
        &self,
        session_id: &str,
        owner: Option<&str>,
        prompt: Option<String>,
        content: Option<Vec<AcceptedContentPart>>,
        session_spec: Option<&norte_code_agent::ModelSpec>,
        supported: &[norte::Modality],
    ) -> Result<ResolvedContent, HostError> {
        let mut upload_ids = Vec::new();
        let (text, attachments) = match (prompt, content) {
            (Some(text), None) => (text, Vec::new()),
            (None, Some(parts)) => {
                let mut text = String::new();
                let mut attachments = Vec::new();
                let mut modality_counts = HashMap::<norte::Modality, u32>::new();
                for (part_index, part) in parts.into_iter().enumerate() {
                    if part.kind == AcceptedPartKind::Text {
                        text.push_str(part.text.as_deref().ok_or_else(|| {
                            HostError::InvalidFrame("text content part is missing text".to_owned())
                        })?);
                        continue;
                    }
                    let modality = match part.kind {
                        AcceptedPartKind::Image => norte::Modality::Image,
                        AcceptedPartKind::Document => norte::Modality::Document,
                        AcceptedPartKind::Audio => norte::Modality::Audio,
                        AcceptedPartKind::Video => norte::Modality::Video,
                        AcceptedPartKind::Text => continue,
                    };
                    if !supported.contains(&modality) {
                        return Err(HostError::InvalidFrame(format!(
                            "UNSUPPORTED_MODALITY: part {part_index} requires {modality:?}; supported: {supported:?}"
                        )));
                    }
                    let modality_count = modality_counts.entry(modality).or_default();
                    *modality_count = modality_count.saturating_add(1);
                    let modality_limits = session_spec
                        .as_ref()
                        .and_then(|spec| spec.capabilities.modalities.limits(modality));
                    if modality_limits
                        .and_then(|limits| limits.max_count)
                        .is_some_and(|max| *modality_count > max)
                    {
                        return Err(HostError::InvalidFrame(format!(
                            "UNSUPPORTED_MODALITY: too many {modality:?} parts"
                        )));
                    }
                    let mime = part.mime_type.clone().ok_or_else(|| {
                        HostError::InvalidFrame("media content requires mime_type".to_owned())
                    })?;
                    validate_media_mime(part.kind, &mime).map_err(HostError::InvalidFrame)?;
                    let mime_type = norte::MimeType::new(mime.clone())
                        .map_err(|error| HostError::InvalidFrame(error.to_string()))?;
                    if modality_limits.is_some_and(|limits| !limits.accepts_mime(&mime_type)) {
                        return Err(HostError::InvalidFrame(format!(
                            "UNSUPPORTED_MIME_TYPE: part {part_index} does not accept `{mime}`"
                        )));
                    }
                    let (source_kind, asset_id) = match part.source.as_ref() {
                        Some(AcceptedSource::Upload { upload_id }) => {
                            (AssetSourceKind::Upload, upload_id.as_str())
                        }
                        Some(AcceptedSource::Output { output_id }) => {
                            (AssetSourceKind::Output, output_id.as_str())
                        }
                        None => {
                            return Err(HostError::InvalidFrame("source is required".to_owned()));
                        }
                    };
                    let upload = self
                        .resolve_session_asset(
                            session_id,
                            asset_id,
                            owner,
                            source_kind == AssetSourceKind::Output,
                        )
                        .await?;
                    if modality_limits
                        .and_then(|limits| limits.max_bytes)
                        .is_some_and(|max| upload.bytes > max)
                    {
                        return Err(HostError::InvalidFrame(format!(
                            "MEDIA_TOO_LARGE: part {part_index} exceeds the {modality:?} byte limit"
                        )));
                    }
                    if upload.mime_type != mime {
                        return Err(HostError::InvalidFrame(format!(
                            "upload MIME type `{}` does not match content MIME type `{mime}`",
                            upload.mime_type
                        )));
                    }
                    let blob = upload.blob_id.parse().map_err(|_| {
                        HostError::Artifact("stored upload has an invalid blob id".to_owned())
                    })?;
                    let name = part
                        .name
                        .as_deref()
                        .or(part.alt.as_deref())
                        .unwrap_or(asset_id)
                        .to_owned();
                    if let Some(alt) = &part.alt {
                        text.push_str("\n[Media description: ");
                        text.push_str(alt);
                        text.push(']');
                    }
                    if source_kind == AssetSourceKind::Upload {
                        upload_ids.push(asset_id.to_owned());
                    }
                    attachments.push(norte_code_wire::Attachment {
                        path: String::new(),
                        blob: Some(blob),
                        mime,
                        name,
                        duration_ms: part.duration_ms,
                        transcript: part.transcript,
                    });
                }
                (text, attachments)
            }
            _ => {
                return Err(HostError::InvalidFrame(
                    "prompt or non-empty content is required".to_owned(),
                ));
            }
        };
        Ok(ResolvedContent {
            text,
            attachments,
            upload_ids,
        })
    }
}
