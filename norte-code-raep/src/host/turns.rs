//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use crate::NorteCodeExtension;
use crate::payloads::payload;
use norte_code_wire::plugin_widgets::PluginSelection;
use norte_code_wire::{ClientRequest, ClientRequestKind, ServerEvent, ServerEventKind};
use raep_types::{AttachMode, Envelope, FrameType, StandardFrameType, WIRE_VERSION};
use serde_json::Value;
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

/// Largest response body the host keeps for a duplicate frame.
///
/// A response above this is re-run instead of held: the journal is a cache of
/// answers, not a second copy of the conversation.
const MAX_CACHED_RESPONSE_BYTES: usize = 1024 * 1024;

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
        // A read is safe to run again, and its answer is often the largest
        // thing the host ever produces: the catalogue and the telemetry report
        // are single frames of megabytes, and caching each request's copy
        // filled the journal with hundreds of megabytes nobody reads.
        let read_only = match &frame.kind {
            FrameType::Extension(name) if name == NorteCodeExtension::Command.as_str() => {
                serde_json::from_value::<norte_code_wire::ClientRequest>(frame.payload.clone())
                    .is_ok_and(|request| is_read_only_request(&request.kind))
            }
            _ => false,
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
            && !read_only
        {
            return Err(HostError::InvalidFrame(
                "FORBIDDEN: observer attachments are read-only".to_owned(),
            ));
        }
        {
            let state = self.state.lock().await;
            if let Some(previous) = state.idempotency.get(&idempotency_key) {
                if previous.fingerprint != frame_fingerprint {
                    return Err(HostError::IdempotencyConflict(
                        "frame id was reused with a different request".to_owned(),
                    ));
                }
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
        let response_bytes = serde_json::to_vec(&result).map_or(usize::MAX, |bytes| bytes.len());
        if !read_only && response_bytes <= MAX_CACHED_RESPONSE_BYTES {
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
    #[allow(clippy::too_many_lines)]
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
    pub(crate) async fn delegator_control(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let request: DelegatorControlRequest = serde_json::from_value(frame.payload)
            .map_err(|error| HostError::InvalidFrame(format!("DELEGATOR_CONTROL: {error}")))?;
        let (suspended, queued) = match request.action {
            DelegatorControlAction::Suspend => self
                .service
                .set_delegator_suspended(&request.child_session_id, true)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
            DelegatorControlAction::Release => self
                .service
                .set_delegator_suspended(&request.child_session_id, false)
                .await
                .map_err(|error| HostError::Service(error.to_string()))?,
        };
        Ok(vec![
            self.emit(
                &frame.session_id,
                StandardFrameType::DelegatorStateChanged,
                payload(DelegatorStateChangedPayload {
                    child_session_id: request.child_session_id.as_str(),
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
            self.service_for_session(&session_id)?
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
            ClientRequestKind::RunCommand {
                command, arguments, ..
            } => {
                self.run_plugin_command(session_id, command, arguments)
                    .await?
            }
            ClientRequestKind::InstallPlugin { url, package, all } => {
                // The build runs on a blocking thread, and the page following
                // it watches the session's stream: the steps go there while
                // the work happens, so a long build is visible instead of
                // looking hung.
                let (steps, mut progress) = tokio::sync::mpsc::unbounded_channel();
                let report = move |step: usize, total: usize, stage: &str| {
                    let _ = steps.send((step, total, stage.to_owned()));
                };
                let work = tokio::task::spawn_blocking(move || {
                    if all {
                        crate::plugin_manager::install_all_plugins(&url, &report)
                            .map(|names| names.join(", "))
                    } else {
                        crate::plugin_manager::install_plugin(&url, package.as_deref(), &report)
                    }
                });
                while let Some((step, total, stage)) = progress.recv().await {
                    self.emit_event(
                        session_id,
                        crate::host::plugin_progress_event("install", step, total, &stage),
                    )
                    .await?;
                }
                let result = work
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
            other => {
                return Err(HostError::InvalidFrame(format!(
                    "UNSUPPORTED_REQUEST: {other:?}"
                )));
            }
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

    async fn run_plugin_command(
        &self,
        session_id: &str,
        command: String,
        arguments: Vec<String>,
    ) -> Result<ServerEventKind, HostError> {
        let plugin_host = self
            .plugin_host
            .as_ref()
            .ok_or_else(|| HostError::Service("plugin commands are unavailable".to_owned()))?
            .clone();
        let session = norte_code_wire::SessionKey::new(session_id);
        let service = self.service_for_session(session_id).ok();
        let invocation = service.as_ref().and_then(|service| {
            let spec = service.session_spec(&session)?;
            Some(crate::plugin_host::invocation(
                PluginSelection {
                    session: Some(session),
                    provider: Some(spec.provider),
                    model: Some(spec.model),
                },
                Some(service),
            ))
        });
        let command_for_run = command.clone();
        let output = tokio::task::spawn_blocking(move || {
            plugin_host.execute_command(&command_for_run, &arguments, invocation.as_ref())
        })
        .await
        .map_err(|error| HostError::Service(error.to_string()))?
        .map_err(|error| HostError::Service(error.to_string()))?;
        Ok(ServerEventKind::PluginCommandCompleted {
            command,
            message: output.message,
        })
    }
}
