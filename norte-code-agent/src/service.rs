//! The in-process service: it owns one Norte engine, maps the client protocol
//! onto turns, and streams the engine's events back as [`ServerEvent`]s.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use futures_core::Stream;
use futures_util::StreamExt;
use norte::{
    BlobStore, Engine, EngineBuilder, EngineConfig, EngineEvent, FinishReason, InMemoryBlobStore,
    InMemoryStore, Message, Model, SessionId, Store, StreamDelta, ToolRegistry, Usage,
};
use tokio::sync::{Notify, mpsc};

use norte_code_config::Config;
use norte_code_protocol::{
    Delta, ErrorCode, ServerEvent, ServerEventKind, SessionInfo, SessionKey,
};

use crate::ServiceError;
use crate::provider::{ModelFactory, ModelSpec, capabilities};

/// A stream of events answering one request.
pub type EventStream = Pin<Box<dyn Stream<Item = ServerEvent> + Send>>;

/// A session as the service tracks it, beyond what Norte stores.
#[derive(Debug)]
struct SessionRecord {
    id: SessionId,
    title: Option<String>,
    model: String,
    created_at: DateTime<Utc>,
    phase: norte_code_protocol::Phase,
    /// Messages of this session already reported for usage.
    billed: usize,
    /// Model requests reported for usage.
    requests: u64,
    /// Provider-reported cost, summed.
    cost: f64,
    /// Requests that carried no cost.
    unpriced: u64,
}

/// Live counters for the turn in flight.
#[derive(Debug, Clone, Copy)]
struct Stats {
    turn: u64,
    billed: usize,
    requests: u64,
    cost: f64,
    unpriced: u64,
}

/// The Norte Code service.
pub struct Service {
    engine: Engine,
    config: Config,
    model: String,
    system_prompt: String,
    sessions: Mutex<BTreeMap<SessionKey, SessionRecord>>,
    cancels: Mutex<BTreeMap<SessionKey, Arc<Notify>>>,
}

impl Service {
    /// Builds a service over an already-built model.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the engine cannot be built.
    pub fn new(
        config: Config,
        model: Arc<dyn Model>,
        store: Arc<dyn Store>,
        blobs: Arc<dyn BlobStore>,
        system_prompt: impl Into<String>,
    ) -> Result<Self, ServiceError> {
        let reference = model.reference();
        let model_label = format!(
            "{}/{}",
            reference.provider.as_str(),
            reference.model.as_str()
        );
        let engine = EngineBuilder::new()
            .store(store)
            .blobs(blobs)
            .model(model)
            .tools(ToolRegistry::new())
            .config(EngineConfig {
                max_iterations: config.max_iterations,
                ..EngineConfig::default()
            })
            .build()?;
        Ok(Self {
            engine,
            config,
            model: model_label,
            system_prompt: system_prompt.into(),
            sessions: Mutex::new(BTreeMap::new()),
            cancels: Mutex::new(BTreeMap::new()),
        })
    }

    /// Builds a service over an in-memory store and a vendor model.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the model cannot be built.
    pub fn in_memory(
        config: Config,
        spec: &ModelSpec,
        factory: &dyn ModelFactory,
        system_prompt: impl Into<String>,
    ) -> Result<Self, ServiceError> {
        let key = format!("{}/{}", spec.provider, spec.model);
        let caps = capabilities(config.model_overrides.get(&key));
        let model = factory.build(spec, &caps)?;
        Self::new(
            config,
            model,
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            system_prompt,
        )
    }

