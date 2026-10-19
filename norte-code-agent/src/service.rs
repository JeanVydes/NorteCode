//! The in-process service: it owns the Norte engines, maps the client protocol
//! onto turns, and streams the engine's events back as [`ServerEvent`]s.
//!
//! An engine is cheap and shares the store and blob store, so each turn builds
//! one for the model the session selected. That is what makes a mid-session
//! model switch a change of model rather than a change of session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_core::Stream;
use futures_util::StreamExt;
use norte::{
    ApprovalId, BlobStore, Decision, Engine, EngineBuilder, EngineConfig, EngineEvent,
    FinishReason, GrantId, GrantScope, InMemoryBlobStore, InMemoryStore, Message, Model, Policy,
    PolicySet, PolicyView, ReasoningEffort, SessionId, Store, StreamDelta, ToolKind, ToolRegistry,
    Usage,
};
use norte_code_skills::{
    SkillRegistry, SkillResourceTool, SkillScriptConfig, SkillScriptTool, SkillTool,
};
use norte_code_tools::{GlobTool, GrepTool, ListDirTool, TodoState, TodoTool, WebFetchTool};
use norte_tools::{CommandTool, EditTool, ReadTool, WriteTool};
use tokio::sync::{Notify, mpsc};

use norte_code_catalog::{Catalog, LoadOptions, load_catalog};
use norte_code_config::{ApprovalMode, Config};
use norte_code_protocol::{
    Delta, ErrorCode, ModelInfo, ReasoningInfo, ServerEvent, ServerEventKind, SessionInfo,
    SessionKey,
};

use crate::ServiceError;
use crate::provider::{ModelFactory, ModelSpec, Wire};

/// A stream of events answering one request.
pub type EventStream = Pin<Box<dyn Stream<Item = ServerEvent> + Send>>;

/// A session as the service tracks it, beyond what Norte stores.
#[derive(Debug)]
struct SessionRecord {
    id: SessionId,
    title: Option<String>,
    spec: ModelSpec,
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

/// Where the catalogue is refreshed from.
#[derive(Clone, Debug)]
struct CatalogSettings {
    url: String,
    ttl: Duration,
    offline: bool,
    cache_dir: PathBuf,
}

/// The Norte Code service.
pub struct Service {
    config: Config,
    catalog: RwLock<Arc<Catalog>>,
    catalog_settings: CatalogSettings,
    default_spec: ModelSpec,
    store: Arc<dyn Store>,
    blobs: Arc<dyn BlobStore>,
    factory: Arc<dyn ModelFactory>,
    system_prompt: String,
    workspace_root: PathBuf,
    skills: RwLock<Arc<SkillRegistry>>,
    todo: Arc<TodoState>,
    secrets: Vec<String>,
    sessions: Mutex<BTreeMap<SessionKey, SessionRecord>>,
    cancels: Mutex<BTreeMap<SessionKey, Arc<Notify>>>,
}

impl Service {
    /// Builds a service.
    ///
    /// The default spec is used for new sessions; every turn builds an engine
    /// for the session's current spec over the shared store and blob store.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the default model cannot be built.
    pub fn new(
        config: Config,
        catalog: Arc<Catalog>,
        default_spec: ModelSpec,
        factory: Arc<dyn ModelFactory>,
        store: Arc<dyn Store>,
        blobs: Arc<dyn BlobStore>,
        system_prompt: impl Into<String>,
    ) -> Result<Self, ServiceError> {
        let settings = CatalogSettings {
            url: config.catalog.url.clone(),
            ttl: Duration::from_secs(config.catalog.ttl_hours.saturating_mul(3600)),
            offline: config.catalog.offline,
            cache_dir: norte_code_catalog::cache_dir()
                .unwrap_or_else(|| PathBuf::from(".norte-code").join("cache")),
        };
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let secrets = collect_secrets(&config, &catalog);
        let skills = Arc::new(discover_skills(&config, &workspace_root));
        let service = Self {
            config,
            catalog: RwLock::new(catalog),
            catalog_settings: settings,
            default_spec,
            store,
            blobs,
            factory,
            system_prompt: system_prompt.into(),
            workspace_root,
            skills: RwLock::new(skills),
            todo: Arc::new(TodoState::default()),
            secrets,
            sessions: Mutex::new(BTreeMap::new()),
            cancels: Mutex::new(BTreeMap::new()),
        };
        // Fail at construction, not at the first message, when the model's
        // provider cannot be built.
        let _ = service.engine(&service.default_spec)?;
        Ok(service)
    }

