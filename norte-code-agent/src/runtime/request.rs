//! The request: from the session's history to what the model receives.
//!
//! The history is reconstructed into the wire-ready message sequence, the
//! layers get to change it, and the stream that comes back is accumulated into
//! the assistant message the turn commits.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::runtime::Session;
use crate::runtime::event::{EngineEvent, StreamDelta};
use norte::domain::continuity::{ContinuityToken, ReplayDecision};
use norte::domain::media::{MediaPart, MediaSource};
use norte::domain::message::{FinishReason, Message, MessageOrigin, MessageStatus};
use norte::domain::part::{Part, ToolCallPart, ToolResultPart};
use norte::domain::role::Role;
use norte::error::{Error, Result};
use norte::id::{CallId, Digest, GenerationId, ModelRef, SessionId, ToolName};
use norte::layer::{Continuation, LayerContext};
use norte::model::{ModelRequest, ModelStream, SESSION_EXTENSION, TurnOptions};
use norte::preprocess::MediaPayload;
use norte::stream::{Accumulated, StreamAccumulator, StreamEvent};

use super::{
    CallSite, Digesting, Harness, PreparedCall, Reconstructed, cache_boundaries, digest_of,
    execute_call, materialize_message, stream_delta,
};

impl Harness {
    /// Runs the model and returns the answer, whether it was interrupted, and
    /// any tool results produced early.
    ///
    /// # Errors
    ///
    /// Returns a capability, codec or provider error.
    #[allow(clippy::too_many_lines)]
    pub async fn generate(
        &self,
        session: &Session,
        history: Vec<Message>,
        options: &TurnOptions,
        sender: &mpsc::Sender<EngineEvent>,
        cancellation: &CancellationToken,
    ) -> Result<(Message, bool, Vec<ToolResultPart>)> {
        let model_ref = self.model.reference().clone();
        let capabilities = self.model.capabilities();
        let (history, invalidated) = self.preprocess_history(history).await?;
        if cancellation.is_cancelled() {
            let message = self
                .finalize_message(
                    Accumulated::default(),
                    true,
                    model_ref,
                    Digest::of_json(&serde_json::json!(null)),
                )
                .await?;
            return Ok((message, true, Vec::new()));
        }
        if invalidated {
            self.emit(
                sender,
                EngineEvent::PrefixInvalidated {
                    session: session.id,
                    reason: "media preprocessing changed the cached prefix".to_owned(),
                },
            )
            .await;
        }
        let reconstructed = self.reconstruct(&history).await?;

        if cancellation.is_cancelled() {
            let message = self
                .finalize_message(
                    Accumulated::default(),
                    true,
                    model_ref,
                    Digest::of_json(&serde_json::json!(null)),
                )
                .await?;
            return Ok((message, true, Vec::new()));
        }

        let sent_parts: Vec<Part> = reconstructed
            .messages
            .iter()
            .flat_map(|message| message.parts.iter().cloned())
            .collect();
        capabilities.check_parts(&model_ref, &sent_parts)?;
        let tools = self.effective_tools(options)?;
        capabilities.check_tools(&model_ref, &tools)?;
        let reasoning = options.reasoning.unwrap_or(self.config.reasoning);
        capabilities.check_reasoning(&model_ref, reasoning)?;
        capabilities.check_generation(&model_ref, &options.params)?;
        let tool_names: Vec<ToolName> = tools.iter().map(|spec| spec.name.clone()).collect();

        let cache_boundaries = reconstructed.cache_boundaries.clone();
        let mut extensions = options.extensions.clone();
        extensions.insert(
            SESSION_EXTENSION.to_owned(),
            Value::String(session.id.to_string()),
        );
        let request = ModelRequest {
            model: model_ref.clone(),
            messages: reconstructed.messages,
            tools,
            reasoning,
            params: options.params.clone(),
            limits: self.config.limits,
            cache_boundaries: reconstructed.cache_boundaries,
            extensions,
        };
        // A layer may alter the request, so the digest must describe the one the
        // model receives (FR-ENG-4). The clone is the fallback for a layer that
        // answers without the model: the request it received is what the digest
        // describes then.
        let reconstructed = request.clone();
        let wire_digest = Mutex::new(None);
        let context = LayerContext {
            session: session.id,
            turn: session.turn,
        };
        let continuation = Digesting {
            model: &*self.model,
            digest: &wire_digest,
        };
        let stream = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                // The person stopped the turn before the provider was asked.
                // Commit the same interrupted message the stream path commits
                // when it is cancelled before its first part; there is no
                // request to report and no stream to drain. Dropping the other
                // future closes the HTTP connection.
                let message = self
                    .finalize_message(
                        Accumulated::default(),
                        true,
                        model_ref,
                        digest_of(&reconstructed),
                    )
                    .await?;
                return Ok((message, true, Vec::new()));
            }
            stream = self.layered_call(&context, request, &continuation, session.id, sender) => {
                stream?
            }
        };
        let request_digest = wire_digest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_else(|| digest_of(&reconstructed));
        // A cancel can land in the window between the provider answering and
        // drain_stream starting, where nothing was watching the token: catch it
        // here, drop the stream (closing the connection) and commit the
        // interrupted, empty assistant message instead of reporting a request.
        if cancellation.is_cancelled() {
            let message = self
                .finalize_message(Accumulated::default(), true, model_ref, request_digest)
                .await?;
            return Ok((message, true, Vec::new()));
        }
        let emit_future = self.emit(
            sender,
            EngineEvent::RequestReconstructed {
                session: session.id,
                turn: session.turn,
                digest: request_digest,
                cache_boundaries,
                params: options.params.clone(),
                reasoning,
                tools: tool_names,
            },
        );
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                let message = self
                    .finalize_message(
                        Accumulated::default(),
                        true,
                        model_ref,
                        request_digest,
                    )
                    .await?;
                return Ok((message, true, Vec::new()));
            }
            _ = emit_future => {}
        }
        let (accumulated, interrupted, early_results) = self
            .drain_stream(session, sender, cancellation, stream)
            .await?;

        let message = self
            .finalize_message(accumulated, interrupted, model_ref, request_digest)
            .await?;
        Ok((message, interrupted, early_results))
    }

    /// Assembles the committed assistant message from what the stream
    /// accumulated: its final parts, its finish reason and the provenance a
    /// replay reads back (FR-ENG-9).
    async fn finalize_message(
        &self,
        accumulated: Accumulated,
        interrupted: bool,
        model: ModelRef,
        request_digest: Digest,
    ) -> Result<Message> {
        let parts = self.finalize_output(accumulated.parts).await?;
        let finish_reason = accumulated.finish_reason.unwrap_or(if interrupted {
            FinishReason::Cancelled
        } else {
            FinishReason::Stop
        });
        let status = if interrupted {
            MessageStatus::Interrupted
        } else if matches!(finish_reason, FinishReason::Length) {
            MessageStatus::Truncated
        } else {
            MessageStatus::Complete
        };
        Ok(Message::new(Role::Assistant, parts).with_origin(
            MessageOrigin::new(
                model,
                GenerationId::new(),
                finish_reason,
                accumulated.usage,
                status,
            )
            .with_digest(request_digest),
        ))
    }

    async fn drain_stream(
        &self,
        session: &Session,
        sender: &mpsc::Sender<EngineEvent>,
        cancellation: &CancellationToken,
        mut stream: ModelStream,
    ) -> Result<(Accumulated, bool, Vec<ToolResultPart>)> {
        let mut accumulator = StreamAccumulator::new(self.config.limits);
        let mut continuity: BTreeMap<u32, ContinuityToken> = BTreeMap::new();
        let mut interrupted = false;
        let mut completed: Vec<Part> = Vec::new();
        let mut early: Vec<(CallId, JoinHandle<ToolResultPart>)> = Vec::new();

        loop {
            let event = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    interrupted = true;
                    break;
                }
                next = stream.next() => match next {
                    Some(Ok(event)) => event,
                    Some(Err(error)) => {
                        self.emit(sender, EngineEvent::Error {
                            session: Some(session.id),
                            message: error.to_string(),
                        }).await;
                        return Err(error);
                    }
                    None => break,
                }
            };

            if let StreamEvent::Continuity { index, fragment } = &event {
                let merged = self
                    .model
                    .continuity()
                    .merge(continuity.get(index).cloned(), fragment.clone())?;
                continuity.insert(*index, merged.clone());
                accumulator.attach_continuity(*index, merged);
                let delta = StreamDelta::Continuity {
                    index: *index,
                    len: fragment.len(),
                };
                if !self.emit_delta(sender, session, delta).await {
                    interrupted = true;
                    break;
                }
                continue;
            }

            if let Some(part) = accumulator.apply(event.clone())? {
                let delivered = self
                    .emit(
                        sender,
                        EngineEvent::PartCompleted {
                            session: session.id,
                            turn: session.turn,
                            part: part.clone(),
                        },
                    )
                    .await;
                if !delivered {
                    interrupted = true;
                    break;
                }
                let ready = match &part {
                    Part::ToolCall(call) => Some(call.clone()),
                    _ => None,
                };
                completed.push(part);
                if let Some(call) = ready
                    && let Some(handle) = self.dispatch_early(session, sender, &call, cancellation)
                {
                    early.push((call.call_id, handle));
                }
            }
            if let Some(delta) = stream_delta(&event)
                && !self.emit_delta(sender, session, delta).await
            {
                interrupted = true;
                break;
            }
        }

        let mut results = Vec::with_capacity(early.len());
        let interrupted = interrupted || cancellation.is_cancelled();
        for (call_id, handle) in early {
            match handle.await {
                Ok(result) => results.push(result),
                Err(error) => results.push(ToolResultPart::error(
                    call_id,
                    format!("early tool task failed: {error}"),
                )),
            }
        }
        Ok((accumulator.finish()?, interrupted, results))
    }

    /// Dispatches a tool call whose arguments just completed for execution before
    /// the assistant message finishes, when the host enabled early tool execution
    /// and the tool policy allows it (FR-STREAM-4).
    ///
    /// The policy is evaluated against the assistant message as known so far,
    /// with no results of its own — nothing has run yet — and the history
    /// committed up to the previous tool turn. Calls that need approval, are
    /// denied or are rate limited return `None` and take the normal path.
    fn dispatch_early(
        &self,
        session: &Session,
        sender: &mpsc::Sender<EngineEvent>,
        call: &ToolCallPart,
        cancellation: &CancellationToken,
    ) -> Option<JoinHandle<ToolResultPart>> {
        if !(self.config.early_tool_execution) {
            return None;
        }
        let Ok(tool_name) = ToolName::new(call.name.clone()) else {
            return None;
        };
        self.tools.spec(&tool_name)?;

        let metadata = self.call_metadata(session, call, &tool_name, &call.arguments);
        Some(tokio::spawn(execute_call(
            Arc::clone(&self.tools),
            Arc::clone(&self.observers),
            sender.clone(),
            CallSite {
                session: session.id,
                turn: session.turn,
            },
            PreparedCall {
                call: call.clone(),
                tool: tool_name,
                arguments: call.arguments.clone(),
                metadata,
            },
            cancellation.clone(),
        )))
    }

    /// The reconstructed request: the message sequence plus deterministic cache
    /// boundary positions (FR-RECON-7).
    async fn reconstruct(&self, history: &[Message]) -> Result<Reconstructed> {
        let target = self.model.reference().clone();
        let mut critical = Vec::with_capacity(history.len());
        let mut ephemeral = Vec::new();
        for message in history {
            let mut rebuilt = Vec::with_capacity(message.parts.len());
            for part in &message.parts {
                rebuilt.push(self.replay_part(&target, history, part.clone())?);
            }
            let mut message = message.clone();
            message.parts = rebuilt;
            if let Some(adapter) = &self.media_adapter {
                message = materialize_message(adapter, &*self.blobs, message).await?;
            }
            if message.cache.is_ephemeral() {
                ephemeral.push(message);
            } else {
                critical.push(message);
            }
        }
        let cache_boundaries = cache_boundaries(&critical);
        critical.extend(ephemeral);
        Ok(Reconstructed {
            messages: critical,
            cache_boundaries,
        })
    }

    fn replay_part(&self, target: &ModelRef, history: &[Message], part: Part) -> Result<Part> {
        let Some(token) = part.continuity().cloned() else {
            return Ok(part);
        };
        match self.model.continuity().replay(target, history, &token) {
            ReplayDecision::Replay => Ok(part),
            ReplayDecision::Downgrade(replacement) => Ok(*replacement),
            ReplayDecision::Block(reason) => Err(Error::ContinuityBlocked(reason)),
            // `Drop`, and any decision a future core adds that this build cannot
            // read, mean the same thing here: leave the token out. Dropping is
            // the safe reading of an unknown continuity decision.
            _ => Ok(part.without_continuity()),
        }
    }

    /// Runs the layer stack, recording a layer's refusal before propagating it.
    ///
    /// A layer wraps one model call and names its reason in the error it
    /// returns; it has no sink and should not need one. Putting the refusal in
    /// the audit trail is the loop's business.
    async fn layered_call(
        &self,
        context: &LayerContext,
        request: ModelRequest,
        continuation: &dyn Continuation,
        session: SessionId,
        sender: &mpsc::Sender<EngineEvent>,
    ) -> Result<ModelStream> {
        match self.layers.run(context, request, continuation).await {
            Ok(stream) => Ok(stream),
            Err(Error::ShortCircuited { layer, reason }) => {
                let emitted = self
                    .emit(
                        sender,
                        EngineEvent::LayerShortCircuited {
                            session,
                            layer: layer.clone(),
                            reason: reason.clone(),
                        },
                    )
                    .await;
                let _ = emitted;
                Err(Error::ShortCircuited { layer, reason })
            }
            Err(error) => Err(error),
        }
    }

    /// Applies output ingestion (URLs and vendor files become blobs) and then
    /// externalizes over-threshold inline bytes.
    async fn finalize_output(&self, parts: Vec<Part>) -> Result<Vec<Part>> {
        let mut parts = parts;
        self.preprocess_parts(&mut parts).await?;
        if let Some(adapter) = &self.media_adapter {
            let mut ingested = Vec::with_capacity(parts.len());
            for part in parts {
                match part {
                    Part::Media(media) => {
                        let media = adapter.ingest(media, &*self.blobs).await?;
                        ingested.push(Part::Media(media));
                    }
                    other => ingested.push(other),
                }
            }
            parts = ingested;
        }
        self.externalize(parts).await
    }

    async fn externalize(&self, parts: Vec<Part>) -> Result<Vec<Part>> {
        let mut externalized = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                Part::Media(media) => {
                    if let MediaSource::Inline(bytes) = &media.source
                        && bytes.len() > self.config.inline_threshold
                    {
                        let id = self.blobs.put(bytes).await?;
                        externalized.push(Part::Media(MediaPart {
                            source: MediaSource::Blob(id),
                            ..media
                        }));
                        continue;
                    }
                    externalized.push(Part::Media(media));
                }
                other => externalized.push(other),
            }
        }
        Ok(externalized)
    }

    async fn preprocess_history(&self, history: Vec<Message>) -> Result<(Vec<Message>, bool)> {
        if self.media.is_none() {
            return Ok((history, false));
        }
        let mut processed = Vec::with_capacity(history.len());
        let mut changed = false;
        for mut message in history {
            changed |= self.preprocess_parts(&mut message.parts).await?;
            processed.push(message);
        }
        Ok((processed, changed))
    }

    /// Applies the optional, idempotent preprocessing step to the inline media in
    /// `parts`, returning whether any bytes or media type changed: a change after
    /// the prefix was already sent is a cache invalidation the engine reports
    /// (FR-EVT-4). Preprocessing runs on input history and on generated output
    /// alike (FR-PART-3).
    async fn preprocess_parts(&self, parts: &mut [Part]) -> Result<bool> {
        let Some(preprocessor) = &self.media else {
            return Ok(false);
        };
        let mut changed = false;
        for part in parts {
            let Part::Media(media) = part else {
                continue;
            };
            let MediaSource::Inline(bytes) = &media.source else {
                continue;
            };
            let payload = MediaPayload::new(media.kind, media.mime.clone(), bytes.clone());
            let transformed = preprocessor.preprocess(payload).await?;
            if transformed.bytes != *bytes || transformed.mime != media.mime {
                changed = true;
            }
            media.mime = transformed.mime;
            media.source = MediaSource::Inline(transformed.bytes);
        }
        Ok(changed)
    }
}