    /// The configuration the service was built with.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Answers one client request.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the request cannot be started; a failure
    /// that happens while a turn runs is delivered as a
    /// [`ServerEventKind::Error`] event instead.
    pub async fn submit(
        self: &Arc<Self>,
        request: norte_code_protocol::ClientRequest,
    ) -> Result<EventStream, ServiceError> {
        use norte_code_protocol::ClientRequestKind as Kind;
        let request_id = request.request_id;
        match request.kind {
            Kind::CreateSession { title } => {
                let key = self.create_session(title.clone()).await?;
                Ok(single(
                    request_id,
                    Some(key),
                    ServerEventKind::SessionCreated { title },
                ))
            }
            Kind::ListSessions => Ok(single(
                request_id,
                None,
                ServerEventKind::SessionList {
                    sessions: self.list_sessions(),
                },
            )),
            Kind::SwitchSession { session } => {
                self.require(&session)?;
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::SessionSwitched,
                ))
            }
            Kind::ForkSession { session, title } => {
                let key = self.fork_session(&session, title.clone()).await?;
                Ok(single(
                    request_id,
                    Some(key),
                    ServerEventKind::SessionForked { from: session },
                ))
            }
            Kind::RenameSession { session, title } => {
                self.rename(&session, &title)?;
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::SessionRenamed { title },
                ))
            }
            Kind::DeleteSession { session } => {
                self.delete(&session)?;
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::SessionDeleted,
                ))
            }
            Kind::SendMessage { session, text, .. } => {
                self.start_turn(request_id, session, Some(text)).await
            }
            Kind::Resume { session } => self.start_turn(request_id, Some(session), None).await,
            Kind::CancelTurn { session } => {
                if let Some(notify) = self.lock_cancels().get(&session).cloned() {
                    notify.notify_one();
                }
                Ok(Box::pin(futures_util::stream::empty()))
            }
            Kind::CostReport { session } => Ok(self.cost_report(request_id, session.as_ref())),
            Kind::ListModels | Kind::RefreshCatalog => Err(ServiceError::Unsupported(
                "the model catalogue arrives in milestone 2",
            )),
            Kind::SetModel { .. } | Kind::SetReasoning { .. } | Kind::SetBudget { .. } => Err(
                ServiceError::Unsupported("session settings arrive in a later milestone"),
            ),
            Kind::Approve { .. } | Kind::Deny { .. } | Kind::Revoke { .. } => {
                Err(ServiceError::Unsupported("approvals arrive in milestone 3"))
            }
            Kind::Steer { .. } => Err(ServiceError::Unsupported("steering arrives with the TUI")),
            Kind::Compact { .. } => Err(ServiceError::Unsupported(
                "compaction arrives in milestone 6",
            )),
        }
    }

    async fn create_session(&self, title: Option<String>) -> Result<SessionKey, ServiceError> {
        let session = self.engine.create().await?;
        self.engine
            .append_history(
                session.id,
                vec![Message::system(self.system_prompt.clone())],
            )
            .await?;
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let record = SessionRecord {
            id: session.id,
            title,
            model: self.model.clone(),
            created_at: Utc::now(),
            phase: norte_code_protocol::Phase::Idle,
            billed: 0,
            requests: 0,
            cost: 0.0,
            unpriced: 0,
        };
        self.lock_sessions().insert(key.clone(), record);
        Ok(key)
    }

    async fn fork_session(
        &self,
        from: &SessionKey,
        title: Option<String>,
    ) -> Result<SessionKey, ServiceError> {
        let source = self.require(from)?;
        let conversation = self.engine.export(source).await?;
        let imported = self.engine.import(conversation).await?;
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let record = SessionRecord {
            id: imported.id,
            title,
            model: self.model.clone(),
            created_at: Utc::now(),
            phase: norte_code_protocol::Phase::Idle,
            billed: 0,
            requests: 0,
            cost: 0.0,
            unpriced: 0,
        };
        self.lock_sessions().insert(key.clone(), record);
        Ok(key)
    }

    /// Starts a turn (or a resume) and returns the stream of its events.
    async fn start_turn(
        self: &Arc<Self>,
        request_id: u64,
        session: Option<SessionKey>,
        input: Option<String>,
    ) -> Result<EventStream, ServiceError> {
        let (key, nid) = if let Some(key) = session {
            (key.clone(), self.require(&key)?)
        } else {
            let key = self.create_session(None).await?;
            let nid = self.require(&key)?;
            (key, nid)
        };
        let state = self.engine.session(nid).await?;
        let turn = state.turn + 1;
        let (tx, rx) = mpsc::channel::<ServerEvent>(256);
        let notify = Arc::new(Notify::new());
        self.lock_cancels().insert(key.clone(), notify.clone());

        let this = Arc::clone(self);
        let task_key = key.clone();
        tokio::spawn(async move {
            let result = this
                .run_turn(request_id, &task_key, nid, turn, input, &tx, &notify)
                .await;
            if let Err(error) = result {
                let _ = tx
                    .send(ServerEvent::error(
                        request_id,
                        Some(task_key.clone()),
                        ErrorCode::Internal,
                        error.to_string(),
                    ))
                    .await;
            }
            this.lock_cancels().remove(&task_key);
        });

        Ok(Box::pin(futures_util::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|event| (event, rx)) },
        )))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_turn(
        &self,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        turn: u64,
        input: Option<String>,
        tx: &mpsc::Sender<ServerEvent>,
        notify: &Notify,
    ) -> Result<(), ServiceError> {
        let mut stream = input.map_or_else(
            || self.engine.resume(nid),
            |text| self.engine.turn(nid).input(Message::user(text)).start(),
        );
        let mut stats = self.stats(key, turn);
        if !emit(
            tx,
            ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::TurnStarted { turn },
            ),
        )
        .await
        {
            return Ok(());
        }

        let mut cancelled = false;
        let mut failure: Option<String> = None;
        loop {
            tokio::select! {
                biased;
                () = notify.notified() => {
                    cancelled = true;
                }
                next = stream.next() => {
                    match next {
                        Some(Ok(event)) => {
                            if !self.handle(request_id, key, nid, &event, &mut stats, tx).await {
                                return Ok(());
                            }
                        }
                        Some(Err(error)) => {
                            failure = Some(error.to_string());
                            break;
                        }
                        None => break,
                    }
                }
            }
            if cancelled {
                stream.cancel();
                cancelled = false;
            }
        }

        let outcome = stream.finish().await;
        // The last request has no successor to settle it.
        let _ = self.settle(request_id, key, nid, &mut stats, tx).await;
        self.store_stats(key, &stats);

        match outcome {
            Ok(outcome) => {
                let phase = to_phase(outcome.session.phase);
                self.set_phase(key, phase);
                emit(
                    tx,
                    ServerEvent::new(
                        request_id,
                        Some(key.clone()),
                        ServerEventKind::TurnFinished {
                            turn,
                            phase,
                            committed: outcome.message.is_some(),
                        },
                    ),
                )
                .await;
            }
            Err(error) => {
                let message = failure.unwrap_or_else(|| error.to_string());
                let phase = self
                    .engine
                    .session(nid)
                    .await
                    .map_or(norte_code_protocol::Phase::Idle, |s| to_phase(s.phase));
                self.set_phase(key, phase);
                emit(
                    tx,
                    ServerEvent::error(request_id, Some(key.clone()), ErrorCode::Provider, message),
                )
                .await;
                emit(
                    tx,
                    ServerEvent::new(
                        request_id,
                        Some(key.clone()),
                        ServerEventKind::TurnFinished {
                            turn,
                            phase,
                            committed: false,
                        },
                    ),
                )
                .await;
            }
        }
        Ok(())
    }

    /// Maps one engine event and answers whether to keep going (the client is
    /// still reading).
    async fn handle(
        &self,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        event: &EngineEvent,
        stats: &mut Stats,
        tx: &mpsc::Sender<ServerEvent>,
    ) -> bool {
        let kind = match event {
            EngineEvent::StreamDelta { delta, .. } => {
                to_delta(delta).map(|delta| ServerEventKind::Delta {
                    turn: stats.turn,
                    delta,
                })
            }
            EngineEvent::ToolCallStarted { call_id, tool, .. } => {
                Some(ServerEventKind::ToolStarted {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                })
            }
            EngineEvent::ToolCallFinished {
                call_id,
                tool,
                is_error,
                duration_ms,
                ..
            } => Some(ServerEventKind::ToolFinished {
                turn: stats.turn,
                call_id: call_id.to_string(),
                tool: tool.to_string(),
                is_error: *is_error,
                duration_ms: *duration_ms,
            }),
            EngineEvent::ApprovalRequested {
                approval,
                call_id,
                tool,
                ..
            } => Some(ServerEventKind::ApprovalRequested {
                turn: stats.turn,
                approval: approval.to_string(),
                call_id: call_id.to_string(),
                tool: tool.to_string(),
            }),
            EngineEvent::ApprovalGranted { approval, .. } => {
                Some(ServerEventKind::ApprovalResolved {
                    approval: approval.to_string(),
                    approved: true,
                })
            }
            EngineEvent::CommitSucceeded { version, .. } => Some(ServerEventKind::Committed {
                turn: stats.turn,
                version: *version,
            }),
            EngineEvent::Error { message, .. } => Some(ServerEventKind::Error {
                code: ErrorCode::Provider,
                message: message.clone(),
            }),
            EngineEvent::RequestReconstructed { .. } => {
                return self.settle(request_id, key, nid, stats, tx).await;
            }
            _ => None,
        };
        if let Some(kind) = kind {
            return emit(tx, ServerEvent::new(request_id, Some(key.clone()), kind)).await;
        }
        true
    }

    /// Reports every model request committed since the last look.
    async fn settle(
        &self,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        stats: &mut Stats,
        tx: &mpsc::Sender<ServerEvent>,
    ) -> bool {
        let Ok(history) = self.engine.history(nid).await else {
            return true;
        };
        for message in history.iter().skip(stats.billed) {
            let Some(origin) = &message.origin else {
                continue;
            };
            stats.requests += 1;
            let cost = reported_cost(&origin.usage);
            if let Some(cost) = cost {
                stats.cost += cost;
            } else {
                stats.unpriced += 1;
            }
            let event = ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::RequestReconstructed {
                    turn: stats.turn,
                    index: stats.requests,
                    usage: to_usage(&origin.usage),
                    cost_usd: cost,
                    truncated: matches!(origin.finish_reason, FinishReason::Length),
                },
            );
            if !emit(tx, event).await {
                stats.billed = history.len();
                return false;
            }
        }
        stats.billed = history.len();
        true
    }

    fn cost_report(&self, request_id: u64, session: Option<&SessionKey>) -> EventStream {
        let lock = self.lock_sessions();
        let (cost, requests, unpriced) = match session {
            Some(key) => lock.get(key).map_or((0.0, 0, 0), |record| {
                (record.cost, record.requests, record.unpriced)
            }),
            None => lock
                .values()
                .fold((0.0, 0, 0), |(cost, requests, unpriced), record| {
                    (
                        cost + record.cost,
                        requests + record.requests,
                        unpriced + record.unpriced,
                    )
                }),
        };
        drop(lock);
        let mut events = vec![ServerEvent::new(
            request_id,
            session.cloned(),
            ServerEventKind::CostUpdated {
                total_usd: cost,
                requests,
            },
        )];
        if unpriced > 0 {
            events.push(ServerEvent::new(
                request_id,
                session.cloned(),
                ServerEventKind::CostNotReported { requests: unpriced },
            ));
        }
        Box::pin(futures_util::stream::iter(events))
    }

    fn list_sessions(&self) -> Vec<SessionInfo> {
        self.lock_sessions()
            .iter()
            .map(|(key, record)| SessionInfo {
                session: key.clone(),
                title: record.title.clone(),
                model: (!record.model.is_empty()).then(|| record.model.clone()),
                created_at: Some(record.created_at.to_rfc3339()),
                phase: record.phase,
            })
            .collect()
    }

    fn stats(&self, key: &SessionKey, turn: u64) -> Stats {
        self.lock_sessions().get(key).map_or(
            Stats {
                turn,
                billed: 0,
                requests: 0,
                cost: 0.0,
                unpriced: 0,
            },
            |record| Stats {
                turn,
                billed: record.billed,
                requests: record.requests,
                cost: record.cost,
                unpriced: record.unpriced,
            },
        )
    }

    fn store_stats(&self, key: &SessionKey, stats: &Stats) {
        if let Some(record) = self.lock_sessions().get_mut(key) {
            record.billed = stats.billed;
            record.requests = stats.requests;
            record.cost = stats.cost;
            record.unpriced = stats.unpriced;
        }
    }

    fn set_phase(&self, key: &SessionKey, phase: norte_code_protocol::Phase) {
        if let Some(record) = self.lock_sessions().get_mut(key) {
            record.phase = phase;
        }
    }

    fn rename(&self, key: &SessionKey, title: &str) -> Result<(), ServiceError> {
        let mut lock = self.lock_sessions();
        let record = lock
            .get_mut(key)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        record.title = Some(title.to_owned());
        Ok(())
    }

    fn delete(&self, key: &SessionKey) -> Result<(), ServiceError> {
        self.lock_sessions()
            .remove(key)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        Ok(())
    }

    fn require(&self, key: &SessionKey) -> Result<SessionId, ServiceError> {
        self.lock_sessions()
            .get(key)
            .map(|record| record.id)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))
    }

    fn lock_sessions(&self) -> MutexGuard<'_, BTreeMap<SessionKey, SessionRecord>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_cancels(&self) -> MutexGuard<'_, BTreeMap<SessionKey, Arc<Notify>>> {
        self.cancels.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A stream carrying one event.
fn single(request_id: u64, session: Option<SessionKey>, kind: ServerEventKind) -> EventStream {
    Box::pin(futures_util::stream::once(async move {
        ServerEvent::new(request_id, session, kind)
    }))
}

/// Sends an event and reports whether the client is still reading.
async fn emit(tx: &mpsc::Sender<ServerEvent>, event: ServerEvent) -> bool {
    tx.send(event).await.is_ok()
}

/// The provider-reported cost, in USD, when the payload carried one.
fn reported_cost(usage: &Usage) -> Option<f64> {
    usage.vendor.get("cost").and_then(serde_json::Value::as_f64)
}

fn to_usage(usage: &Usage) -> norte_code_protocol::Usage {
    norte_code_protocol::Usage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        cost_usd: reported_cost(usage),
        vendor: usage.vendor.clone(),
    }
}