    /// Builds a service over an in-memory store.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the model cannot be built.
    pub fn in_memory(
        config: Config,
        catalog: Arc<Catalog>,
        spec: &ModelSpec,
        factory: Arc<dyn ModelFactory>,
        system_prompt: impl Into<String>,
    ) -> Result<Self, ServiceError> {
        Self::new(
            config,
            catalog,
            spec.clone(),
            factory,
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            system_prompt,
        )
    }

    /// Builds an engine for one spec over the shared stores.
    fn engine(&self, spec: &ModelSpec) -> Result<Engine, ServiceError> {
        let model: Arc<dyn Model> = self.factory.build(spec, &spec.capabilities)?;
        Ok(EngineBuilder::new()
            .store(Arc::clone(&self.store))
            .blobs(Arc::clone(&self.blobs))
            .model(model)
            .tools(self.tools()?)
            .policies(self.policies())
            .config(EngineConfig {
                max_iterations: self.config.max_iterations,
                ..EngineConfig::default()
            })
            .build()?)
    }

    /// The tool registry every engine shares, rebuilt cheaply per turn because
    /// Norte's registry is immutable and not cloneable.
    fn tools(&self) -> Result<ToolRegistry, ServiceError> {
        let mut tools = ToolRegistry::new();
        tools.register_tool_declaration(ReadTool::new().into_declaration())?;
        tools.register_tool_declaration(WriteTool::new().into_declaration())?;
        tools.register_tool_declaration(EditTool::new().into_declaration())?;

        let mut env = BTreeMap::new();
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_owned(), path);
        }
        let command = CommandTool::new()
            .with_env(env)
            .with_secrets(self.secrets.clone())
            .with_blobs(Arc::clone(&self.blobs))
            .into_declaration();
        tools.register_tool_declaration(command)?;
        tools.register_tool_declaration(GlobTool::new(&self.workspace_root).into_declaration())?;
        tools
            .register_tool_declaration(ListDirTool::new(&self.workspace_root).into_declaration())?;
        tools.register_tool_declaration(GrepTool::new(&self.workspace_root).into_declaration())?;
        tools
            .register_tool_declaration(TodoTool::new(Arc::clone(&self.todo)).into_declaration())?;
        if self.config.tools.web_fetch {
            tools.register_tool_declaration(WebFetchTool::new().into_declaration())?;
        }

        if self.config.skills.enabled {
            let skills = self.skills();
            tools.register_tool_declaration(
                SkillTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            tools.register_tool_declaration(
                SkillResourceTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            if self.config.skills.allow_scripts {
                tools.register_tool_declaration(
                    SkillScriptTool::new(
                        Arc::clone(&skills),
                        SkillScriptConfig::in_workspace(&self.workspace_root),
                    )
                    .into_declaration(),
                )?;
            }
        }
        Ok(tools)
    }

    fn policies(&self) -> PolicySet {
        PolicySet::new(vec![Arc::new(ApprovalModePolicy {
            mode: self.config.approval_mode,
            skills: self.skills(),
            honor_allowed_tools: self.config.skills.honor_allowed_tools,
        })])
    }

    /// The skills registry in use.
    #[must_use]
    pub fn skills(&self) -> Arc<SkillRegistry> {
        Arc::clone(&self.lock_skills())
    }

    /// The configuration the service was built with.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The catalogue in use.
    #[must_use]
    pub fn catalog(&self) -> Arc<Catalog> {
        Arc::clone(&self.lock_catalog())
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
                self.require_record(&session)?;
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
            Kind::ListModels => Ok(self.list_models_event(request_id)),
            Kind::ListSkills => Ok(self.skill_list_event(request_id)),
            Kind::RefreshSkills => Ok(self.refresh_skills_event(request_id)),
            Kind::RefreshCatalog => self.refresh_catalog(request_id).await,
            Kind::SetModel { session, model } => self.set_model_event(request_id, session, model),
            Kind::SetReasoning { .. } | Kind::SetBudget { .. } => Err(ServiceError::Unsupported(
                "session settings arrive in a later milestone",
            )),
            Kind::Approve {
                session,
                approval,
                arguments,
                scope,
                remember,
            } => {
                self.approve(request_id, session, approval, arguments, scope, remember)
                    .await
            }
            Kind::Deny {
                session,
                approval,
                reason,
            } => self.deny(request_id, session, approval, reason).await,
            Kind::Revoke {
                session,
                approval,
                grant,
            } => self.revoke(request_id, session, approval, grant).await,
            Kind::Steer { .. } => Err(ServiceError::Unsupported("steering arrives with the TUI")),
            Kind::Compact { .. } => Err(ServiceError::Unsupported(
                "compaction arrives in milestone 6",
            )),
        }
    }

