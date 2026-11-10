//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use crate::NorteCodeExtension;
use crate::payloads::payload;
use norte_code_wire::{
    ClientRequest, ClientRequestKind, Delta, ServerEvent, ServerEventKind, SessionKey,
};
use raep_types::{AttachMode, Envelope, FrameType, StandardFrameType, TurnId, WIRE_VERSION};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
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
    /// Handles one decoded RAEP frame.
    ///
    /// Service events are published as soon as they arrive, while the
    /// returned vector provides the same frames to a request/response
    /// transport. This keeps transports independent from the agent loop.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame is malformed, unsupported, unauthenticated
    /// for the current attachment, or rejected by the Norte Code service.
    ///
    pub async fn handle(&self, frame: Envelope<Value>) -> Result<Vec<Envelope<Value>>, HostError> {
        self.handle_as(frame, None).await
    }
    /// Handles one frame while enforcing ownership for `principal`.
    ///
    /// `None` is reserved for in-process callers and keeps the existing local
    /// behavior. HTTP and WebSocket adapters pass the authenticated principal.
    ///
    /// # Errors
    ///
    /// Returns an error when the frame is invalid or the principal does not
    /// own the requested session.
    #[allow(clippy::too_many_lines)]
    pub async fn handle_as(
        &self,
        frame: Envelope<Value>,
        principal: Option<&str>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        if frame.version != WIRE_VERSION {
            return Err(HostError::InvalidFrame(
                "unsupported RAEP wire version".to_owned(),
            ));
        }
        if frame.seq.is_some() {
            return Err(HostError::InvalidFrame(
                "client frames must not include seq".to_owned(),
            ));
        }
        let frame_fingerprint = crate::bundle::sha256(&serde_json::to_vec(&frame)?);
        let idempotency_key = (frame.session_id.clone(), frame.id.clone());
        self.load_persisted_idempotency(&frame.session_id).await?;
        let is_user_input = matches!(
            &frame.kind,
            FrameType::Standard(StandardFrameType::UserInput)
        );
        let turn_id = is_user_input.then(|| frame.turn_id.clone()).flatten();
        let turn_fingerprint = if is_user_input {
            Some(crate::bundle::sha256(&serde_json::to_vec(&frame.payload)?))
        } else {
            None
        };
        if let Some(principal) = principal
            && !frame.session_id.is_empty()
        {
            self.claim_session(&frame.session_id, principal).await?;
        }
        if !matches!(
            &frame.kind,
            FrameType::Standard(
                StandardFrameType::SessionAttach | StandardFrameType::SessionDetach,
            )
        ) && self
            .state
            .lock()
            .await
            .attachment_modes
            .get(&frame.session_id)
            .is_some_and(|mode| *mode == AttachMode::Observer)
            && !observer_read_only_frame(&frame.kind)
        {
            return Err(HostError::InvalidFrame(
                "FORBIDDEN: observer attachments are read-only".to_owned(),
            ));
        }
        {
            let state = self.state.lock().await;
            if let Some(previous) = state.idempotency.get(&idempotency_key) {
                if previous.fingerprint == frame_fingerprint {
                    if matches!(
                        &frame.kind,
                        FrameType::Standard(StandardFrameType::UserInput)
                    ) && let Some(turn_id) = &frame.turn_id
                        && let Some(latest) = state
                            .turn_request_frames
                            .get(&(frame.session_id.clone(), turn_id.clone()))
                    {
                        return Ok(vec![latest.clone()]);
                    }
                    return Ok(previous.response.clone());
                }
                return Err(HostError::IdempotencyConflict(
                    "frame id was reused with a different request".to_owned(),
                ));
            }
        }
        let is_attach = matches!(
            &frame.kind,
            FrameType::Standard(StandardFrameType::SessionAttach)
        );
        let result = match frame.kind {
            FrameType::Standard(StandardFrameType::SessionAttach) => self.attach(frame).await,
            FrameType::Standard(StandardFrameType::UserInput) => {
                self.user_input(frame, principal).await
            }
            FrameType::Standard(StandardFrameType::CancelTurn) => self.cancel(frame).await,
            FrameType::Standard(StandardFrameType::SessionDetach) => self.detach(frame).await,
            FrameType::Standard(StandardFrameType::SessionClose) => self.close(frame).await,
            FrameType::Standard(StandardFrameType::Ack) => self.ack(frame).await,
            FrameType::Standard(StandardFrameType::SessionConfigure) => {
                self.agent_command(frame).await
            }
            FrameType::Standard(StandardFrameType::HistoryGet) => self.history_get(frame).await,
            FrameType::Standard(StandardFrameType::CancelJob) => self.cancel_job(frame).await,
            FrameType::Standard(StandardFrameType::JobList) => self.job_list(frame).await,
            FrameType::Standard(StandardFrameType::JobGet) => self.job_get(frame).await,
            FrameType::Standard(StandardFrameType::SessionFork) => self.fork(frame).await,
            FrameType::Standard(StandardFrameType::SessionCompact) => self.compact(frame).await,
            FrameType::Standard(StandardFrameType::ContextPin) => self.context_pin(frame).await,
            FrameType::Standard(StandardFrameType::SessionExport) => {
                self.export(frame, principal).await
            }
            FrameType::Standard(StandardFrameType::SessionImport) => Self::import(),
            FrameType::Standard(StandardFrameType::ToolApprovalResponse) => {
                Self::approval_response(&frame)
            }
            FrameType::Standard(StandardFrameType::SessionRewind) => self.rewind(frame).await,
            FrameType::Standard(StandardFrameType::InputStreamStart) => {
                Self::input_stream(&frame, "start")
            }
            FrameType::Standard(StandardFrameType::InputStreamData) => {
                Self::input_stream(&frame, "data")
            }
            FrameType::Standard(StandardFrameType::InputStreamEnd) => {
                Self::input_stream(&frame, "end")
            }
            FrameType::Standard(StandardFrameType::ClientToolResult) => {
                Self::client_tool_result(&frame)
            }
            FrameType::Standard(StandardFrameType::PresenceUpdate) => Self::presence_update(&frame),
            FrameType::Standard(StandardFrameType::DelegatorControl) => {
                self.delegator_control(frame).await
            }
            FrameType::Standard(StandardFrameType::Handoff) => Self::handoff(&frame),
            FrameType::Standard(StandardFrameType::AuthRefresh) => Self::auth_refresh(&frame),
            FrameType::Extension(ref name) if name == NorteCodeExtension::Command.as_str() => {
                self.extension_command(frame).await
            }
            kind => Err(HostError::UnknownType(format!("{kind:?}"))),
        }?;
        if is_attach && let Some(principal) = principal {
            for attached in &result {
                if !attached.session_id.is_empty() {
                    self.claim_session(&attached.session_id, principal).await?;
                }
            }
        }
        let service_turn_id = if let Some(turn_id) = &turn_id {
            self.state
                .lock()
                .await
                .turn_aliases
                .iter()
                .find(|((session_id, _), alias)| {
                    session_id == &idempotency_key.0 && *alias == turn_id
                })
                .map(|((_, service_turn), _)| *service_turn)
        } else {
            None
        };
        if let Some(store) = &self.replay_store {
            store
                .save_idempotency(&IdempotencyRecord {
                    session_id: idempotency_key.0.clone(),
                    frame_id: idempotency_key.1.clone(),
                    fingerprint: frame_fingerprint.clone(),
                    turn_id,
                    turn_fingerprint,
                    service_turn_id,
                    response: result.clone(),
                })
                .await?;
        }
        let mut state = self.state.lock().await;
        state.idempotency.insert(
            idempotency_key.clone(),
            CachedRequest {
                fingerprint: frame_fingerprint,
                response: result.clone(),
            },
        );
        state.idempotency_order.push_back(idempotency_key);
        while state.idempotency_order.len() > self.config.idempotency_capacity {
            if let Some(oldest) = state.idempotency_order.pop_front() {
                state.idempotency.remove(&oldest);
            }
        }
        Ok(result)
    }
    /// Handles a standard `SESSION_CONFIGURE` frame (RFC 8.1).
    ///
    /// The payload is the standard one — `agent_id`, `agent_override`,
    /// `options`, `apply` — not a serialised [`AgentCommand`]. Known
    /// override fields are translated onto service requests; anything the
    /// host cannot express fails with `AGENT_OVERRIDE_DENIED` instead of
    /// being silently dropped.
    pub(crate) async fn agent_command(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let configure: SessionConfigurePayload = serde_json::from_value(frame.payload)?;
        let job_turn_policy = parse_job_turn_policy(configure.options.as_ref())?;
        if job_turn_policy.is_some()
            && configure
                .apply
                .is_some_and(|apply| apply != ConfigureApplyPolicy::Now)
        {
            return Err(HostError::InvalidFrame(
                "SESSION_OPTIONS_UNSUPPORTED: job_turn_policy only supports apply=now".to_owned(),
            ));
        }
        let session_id = frame.session_id;
        let session = SessionKey::new(session_id.clone());
        if let Some(agent_id) = &configure.agent_id
            && agent_id != &self.config.agent_id
        {
            return Err(HostError::InvalidFrame(format!(
                "agent_mismatch: session runs agent `{}`",
                self.config.agent_id
            )));
        }
        let override_value = configure.agent_override.unwrap_or(Value::Null);
        let (reasoning, model, budget) = if override_value.is_null() {
            (None, None, None)
        } else {
            let decoded = decode_override(&override_value)?;
            validate_override(decoded.0.as_ref(), decoded.1.as_ref(), decoded.2.as_ref())?;
            decoded
        };
        let mut output = Vec::new();
        if let Some((provider, model_id)) = model {
            output.extend(
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
        if let Some(reasoning) = reasoning {
            output.extend(
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
        if let Some(budget) = budget {
            output.extend(
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
        if let Some(policy) = job_turn_policy {
            let service = self.service_for_session(&session_id).await?;
            if !service.set_job_turn_policy(&session, policy) {
                return Err(HostError::InvalidFrame(
                    "session not found while applying job_turn_policy".to_owned(),
                ));
            }
        }
        if output.is_empty() {
            output.push(
                self.emit(
                    &session_id,
                    StandardFrameType::SessionConfigured,
                    payload(SessionConfiguredPayload { applied: true }),
                )
                .await?,
            );
        }
        Ok(output)
    }
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
        let service = self.service_for_session(&frame.session_id).await?;
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
        let mut upload_ids = Vec::new();
        let (text, attachments) = match (input.prompt, input.content) {
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
                            &frame.session_id,
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
        let queue_mode = if input.replaces_turn_id.is_some() {
            // `queue_policy` does not apply while `replaces_turn_id` is
            // present (RFC 18.2): the input is a rewind-and-re-run in one
            // step, not a queued input. The rewind is applied below before
            // any queueing decision, and the input then runs immediately.
            norte_code_wire::QueueMode::Reject
        } else {
            match input.queue_policy.clone() {
                Some(raep_types::QueuePolicy::Queue) => norte_code_wire::QueueMode::Queue,
                Some(raep_types::QueuePolicy::Interrupt) => norte_code_wire::QueueMode::Interrupt,
                Some(raep_types::QueuePolicy::Reject) | None => norte_code_wire::QueueMode::Reject,
            }
        };
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
        let queued_position = {
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
            let occupied = state.active_turns.contains_key(&frame.session_id)
                || state.turn_reservations.contains_key(&frame.session_id);
            if occupied && queue_mode == norte_code_wire::QueueMode::Reject {
                return Err(HostError::SessionBusy);
            }
            let queued = state
                .queued_turns
                .get(&frame.session_id)
                .map_or(0, VecDeque::len);
            if occupied && queued >= self.config.max_queued_inputs {
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
                let queue = state
                    .queued_turns
                    .entry(frame.session_id.clone())
                    .or_default();
                if queue_mode == norte_code_wire::QueueMode::Interrupt {
                    queue.push_front(queued_turn.clone());
                    Some(1)
                } else {
                    queue.push_back(queued_turn.clone());
                    Some(u64::try_from(queue.len()).unwrap_or(u64::MAX))
                }
            } else {
                state
                    .turn_reservations
                    .insert(frame.session_id.clone(), turn_id.clone());
                state
                    .pending_turn_ids
                    .entry(frame.session_id.clone())
                    .or_default()
                    .push_back(turn_id.clone());
                if queued_turn
                    .output
                    .iter()
                    .any(|modality| !matches!(modality, raep_types::Modality::Text))
                {
                    state
                        .pending_output
                        .insert(frame.session_id.clone(), queued_turn.output.clone());
                }
                None
            }
        };
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
            if queue_mode == norte_code_wire::QueueMode::Interrupt
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
                if let Some(queue) = state.queued_turns.get_mut(&frame.session_id) {
                    queue.retain(|queued| queued.turn_id != turn_id);
                }
                drop(state);
                self.remove_turn_request(&frame.session_id, &turn_id).await;
                return Err(error);
            }
            return Ok(output);
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
                state.pending_output.remove(&frame.session_id);
                state.pending_turn_ids.remove(&frame.session_id);
                state.turn_reservations.remove(&frame.session_id);
                drop(state);
                self.remove_turn_request(&frame.session_id, &turn_id).await;
                return Err(error);
            }
        }
        Ok(output)
    }
    pub(crate) async fn cancel(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let requested_turn = frame.turn_id.as_ref().ok_or_else(|| {
            HostError::InvalidFrame("CANCEL_TURN requires envelope.turn_id".to_owned())
        })?;
        let (active_turn, cancelled_queued_turn) = {
            let mut state = self.state.lock().await;
            let active_turn = state.active_turns.get(&frame.session_id).cloned();
            // A turn that is running but has not produced its first event yet
            // lives only in the reservation: the host reserved it when the
            // input was accepted and learned nothing since. Treating that as
            // "turn not active" refused the only cancel the client could send,
            // and left the session with a reservation nothing could clear.
            let reserved = state.turn_reservations.get(&frame.session_id).cloned();
            if active_turn.as_ref() == Some(requested_turn)
                || (active_turn.is_none() && reserved.as_ref() == Some(requested_turn))
            {
                (Some(requested_turn.clone()), false)
            } else {
                let Some(queue) = state.queued_turns.get_mut(&frame.session_id) else {
                    return Err(HostError::TurnNotActive);
                };
                let Some(position) = queue
                    .iter()
                    .position(|queued| &queued.turn_id == requested_turn)
                else {
                    return Err(HostError::TurnNotActive);
                };
                queue.remove(position);
                if queue.is_empty() {
                    state.queued_turns.remove(&frame.session_id);
                }
                if let Some(pending) = state.pending_turn_ids.get_mut(&frame.session_id) {
                    pending.retain(|turn_id| turn_id != requested_turn);
                }
                (active_turn, true)
            }
        };
        if cancelled_queued_turn {
            return self
                .emit_with_turn_id(
                    &frame.session_id,
                    StandardFrameType::TurnStateChanged,
                    payload(raep_types::TurnStateChanged {
                        state: raep_types::TurnState::Cancelled,
                        stop_reason: Some(raep_types::StopReason::Cancelled),
                        usage: None,
                        error: None,
                    }),
                    requested_turn.clone(),
                )
                .await
                .map(|frame| vec![frame]);
        }
        let active_turn = active_turn.ok_or(HostError::TurnNotActive)?;
        if &active_turn != requested_turn {
            return Err(HostError::TurnNotActive);
        }
        let request = ClientRequest {
            request_id: 0,
            kind: ClientRequestKind::CancelTurn {
                session: SessionKey::new(frame.session_id.clone()),
            },
        };
        let events = self
            .service
            .submit(request)
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        let collected = self
            .collect_service_events(&frame.session_id, events)
            .await?;
        // The service cancels the token, but an engine parked in a tool sends
        // nothing, so the turn's reservation would stay and every later input
        // would meet `SESSION_BUSY`. The cancel itself is the one place that
        // knows the turn is over from the client's point of view.
        let mut released = self.release_turn(&frame.session_id, requested_turn).await;
        if collected.is_empty() {
            released.push(
                self.emit_with_turn_id(
                    &frame.session_id,
                    StandardFrameType::TurnStateChanged,
                    payload(raep_types::TurnStateChanged {
                        state: raep_types::TurnState::Cancelled,
                        stop_reason: Some(raep_types::StopReason::Cancelled),
                        usage: None,
                        error: None,
                    }),
                    requested_turn.clone(),
                )
                .await?,
            );
        }
        let mut output = collected;
        output.append(&mut released);
        Ok(output)
    }
    /// Drops the host's record of a running turn, answering the frames that
    /// tell the client it ended.
    pub(crate) async fn release_turn(
        &self,
        session_id: &str,
        turn_id: &TurnId,
    ) -> Vec<Envelope<Value>> {
        {
            let mut state = self.state.lock().await;
            if state.active_turns.get(session_id) == Some(turn_id) {
                state.active_turns.remove(session_id);
            }
            if state.turn_reservations.get(session_id) == Some(turn_id) {
                state.turn_reservations.remove(session_id);
                state.pending_output.remove(session_id);
            }
            if let Some(pending) = state.pending_turn_ids.get_mut(session_id) {
                pending.retain(|pending_turn| pending_turn != turn_id);
                if pending.is_empty() {
                    state.pending_turn_ids.remove(session_id);
                }
            }
        }
        self.remove_turn_request(session_id, turn_id).await;
        Vec::new()
    }
    pub(crate) async fn detach(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let mut state = self.state.lock().await;
        state.attached.remove(&frame.session_id);
        state.attachment_modes.remove(&frame.session_id);
        Ok(Vec::new())
    }
    pub(crate) async fn close(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let purge = frame
            .payload
            .get("purge")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let events = self
            .submit_service(
                &frame.session_id,
                ClientRequestKind::DeleteSession {
                    session: SessionKey::new(frame.session_id.clone()),
                },
            )
            .await?;
        let mut output = events;
        output.push(
            self.emit(
                &frame.session_id,
                StandardFrameType::SessionClosed,
                payload(SessionClosedPayload {
                    reason: if purge {
                        SessionClosedReason::Purged
                    } else {
                        SessionClosedReason::Client
                    },
                }),
            )
            .await?,
        );
        {
            let mut state = self.state.lock().await;
            state.attached.remove(&frame.session_id);
            state.attachment_modes.remove(&frame.session_id);
        }
        Ok(output)
    }
    pub(crate) async fn ack(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let last_seq = frame
            .payload
            .get("last_seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| HostError::InvalidFrame("ACK.last_seq is required".to_owned()))?;
        let mut state = self.state.lock().await;
        if last_seq
            > state
                .sequences
                .get(&frame.session_id)
                .copied()
                .unwrap_or_default()
        {
            return Err(HostError::InvalidFrame(
                "ACK.last_seq exceeds the last emitted sequence".to_owned(),
            ));
        }
        if let Some(frames) = state.replay.get_mut(&frame.session_id) {
            frames.retain(|stored| stored.seq.is_none_or(|seq| seq > last_seq));
        }
        Ok(Vec::new())
    }
    pub(crate) async fn submit_service(
        &self,
        session_id: &str,
        kind: ClientRequestKind,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind,
            })
            .await
            .map_err(|error| HostError::Service(error.to_string()))?;
        self.collect_service_events(session_id, stream).await
    }
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn collect_service_events(
        &self,
        session_id: &str,
        mut stream: norte_code_agent::EventStream,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        use futures_util::StreamExt;
        let mut output = Vec::new();
        while let Some(event) = stream.next().await {
            let event_session = event
                .session
                .as_ref()
                .map_or_else(|| session_id.to_owned(), |key| key.as_str().to_owned());
            if let ServerEventKind::TurnStarted { turn } = &event.kind {
                let mut state = self.state.lock().await;
                let client_turn_id = state
                    .pending_turn_ids
                    .get_mut(&event_session)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or_else(|| {
                        TurnId::new(turn.to_string()).expect("numeric turn id is valid")
                    });
                state
                    .turn_aliases
                    .insert((event_session.clone(), *turn), client_turn_id.clone());
                state
                    .turn_alias_order
                    .push_back((event_session.clone(), *turn));
                while state.turn_alias_order.len() > self.config.idempotency_capacity {
                    if let Some(oldest) = state.turn_alias_order.pop_front() {
                        state.turn_aliases.remove(&oldest);
                    }
                }
                state
                    .active_turns
                    .insert(event_session.clone(), client_turn_id);
                state.turn_reservations.remove(&event_session);
                drop(state);
                let requested = self
                    .state
                    .lock()
                    .await
                    .pending_output
                    .remove(&event_session);
                if let Some(requested) = requested {
                    self.state
                        .lock()
                        .await
                        .turn_output
                        .insert((event_session.clone(), *turn), requested);
                }
            }
            if let ServerEventKind::Delta {
                delta: Delta::Media { .. },
                ..
            } = &event.kind
            {
                continue;
            }
            if let ServerEventKind::MediaPartCompleted { turn, index, media } = &event.kind {
                let requested = self
                    .state
                    .lock()
                    .await
                    .turn_output
                    .get(&(event_session.clone(), *turn))
                    .cloned()
                    .unwrap_or_default();
                if requested.contains(&raep_modality_for_media(media.kind)) {
                    output.extend(
                        self.emit_media_output(&event_session, *turn, *index, media)
                            .await?,
                    );
                }
                continue;
            }
            let tool_state_change = match &event.kind {
                ServerEventKind::ToolStarted { turn, .. } => {
                    Some((*turn, raep_types::TurnState::ExecutingTool))
                }
                _ => None,
            };
            let finished_tool_turn = match &event.kind {
                ServerEventKind::ToolFinished { turn, .. } => Some(*turn),
                _ => None,
            };
            if let Some((turn, state)) = tool_state_change {
                let turn_id = self
                    .state
                    .lock()
                    .await
                    .turn_aliases
                    .get(&(event_session.clone(), turn))
                    .cloned();
                if let Some(turn_id) = turn_id {
                    output.push(
                        self.emit_with_turn_id(
                            &event_session,
                            StandardFrameType::TurnStateChanged,
                            payload(raep_types::TurnStateChanged {
                                state,
                                stop_reason: None,
                                usage: None,
                                error: None,
                            }),
                            turn_id,
                        )
                        .await?,
                    );
                }
            }
            let (finished_turn, stopped_turn_id) = match &event.kind {
                ServerEventKind::TurnFinished { turn, .. } => (Some(*turn), None),
                ServerEventKind::TurnStopped { .. } => {
                    let state = self.state.lock().await;
                    let client_turn_id = state.active_turns.get(&event_session).cloned();
                    let service_turn_id = client_turn_id.as_ref().and_then(|client_turn_id| {
                        state
                            .turn_aliases
                            .iter()
                            .find(|((session, _), alias)| {
                                session == &event_session && *alias == client_turn_id
                            })
                            .map(|((_, turn), _)| *turn)
                    });
                    (service_turn_id, client_turn_id)
                }
                _ => (None, None),
            };
            output.push(
                self.emit_event_with_turn_id(session_id, event, stopped_turn_id.clone())
                    .await?,
            );
            if let Some(turn) = finished_tool_turn {
                let turn_id = self
                    .state
                    .lock()
                    .await
                    .turn_aliases
                    .get(&(event_session.clone(), turn))
                    .cloned();
                if let Some(turn_id) = turn_id {
                    output.push(
                        self.emit_with_turn_id(
                            &event_session,
                            StandardFrameType::TurnStateChanged,
                            payload(raep_types::TurnStateChanged {
                                state: raep_types::TurnState::CallingProvider,
                                stop_reason: None,
                                usage: None,
                                error: None,
                            }),
                            turn_id,
                        )
                        .await?,
                    );
                }
            }
            if finished_turn.is_some() || stopped_turn_id.is_some() {
                {
                    let mut state = self.state.lock().await;
                    if let Some(turn) = finished_turn {
                        state.turn_output.remove(&(event_session.clone(), turn));
                    }
                    state.active_turns.remove(&event_session);
                    state.turn_reservations.remove(&event_session);
                }
                if let Some(next) = self.reserve_next_queued_turn(&event_session).await {
                    output.extend(self.start_queued_turn(&event_session, next).await?);
                }
            }
        }
        self.state.lock().await.pending_output.remove(session_id);
        Ok(output)
    }
    pub(crate) async fn reserve_next_queued_turn(&self, session_id: &str) -> Option<QueuedTurn> {
        let mut state = self.state.lock().await;
        let next = state
            .queued_turns
            .get_mut(session_id)
            .and_then(VecDeque::pop_front);
        if state
            .queued_turns
            .get(session_id)
            .is_some_and(VecDeque::is_empty)
        {
            state.queued_turns.remove(session_id);
        }
        let next = next?;
        state
            .turn_reservations
            .insert(session_id.to_owned(), next.turn_id.clone());
        state
            .pending_turn_ids
            .entry(session_id.to_owned())
            .or_default()
            .push_back(next.turn_id.clone());
        if next
            .output
            .iter()
            .any(|modality| !matches!(modality, raep_types::Modality::Text))
        {
            state
                .pending_output
                .insert(session_id.to_owned(), next.output.clone());
        }
        Some(next)
    }
    pub(crate) async fn start_queued_turn(
        &self,
        session_id: &str,
        turn: QueuedTurn,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        // `replaces_turn_id` is a one-step rewind-and-re-run: the rewind has
        // already happened when the input was accepted (see
        // `NorteCodeRaepHost::user_input`), so by the time a queued turn runs
        // there is nothing left to rewind. The queued turn simply carries the
        // response format the client asked for.
        let stream = self
            .service
            .submit(ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::SendMessage {
                    session: Some(SessionKey::new(session_id)),
                    text: turn.text,
                    attachments: turn.attachments,
                    budget: turn.budget,
                    reasoning: turn.reasoning,
                    output: Vec::new(),
                    response_format: turn.response_format,
                    metadata: turn.metadata,
                    replay: false,
                },
            })
            .await;
        match stream {
            Ok(stream) => Box::pin(self.collect_service_events(session_id, stream)).await,
            Err(error) => {
                let message = error.to_string();
                {
                    let mut state = self.state.lock().await;
                    state.turn_reservations.remove(session_id);
                    state.pending_turn_ids.remove(session_id);
                    state.pending_output.remove(session_id);
                }
                let terminal = self
                    .emit_with_turn_id(
                        session_id,
                        StandardFrameType::TurnStateChanged,
                        payload(raep_types::TurnStateChanged {
                            state: raep_types::TurnState::Failed,
                            stop_reason: Some(raep_types::StopReason::Error),
                            usage: None,
                            error: Some(raep_types::ErrorPayload {
                                code: "UNAVAILABLE".to_owned(),
                                message,
                                retryable: Some(true),
                                details: None,
                            }),
                        }),
                        turn.turn_id,
                    )
                    .await?;
                let mut output = vec![terminal];
                if let Some(next) = self.reserve_next_queued_turn(session_id).await {
                    output.extend(Box::pin(self.start_queued_turn(session_id, next)).await?);
                }
                Ok(output)
            }
        }
    }
    /// Delegator control (RFC 8.2.11): `suspend`/`release` of the delegator.
    pub(crate) async fn delegator_control(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let action = frame
            .payload
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let child_session_id = frame
            .payload
            .get("child_session_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let child = SessionKey::new(child_session_id.clone());
        let (suspended, queued) = match action.as_str() {
            "suspend" => self
                .service
                .set_delegator_suspended(&child, true)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
            "release" => self
                .service
                .set_delegator_suspended(&child, false)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "DELEGATOR_CONTROL: unknown action `{other}`; use suspend or release"
                )));
            }
        };
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::DelegatorStateChanged,
                payload(DelegatorStateChangedPayload {
                    child_session_id: &child_session_id,
                    suspended,
                    by: DelegatorActor::Client,
                    queued_messages: queued,
                }),
            )
            .await?,
        ])
    }
    pub(crate) async fn extension_command(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let request: ClientRequest = serde_json::from_value(frame.payload)?;
        if is_plugin_request(&request.kind) {
            return self.plugin_command(&frame.session_id, request).await;
        }
        // Connection-scoped commands deliberately ignore the route's
        // placeholder session id. The HTTP adapter has a session-shaped URL
        // for every command, but listing sessions, creating one, and querying
        // host-wide catalogues must work before any session is attached.
        let session_id = session_for_request(&request.kind)
            .map(std::string::ToString::to_string)
            .unwrap_or_default();
        // Reads that only look at what already happened (telemetry, cost,
        // jobs, history) do not need a live attachment: the session exists or
        // it does not, and a dashboard opening a session to read its usage
        // would otherwise have to attach — and displace the stream a
        // conversation page is already holding.
        if !session_id.is_empty() && !is_read_only_request(&request.kind) {
            self.require_attached(&session_id).await?;
        }
        let service = if session_id.is_empty() {
            Arc::clone(&self.service)
        } else {
            self.service_for_session(&session_id).await?
        };
        self.collect_service_events(
            &session_id,
            service
                .submit(request)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
        )
        .await
    }
    pub(crate) async fn plugin_command(
        &self,
        session_id: &str,
        request: ClientRequest,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        let event = match request.kind {
            ClientRequestKind::ListPlugins => ServerEventKind::PluginList {
                plugins: crate::plugin_manager::plugin_info(),
            },
            ClientRequestKind::InstallPlugin { url, package, all } => {
                let result = tokio::task::spawn_blocking(move || {
                    if all {
                        crate::plugin_manager::install_all_plugins(&url, &|_, _, _| {})
                            .map(|names| names.join(", "))
                    } else {
                        crate::plugin_manager::install_plugin(
                            &url,
                            package.as_deref(),
                            &|_, _, _| {},
                        )
                    }
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: "installed".to_owned(),
                    name: result,
                }
            }
            ClientRequestKind::UpdatePlugins { name } => {
                let result = tokio::task::spawn_blocking(move || {
                    crate::plugin_manager::update_plugins(name.as_deref())
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                let updated = summarize_plugin_updates(result).map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: "updated".to_owned(),
                    name: updated,
                }
            }
            ClientRequestKind::RemovePlugin { name } => {
                tokio::task::spawn_blocking({
                    let name = name.clone();
                    move || crate::plugin_manager::remove_plugin(&name)
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: "removed".to_owned(),
                    name,
                }
            }
            ClientRequestKind::SetPluginEnabled { name, enabled } => {
                tokio::task::spawn_blocking({
                    let name = name.clone();
                    move || crate::plugin_manager::set_plugin_enabled(&name, enabled)
                })
                .await
                .map_err(|error| HostError::Service(error.to_string()))?
                .map_err(HostError::Service)?;
                ServerEventKind::PluginChanged {
                    operation: if enabled { "enabled" } else { "disabled" }.to_owned(),
                    name,
                }
            }
            _ => unreachable!("plugin request was checked before dispatch"),
        };
        self.emit_event(
            session_id,
            ServerEvent {
                request_id: request.request_id,
                session: None,
                kind: event,
            },
        )
        .await
        .map(|frame| vec![frame])
    }
}