fn to_phase(phase: norte::Phase) -> norte_code_protocol::Phase {
    match phase {
        norte::Phase::Streaming => norte_code_protocol::Phase::Streaming,
        norte::Phase::AwaitingTool => norte_code_protocol::Phase::AwaitingTool,
        norte::Phase::AwaitingApproval => norte_code_protocol::Phase::AwaitingApproval,
        _ => norte_code_protocol::Phase::Idle,
    }
}

fn to_delta(delta: &StreamDelta) -> Option<Delta> {
    Some(match delta {
        StreamDelta::Text { index, text } => Delta::Text {
            index: *index,
            text: text.clone(),
        },
        StreamDelta::Reasoning { index, text } => Delta::Reasoning {
            index: *index,
            text: text.clone(),
        },
        StreamDelta::Refusal { index, text } => Delta::Refusal {
            index: *index,
            text: text.clone(),
        },
        StreamDelta::ToolArguments {
            index,
            call_id,
            arguments,
        } => Delta::ToolArguments {
            index: *index,
            call_id: call_id.to_string(),
            arguments: arguments.clone(),
        },
        StreamDelta::Media { index, bytes, last } => Delta::Media {
            index: *index,
            bytes: u64::try_from(*bytes).unwrap_or(u64::MAX),
            last: *last,
        },
        // Continuity, safety ratings and logprobs are not answer text; the TUI
        // has nothing to render for them.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use norte::{InMemoryBlobStore, InMemoryStore, ModelId, ModelRef, ProviderId};
    use norte_code_protocol::{ClientRequest, ClientRequestKind};
    use norte_testing::ScriptedModel;

    fn scripted() -> Arc<ScriptedModel> {
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        Arc::new(ScriptedModel::new(reference).turn_deltas(["Hel", "lo"]))
    }

    fn service(model: Arc<ScriptedModel>) -> Arc<Service> {
        Arc::new(
            Service::new(
                Config::default(),
                model,
                Arc::new(InMemoryStore::new()),
                Arc::new(InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        )
    }

    async fn drain(
        service: &Arc<Service>,
        kind: ClientRequestKind,
    ) -> Vec<norte_code_protocol::ServerEvent> {
        service
            .submit(ClientRequest {
                request_id: 7,
                kind,
            })
            .await
            .expect("request")
            .collect()
            .await
    }

    #[tokio::test]
    async fn a_turn_streams_text_and_commits() {
        let service = service(scripted());
        let events = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "hi".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;

        let mut text = String::new();
        let mut requests = 0;
        let mut committed = false;
        for event in &events {
            assert_eq!(event.request_id, 7);
            assert!(event.session.is_some());
            match &event.kind {
                ServerEventKind::Delta {
                    delta: Delta::Text { text: chunk, .. },
                    ..
                } => text.push_str(chunk),
                ServerEventKind::RequestReconstructed { .. } => requests += 1,
                ServerEventKind::TurnFinished {
                    committed: done, ..
                } => committed = *done,
                _ => {}
            }
        }
        assert_eq!(text, "Hello");
        assert_eq!(requests, 1);
        assert!(committed);
        assert!(matches!(
            events.first().map(|event| &event.kind),
            Some(ServerEventKind::TurnStarted { .. })
        ));
    }

    #[tokio::test]
    async fn sessions_are_created_listed_and_renamed() {
        let service = service(scripted());
        let created = drain(
            &service,
            ClientRequestKind::CreateSession {
                title: Some("first".to_owned()),
            },
        )
        .await;
        let key = created[0].session.clone().expect("session");
        assert!(matches!(
            created[0].kind,
            ServerEventKind::SessionCreated { .. }
        ));

        let listed = drain(&service, ClientRequestKind::ListSessions).await;
        let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
            panic!("expected a list");
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("first"));

        let renamed = drain(
            &service,
            ClientRequestKind::RenameSession {
                session: key,
                title: "second".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            renamed[0].kind,
            ServerEventKind::SessionRenamed { .. }
        ));
    }

    #[tokio::test]
    async fn an_unknown_session_is_an_error() {
        let service = service(scripted());
        let error = service
            .submit(ClientRequest {
                request_id: 1,
                kind: ClientRequestKind::SendMessage {
                    session: Some(SessionKey::new("missing")),
                    text: "hi".to_owned(),
                    budget: None,
                    reasoning: None,
                },
            })
            .await;
        assert!(error.is_err());
    }
}