    fn list_models_event(&self, request_id: u64) -> EventStream {
        single(
            request_id,
            None,
            ServerEventKind::ModelList {
                models: self.list_models(),
            },
        )
    }

    fn skill_list_event(&self, request_id: u64) -> EventStream {
        let skills = self.skills();
        let mut warnings = Vec::new();
        let mut infos = Vec::new();
        for skill in skills.skills() {
            infos.push(norte_code_protocol::SkillInfo {
                name: skill.name.clone(),
                description: skill.description.clone(),
                root: skill.root.display().to_string(),
                allowed_tools: skill.allowed_tools.clone(),
            });
        }
        for warning in skills.warnings() {
            warnings.push(format!("{}: {}", warning.path.display(), warning.message));
        }
        single(
            request_id,
            None,
            ServerEventKind::SkillList {
                skills: infos,
                warnings,
            },
        )
    }

    fn refresh_skills_event(&self, request_id: u64) -> EventStream {
        let registry = Arc::new(discover_skills(&self.config, &self.workspace_root));
        let count = registry.len();
        for warning in registry.warnings() {
            tracing::warn!(
                path = %warning.path.display(),
                "skipping skill: {}",
                warning.message
            );
        }
        *self.lock_skills_write() = registry;
        single(
            request_id,
            None,
            ServerEventKind::SkillsUpdated { skills: count },
        )
    }

    async fn refresh_catalog(&self, request_id: u64) -> Result<EventStream, ServiceError> {
        let loaded = load_catalog(&LoadOptions {
            url: &self.catalog_settings.url,
            ttl: self.catalog_settings.ttl,
            offline: self.catalog_settings.offline,
            cache_dir: &self.catalog_settings.cache_dir,
        })
        .await?;
        let models = loaded.catalog.model_count();
        *self.lock_catalog_write() = Arc::new(loaded.catalog);
        Ok(single(
            request_id,
            None,
            ServerEventKind::CatalogUpdated { models },
        ))
    }

    fn set_model_event(
        &self,
        request_id: u64,
        session: SessionKey,
        model: String,
    ) -> Result<EventStream, ServiceError> {
        let spec = ModelSpec::from_catalog(&self.config, &self.catalog(), &model, None)?;
        self.set_model(&session, spec)?;
        Ok(single(
            request_id,
            Some(session),
            ServerEventKind::ModelChanged { model },
        ))
    }

    async fn approve(
        self: &Arc<Self>,
        request_id: u64,
        session: SessionKey,
        approval: String,
        arguments: Option<serde_json::Value>,
        scope: Option<norte_code_protocol::GrantScope>,
        remember: Option<bool>,
    ) -> Result<EventStream, ServiceError> {
        if arguments.is_some() {
            return Err(ServiceError::Unsupported(
                "editing approval arguments is not implemented for this client",
            ));
        }
        let (nid, spec, _) = self.require_record(&session)?;
        let engine = self.engine(&spec)?;
        let id: ApprovalId = approval.parse().map_err(ServiceError::Norte)?;
        let record = engine
            .pending_approvals(nid)
            .await?
            .into_iter()
            .find(|record| record.id == id)
            .ok_or(ServiceError::Norte(norte::Error::ApprovalNotFound(id)))?;
        let grant = match (scope, remember) {
            (Some(norte_code_protocol::GrantScope::Always), _) => GrantScope::Global,
            (Some(norte_code_protocol::GrantScope::Session), _) | (_, Some(true)) => {
                GrantScope::Session(nid)
            }
            _ => GrantScope::Call(record.call_id),
        };
        engine.approve(id, grant, "user").await?;
        let head = single(
            request_id,
            Some(session.clone()),
            ServerEventKind::ApprovalResolved {
                approval: approval.clone(),
                approved: true,
            },
        );
        let resumed = self.start_turn(request_id, Some(session), None).await?;
        Ok(Box::pin(head.chain(resumed)))
    }

    async fn deny(
        self: &Arc<Self>,
        request_id: u64,
        session: SessionKey,
        approval: String,
        reason: Option<String>,
    ) -> Result<EventStream, ServiceError> {
        let (_, spec, _) = self.require_record(&session)?;
        let engine = self.engine(&spec)?;
        let id: ApprovalId = approval.parse().map_err(ServiceError::Norte)?;
        engine
            .deny(
                id,
                "user",
                reason.unwrap_or_else(|| "denied by the user".to_owned()),
            )
            .await?;
        let head = single(
            request_id,
            Some(session.clone()),
            ServerEventKind::ApprovalResolved {
                approval: approval.clone(),
                approved: false,
            },
        );
        let resumed = self.start_turn(request_id, Some(session), None).await?;
        Ok(Box::pin(head.chain(resumed)))
    }

    async fn revoke(
        &self,
        request_id: u64,
        session: SessionKey,
        approval: String,
        grant: String,
    ) -> Result<EventStream, ServiceError> {
        let (nid, spec, _) = self.require_record(&session)?;
        let engine = self.engine(&spec)?;
        let grant: GrantId = grant.parse().map_err(ServiceError::Norte)?;
        engine.revoke(nid, grant, "user").await?;
        Ok(single(
            request_id,
            Some(session),
            ServerEventKind::GrantRevoked {
                approval,
                grant: grant.to_string(),
            },
        ))
    }

    async fn create_session(&self, title: Option<String>) -> Result<SessionKey, ServiceError> {
        let spec = self.default_spec.clone();
        let engine = self.engine(&spec)?;
        let session = engine.create().await?;
        engine
            .append_history(
                session.id,
                vec![Message::system(self.system_prompt.clone())],
            )
            .await?;
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let record = SessionRecord {
            id: session.id,
            title,
            spec,
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
        let (source, spec, _) = self.require_record(from)?;
        let engine = self.engine(&spec)?;
        let conversation = engine.export(source).await?;
        let imported = engine.import(conversation).await?;
        let key = SessionKey::new(uuid::Uuid::new_v4().to_string());
        let record = SessionRecord {
            id: imported.id,
            title,
            spec,
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
        let (key, nid, spec) = if let Some(key) = session {
            let (nid, spec, _) = self.require_record(&key)?;
            (key, nid, spec)
        } else {
            let key = self.create_session(None).await?;
            let (nid, spec, _) = self.require_record(&key)?;
            (key, nid, spec)
        };
        let engine = self.engine(&spec)?;
        let state = engine.session(nid).await?;
        let turn = state.turn + 1;
        let (tx, rx) = mpsc::channel::<ServerEvent>(256);
        let notify = Arc::new(Notify::new());
        self.lock_cancels().insert(key.clone(), notify.clone());

        let this = Arc::clone(self);
        let task_key = key.clone();
        tokio::spawn(async move {
            let result = this
                .run_turn(
                    &engine, request_id, &task_key, nid, turn, input, &tx, &notify,
                )
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
        engine: &Engine,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        turn: u64,
        input: Option<String>,
        tx: &mpsc::Sender<ServerEvent>,
        notify: &Notify,
    ) -> Result<(), ServiceError> {
        let mut stream = input.map_or_else(
            || engine.resume(nid),
            |text| engine.turn(nid).input(Message::user(text)).start(),
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
                            if !self.handle(engine, request_id, key, nid, &event, &mut stats, tx).await {
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
        let _ = self
            .settle(engine, request_id, key, nid, &mut stats, tx)
            .await;
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
                let phase = engine
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
    #[allow(clippy::too_many_arguments)]
    async fn handle(
        &self,
        engine: &Engine,
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
            } => {
                let error = if *is_error {
                    tool_error(engine, nid, call_id).await
                } else {
                    None
                };
                Some(ServerEventKind::ToolFinished {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    is_error: *is_error,
                    error,
                    duration_ms: *duration_ms,
                })
            }
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
                return self.settle(engine, request_id, key, nid, stats, tx).await;
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
        engine: &Engine,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        stats: &mut Stats,
        tx: &mpsc::Sender<ServerEvent>,
    ) -> bool {
        let Ok(history) = engine.history(nid).await else {
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
                model: Some(record.spec.label()),
                created_at: Some(record.created_at.to_rfc3339()),
                phase: record.phase,
            })
            .collect()
    }

    /// Flattens the catalogue for a picker, applying the config overrides.
    fn list_models(&self) -> Vec<ModelInfo> {
        model_list(&self.config, &self.catalog())
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

    fn set_model(&self, key: &SessionKey, spec: ModelSpec) -> Result<(), ServiceError> {
        let mut lock = self.lock_sessions();
        let record = lock
            .get_mut(key)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        if record.phase != norte_code_protocol::Phase::Idle {
            return Err(ServiceError::Unsupported(
                "switch the model at a turn boundary",
            ));
        }
        record.spec = spec;
        Ok(())
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

    fn require_record(
        &self,
        key: &SessionKey,
    ) -> Result<(SessionId, ModelSpec, norte_code_protocol::Phase), ServiceError> {
        self.lock_sessions()
            .get(key)
            .map(|record| (record.id, record.spec.clone(), record.phase))
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))
    }

    fn lock_sessions(&self) -> MutexGuard<'_, BTreeMap<SessionKey, SessionRecord>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_cancels(&self) -> MutexGuard<'_, BTreeMap<SessionKey, Arc<Notify>>> {
        self.cancels.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_catalog(&self) -> RwLockReadGuard<'_, Arc<Catalog>> {
        self.catalog.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_catalog_write(&self) -> RwLockWriteGuard<'_, Arc<Catalog>> {
        self.catalog.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_skills(&self) -> RwLockReadGuard<'_, Arc<SkillRegistry>> {
        self.skills.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_skills_write(&self) -> RwLockWriteGuard<'_, Arc<SkillRegistry>> {
        self.skills.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Parks the tools a human must approve, according to the session's mode.
struct ApprovalModePolicy {
    mode: ApprovalMode,
    skills: Arc<SkillRegistry>,
    honor_allowed_tools: bool,
}

impl Policy for ApprovalModePolicy {
    fn name(&self) -> &'static str {
        "approval-mode"
    }

    fn decide(&self, view: &PolicyView<'_>) -> Decision {
        let tool = view.call.name.as_str();
        if self.honor_allowed_tools
            && self
                .skills
                .preapproves(view.session.id, tool, &view.call.arguments)
        {
            return Decision::Allow;
        }
        let needs_approval = match self.mode {
            ApprovalMode::Yolo => false,
            ApprovalMode::AutoEdit => matches!(tool, "command" | "skill_script"),
            ApprovalMode::Ask => {
                matches!(tool, "write" | "edit" | "command" | "skill_script")
            }
        };
        if needs_approval {
            Decision::RequireApproval(format!("allow `{tool}` to run with these arguments?"))
        } else {
            Decision::Allow
        }
    }
}

fn discover_skills(config: &Config, workspace: &Path) -> SkillRegistry {
    if !config.skills.enabled {
        return SkillRegistry::discover(&[], 1);
    }
    let roots =
        norte_code_skills::discovery_roots(workspace, &config.skills.paths, config.skills.interop);
    SkillRegistry::discover(&roots, config.skills.max_skills)
}

/// Every API key the config can resolve, for redaction in command output.
///
/// Missing keys are not an error here: only the selected provider needs one.
fn collect_secrets(config: &Config, catalog: &Catalog) -> Vec<String> {
    let mut secrets = Vec::new();
    for (id, provider) in &catalog.providers {
        if let Ok(key) = config.resolve_api_key(id, &provider.env) {
            secrets.push(key.expose().to_owned());
        }
    }
    for id in config.providers.keys() {
        let env = catalog
            .provider(id)
            .map_or_else(Vec::new, |provider| provider.env.clone());
        if let Ok(key) = config.resolve_api_key(id, &env) {
            secrets.push(key.expose().to_owned());
        }
    }
    secrets.retain(|secret| !secret.trim().is_empty());
    secrets.sort();
    secrets.dedup();
    secrets
}

/// Flattens a catalogue for a picker, applying the config overrides.
#[must_use]
pub fn model_list(config: &Config, catalog: &Catalog) -> Vec<ModelInfo> {
    let mut models = Vec::new();
    for entry in catalog.models() {
        let wire = Wire::from_npm(entry.provider.npm.as_deref().unwrap_or_default());
        let overrides = config
            .model_overrides
            .get(&format!("{}/{}", entry.provider.id, entry.model.id));
        let capabilities = norte_code_catalog::capabilities(entry.model, wire, overrides);
        let mut efforts: Vec<String> = capabilities
            .reasoning
            .efforts
            .iter()
            .map(|effort| effort_name(*effort).to_owned())
            .collect();
        efforts.sort();
        models.push(ModelInfo {
            provider: entry.provider.id.clone(),
            model: entry.model.id.clone(),
            name: entry.model.name.clone(),
            description: entry.model.description.clone(),
            max_context: capabilities.max_context,
            max_output: capabilities.max_output,
            tools: capabilities.tool_kinds.contains(&ToolKind::Function),
            input: entry.model.modalities.input.clone(),
            output: entry.model.modalities.output.clone(),
            reasoning: ReasoningInfo {
                supported: capabilities.reasoning.supported,
                can_disable: capabilities.reasoning.can_disable,
                efforts,
                supports_budget: capabilities.reasoning.supports_budget,
                max_budget: capabilities.reasoning.max_budget,
            },
            deprecated: entry.model.status.as_deref() == Some("deprecated"),
        });
    }
    models
}

fn effort_name(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        // `High` and any future top end map to Norte's highest effort.
        _ => "high",
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

/// The text of a failed tool result, read from the committed history.
async fn tool_error(
    engine: &Engine,
    session: SessionId,
    call_id: &norte::CallId,
) -> Option<String> {
    let history = engine.history(session).await.ok()?;
    for message in history.iter().rev() {
        for part in &message.parts {
            if let norte::Part::ToolResult(result) = part {
                if result.call_id == *call_id && result.is_error {
                    let text = result
                        .parts
                        .iter()
                        .filter_map(norte::Part::as_text)
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Some(text);
                }
            }
        }
    }
    None
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
    use norte::{
        CallId, Capabilities, InMemoryBlobStore, InMemoryStore, ModelId, ModelRef, ProviderId,
    };
    use norte_code_config::ApiKey;
    use norte_code_protocol::{ClientRequest, ClientRequestKind};
    use norte_testing::ScriptedModel;

    struct ScriptedFactory {
        model: Arc<ScriptedModel>,
    }

    impl ModelFactory for ScriptedFactory {
        fn build(
            &self,
            _spec: &ModelSpec,
            _capabilities: &Capabilities,
        ) -> Result<Arc<dyn Model>, ServiceError> {
            Ok(Arc::clone(&self.model) as Arc<dyn Model>)
        }
    }

    fn catalog() -> Arc<Catalog> {
        Arc::new(
            Catalog::parse(
                br#"{
                    "providers": {
                        "test": {
                            "id": "test",
                            "env": ["TEST_API_KEY"],
                            "npm": "@ai-sdk/openai-compatible",
                            "api": "https://example.invalid/v1",
                            "models": {
                                "scripted": {
                                    "id": "scripted",
                                    "name": "Scripted",
                                    "tool_call": true,
                                    "modalities": {"input": ["text"], "output": ["text"]},
                                    "limit": {"context": 1000, "output": 100}
                                },
                                "other": {
                                    "id": "other",
                                    "name": "Other",
                                    "tool_call": true,
                                    "modalities": {"input": ["text"], "output": ["text"]},
                                    "limit": {"context": 2000, "output": 200}
                                }
                            }
                        }
                    }
                }"#,
            )
            .expect("catalog"),
        )
    }

    fn spec(model: &str) -> ModelSpec {
        let catalog = catalog();
        let entry = catalog.find("test", model).expect("model");
        ModelSpec {
            provider: "test".to_owned(),
            model: model.to_owned(),
            wire: Wire::Chat,
            endpoint: Some("https://example.invalid/v1".to_owned()),
            api_key: ApiKey::new("test"),
            capabilities: norte_code_catalog::capabilities(entry, Wire::Chat, None),
        }
    }

    fn config() -> Config {
        let key_file =
            std::env::temp_dir().join(format!("norte-code-service-{}-key", std::process::id()));
        std::fs::write(&key_file, "sk-test\n").expect("key file");
        Config::from_json(&format!(
            r#"{{"providers":{{"test":{{"api_key_file":"{}"}}}}}}"#,
            key_file.display()
        ))
        .expect("config")
        .config
    }

    fn service() -> Arc<Service> {
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let model = Arc::new(ScriptedModel::new(reference).turn_deltas(["Hel", "lo"]));
        Arc::new(
            Service::new(
                config(),
                catalog(),
                spec("scripted"),
                Arc::new(ScriptedFactory { model }),
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
        let service = service();
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
        let service = service();
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
    async fn models_are_listed_with_their_catalogue_limits() {
        let service = service();
        let events = drain(&service, ClientRequestKind::ListModels).await;
        let ServerEventKind::ModelList { models } = &events[0].kind else {
            panic!("expected a model list");
        };
        assert_eq!(models.len(), 2);
        let scripted = models
            .iter()
            .find(|model| model.model == "scripted")
            .expect("scripted");
        assert_eq!(scripted.max_context, Some(1000));
        assert_eq!(scripted.max_output, Some(100));
        assert!(scripted.tools);
    }

    #[tokio::test]
    async fn a_session_can_switch_models_at_a_boundary() {
        let service = service();
        let created = drain(
            &service,
            ClientRequestKind::CreateSession {
                title: Some("switch".to_owned()),
            },
        )
        .await;
        let key = created[0].session.clone().expect("session");
        let changed = drain(
            &service,
            ClientRequestKind::SetModel {
                session: key.clone(),
                model: "test/other".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            changed[0].kind,
            ServerEventKind::ModelChanged { .. }
        ));
        let listed = drain(&service, ClientRequestKind::ListSessions).await;
        let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
            panic!("expected a list");
        };
        assert_eq!(sessions[0].model.as_deref(), Some("test/other"));
    }

    #[tokio::test]
    async fn a_write_parks_for_approval_and_resumes() {
        let target =
            std::env::temp_dir().join(format!("norte-code-write-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&target);
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let call = CallId::new();
        let model = Arc::new(
            ScriptedModel::new(reference)
                .turn_tool_call(
                    call,
                    "write",
                    &serde_json::json!({
                        "path": target.display().to_string(),
                        "content": "written by a tool",
                        "overwrite": true
                    }),
                )
                .turn_text("done"),
        );
        let service = Arc::new(
            Service::new(
                config(),
                catalog(),
                spec("scripted"),
                Arc::new(ScriptedFactory { model }),
                Arc::new(InMemoryStore::new()),
                Arc::new(InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );

        let first = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "write it".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        let session = first[0].session.clone().expect("session");
        let approval = first
            .iter()
            .find_map(|event| match &event.kind {
                ServerEventKind::ApprovalRequested { approval, .. } => Some(approval.clone()),
                _ => None,
            })
            .expect("approval request");
        assert!(first.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                phase: norte_code_protocol::Phase::AwaitingApproval,
                ..
            }
        )));
        assert!(!target.exists());

        let second = drain(
            &service,
            ClientRequestKind::Approve {
                session,
                approval,
                arguments: None,
                scope: None,
                remember: None,
            },
        )
        .await;
        assert!(second.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::ApprovalResolved { approved: true, .. }
        )));
        assert!(second.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::ToolFinished {
                ref tool,
                is_error: false,
                ..
            } if tool == "write"
        )));
        assert_eq!(
            std::fs::read_to_string(&target).expect("written file"),
            "written by a tool"
        );
        let _ = std::fs::remove_file(&target);
    }

    #[tokio::test]
    async fn skills_and_tools_are_listed_over_the_protocol() {
        let service = service();
        let skills = drain(&service, ClientRequestKind::ListSkills).await;
        assert!(matches!(skills[0].kind, ServerEventKind::SkillList { .. }));
        let refreshed = drain(&service, ClientRequestKind::RefreshSkills).await;
        assert!(matches!(
            refreshed[0].kind,
            ServerEventKind::SkillsUpdated { .. }
        ));
    }

    #[tokio::test]
    async fn an_unknown_session_is_an_error() {
        let service = service();
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
