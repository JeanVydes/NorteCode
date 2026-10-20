//! The in-process service: it owns the Norte engines, maps the client protocol
//! onto turns, and streams the engine's events back as [`ServerEvent`]s.
//!
//! An engine is cheap and shares the store and blob store, so each turn builds
//! one for the model the session selected. That is what makes a mid-session
//! model switch a change of model rather than a change of session.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_core::Stream;
use futures_util::StreamExt;
use norte::{
    ApprovalId, BlobStore, CallId, Continuation, Decision, Engine, EngineBuilder, EngineConfig,
    EngineEvent, FinishReason, FnExecutor, GrantId, GrantScope, InMemoryBlobStore, InMemoryStore,
    Layer, LayerContext, LayerStack, Message, Model, ModelRequest, ModelStream, Policy, PolicySet,
    PolicyView, Reasoning as ReasoningCapability, ReasoningEffort, ReasoningRequest, SessionId,
    ShortCircuitReason, Store, StreamDelta, ToolArgs, ToolContext, ToolDeclaration, ToolKind,
    ToolOutput, ToolRegistry, Usage,
};
use norte_code_cost::{BudgetBook, BudgetNotice, CostRecord, CostSummary, summarize};
use norte_code_skills::{
    SkillRegistry, SkillResourceTool, SkillScriptConfig, SkillScriptTool, SkillTool,
};
use norte_code_store_sqlite::{InMemoryDirectory, SessionDirectory, StoredQuestion, StoredSession};
use norte_code_tools::{
    AskUserTool, DurableQuestion, GlobTool, GrepTool, ListDirTool, QuestionBroker, QuestionStore,
    TodoState, TodoTool, WebFetchTool,
};
use norte_tools::{CommandTool, EditTool, ReadTool, WriteTool};
use tokio::sync::{Notify, mpsc};

use norte_code_catalog::{Catalog, LoadOptions, load_catalog};
use norte_code_config::{ApprovalMode, BudgetConfig, Config};
use norte_code_protocol::{
    Budget, BudgetUnit, Delta, ErrorCode, ModelInfo, Reasoning as ProtocolReasoning, ReasoningInfo,
    ServerEvent, ServerEventKind, SessionInfo, SessionKey,
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
    /// Input tokens, summed over reported requests.
    input_tokens: u64,
    /// Output tokens, summed over reported requests.
    output_tokens: u64,
    /// The session budget, when one was set.
    budget: Option<Budget>,
    /// Tools this session refuses without asking again.
    denied_tools: Vec<String>,
}

/// Live counters for the turn in flight.
#[derive(Debug, Clone, Copy)]
struct Stats {
    turn: u64,
    billed: usize,
    requests: u64,
    cost: f64,
    unpriced: u64,
    input_tokens: u64,
    output_tokens: u64,
}

/// Where the catalogue is refreshed from.
#[derive(Clone, Debug)]
struct CatalogSettings {
    url: String,
    ttl: Duration,
    offline: bool,
    cache_dir: PathBuf,
}

/// The static text a request carries before any conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PromptSize {
    /// Characters in the system prompt.
    pub system_chars: usize,
    /// Characters in the serialized tool declarations.
    pub tool_chars: usize,
}

impl PromptSize {
    /// Characters in the system prompt plus the tool declarations.
    #[must_use]
    pub fn total_chars(self) -> usize {
        self.system_chars.saturating_add(self.tool_chars)
    }
}

/// Live arguments and results for tool calls in flight.
///
/// Norte's engine events carry no output, and a result is not in history until
/// the turn commits, so the host records both sides around the executor. The
/// renderer gets the effective arguments and the text the model will see.
#[derive(Default)]
struct ToolActivity {
    arguments: Mutex<BTreeMap<CallId, serde_json::Value>>,
    outputs: Mutex<BTreeMap<CallId, ToolOutput>>,
}

impl ToolActivity {
    fn started(&self, call_id: &CallId) -> Option<serde_json::Value> {
        self.arguments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(call_id)
    }

    fn finished(&self, call_id: &CallId) -> Option<ToolOutput> {
        self.outputs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(call_id)
    }
}

/// A tool executor that keeps the output for the renderer.
struct RecordingExecutor<A> {
    inner: Arc<dyn FnExecutor<A, ToolOutput>>,
    activity: Arc<ToolActivity>,
}

#[async_trait]
impl<A> FnExecutor<A, ToolOutput> for RecordingExecutor<A>
where
    A: ToolArgs,
{
    async fn call(&self, args: A) -> norte::Result<ToolOutput> {
        self.inner.call(args).await
    }

    async fn call_with_context(&self, context: &ToolContext, args: A) -> norte::Result<ToolOutput> {
        let result = self.inner.call_with_context(context, args).await;
        if let Some(call_id) = context.call_id {
            let output = match &result {
                Ok(output) => output.clone(),
                Err(error) => ToolOutput::error(error.to_string()),
            };
            self.activity
                .outputs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(call_id, output);
        }
        result
    }
}

/// Wraps a rich tool declaration so its output is visible to the host.
fn recording_tool<A>(
    declaration: ToolDeclaration<A>,
    activity: Arc<ToolActivity>,
) -> ToolDeclaration<A>
where
    A: ToolArgs + serde::de::DeserializeOwned + std::fmt::Debug + 'static,
{
    ToolDeclaration {
        name: declaration.name,
        description: declaration.description,
        parameters: declaration.parameters,
        executor: Arc::new(RecordingExecutor {
            inner: declaration.executor,
            activity,
        }),
        approval: declaration.approval,
    }
}

/// The text a tool result carries, with structured output as a fallback.
fn output_text(output: &ToolOutput) -> Option<String> {
    let text = output
        .parts
        .iter()
        .filter_map(norte::Part::as_text)
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        output.structured.as_ref().map(ToString::to_string)
    } else {
        Some(text)
    }
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
    questions: Arc<QuestionBroker>,
    secrets: Vec<String>,
    directory: Arc<dyn SessionDirectory>,
    effects: Arc<EffectJournal>,
    activity: Arc<ToolActivity>,
    sessions: Mutex<BTreeMap<SessionKey, SessionRecord>>,
    cancels: Mutex<BTreeMap<SessionKey, Arc<Notify>>>,
    budget_states: Mutex<BTreeMap<SessionKey, Arc<Mutex<BudgetBook>>>>,
    approval_rules: Arc<ApprovalRules>,
    config_path: Mutex<Option<PathBuf>>,
}

impl Service {
    /// Builds a service with an in-memory session directory.
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
        Self::build(
            config,
            catalog,
            default_spec,
            factory,
            store,
            blobs,
            Arc::new(InMemoryDirectory::new()),
            system_prompt,
        )
    }

    /// Builds a service that restores its sessions from `directory`.
    ///
    /// Mirrors [`Service::new`] but reloads the logical sessions a previous
    /// process persisted. A stored session whose model can no longer be
    /// resolved falls back to the default spec instead of aborting startup.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the default model cannot be built or the
    /// directory cannot be read.
    #[allow(clippy::too_many_arguments)]
    pub async fn with_directory(
        config: Config,
        catalog: Arc<Catalog>,
        default_spec: ModelSpec,
        factory: Arc<dyn ModelFactory>,
        store: Arc<dyn Store>,
        blobs: Arc<dyn BlobStore>,
        directory: Arc<dyn SessionDirectory>,
        system_prompt: impl Into<String>,
    ) -> Result<Self, ServiceError> {
        let service = Self::build(
            config,
            catalog,
            default_spec,
            factory,
            store,
            blobs,
            directory,
            system_prompt,
        )?;
        service.directory.repair_interrupted().await?;
        service.load_sessions().await?;
        service.effects.load().await?;
        Ok(service)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        config: Config,
        catalog: Arc<Catalog>,
        default_spec: ModelSpec,
        factory: Arc<dyn ModelFactory>,
        store: Arc<dyn Store>,
        blobs: Arc<dyn BlobStore>,
        directory: Arc<dyn SessionDirectory>,
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
        let workspace_root = std::fs::canonicalize(&workspace_root).unwrap_or(workspace_root);
        let secrets = collect_secrets(&config, &catalog);
        let skills = Arc::new(discover_skills(&config, &workspace_root));
        let questions = Arc::new(
            QuestionBroker::new()
                .with_store(Arc::new(DirectoryQuestions::new(Arc::clone(&directory)))),
        );
        let effects = Arc::new(EffectJournal::new(Arc::clone(&directory)));
        let activity = Arc::new(ToolActivity::default());
        let approval_rules = Arc::new(ApprovalRules::new(&config.approvals));
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
            questions,
            secrets,
            directory,
            effects,
            activity,
            sessions: Mutex::new(BTreeMap::new()),
            cancels: Mutex::new(BTreeMap::new()),
            budget_states: Mutex::new(BTreeMap::new()),
            approval_rules,
            config_path: Mutex::new(norte_code_config::user_config_path()),
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

    /// Rebuilds the in-memory session map from the durable directory.
    #[allow(clippy::too_many_lines)]
    async fn load_sessions(&self) -> Result<(), ServiceError> {
        let stored = self.directory.sessions().await?;
        let catalog = self.catalog();
        let mut records = Vec::with_capacity(stored.len());
        let mut repaired = Vec::new();
        for entry in stored {
            let key = SessionKey::new(entry.key.clone());
            let stored_phase = entry.phase.clone();
            let spec = match ModelSpec::from_catalog(&self.config, &catalog, &entry.model, None) {
                Ok(spec) => spec,
                Err(error) => {
                    tracing::warn!(
                        session = %entry.key,
                        model = %entry.model,
                        %error,
                        "stored session model is unavailable; using the default"
                    );
                    self.default_spec.clone()
                }
            };
            // Norte's store is the authority on phase: a parked tool or
            // approval committed it, while the directory snapshot may predate
            // the park. `streaming` is the exception: no process is streaming
            // a session it just loaded, so a hard kill left it behind. Put the
            // store back to idle and rewrite the directory row.
            let mut needs_repair = stored_phase == "streaming";
            let phase = match self.store.load(entry.north).await {
                Ok(session) if session.phase == norte::Phase::Streaming => {
                    needs_repair = true;
                    let commit = norte::Commit {
                        session_id: entry.north,
                        expected_version: Some(session.version),
                        nodes: Vec::new(),
                        head: session.head,
                        phase: norte::Phase::Idle,
                        turn: session.turn,
                        queue: session.queue.clone(),
                        grants: session.grants.clone(),
                        metadata: Some(session.metadata.clone()),
                    };
                    if let Err(error) = self.store.commit(commit).await {
                        tracing::warn!(
                            session = %entry.key,
                            %error,
                            "could not mark an interrupted session idle"
                        );
                    }
                    norte_code_protocol::Phase::Idle
                }
                Ok(session) => to_phase(session.phase),
                Err(error) => {
                    tracing::warn!(
                        session = %entry.key,
                        %error,
                        "could not read the session phase; using the directory copy"
                    );
                    match stored_phase.as_str() {
                        "awaiting_tool" => norte_code_protocol::Phase::AwaitingTool,
                        "awaiting_approval" => norte_code_protocol::Phase::AwaitingApproval,
                        // `streaming` cannot survive a process; everything
                        // else starts idle.
                        _ => norte_code_protocol::Phase::Idle,
                    }
                }
            };
            self.approval_rules
                .load_session(entry.north, &entry.denied_tools);
            let stats = &entry.stats;
            let record = SessionRecord {
                id: entry.north,
                title: entry.title,
                spec,
                created_at: entry.created_at,
                phase,
                billed: stats
                    .get("billed")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(0),
                requests: stats
                    .get("requests")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                cost: stats
                    .get("cost")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0),
                unpriced: stats
                    .get("unpriced")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                input_tokens: stats
                    .get("input_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                output_tokens: stats
                    .get("output_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                budget: stats
                    .get("budget")
                    .and_then(|value| serde_json::from_value(value.clone()).ok()),
                denied_tools: entry.denied_tools.clone(),
            };
            records.push((key.clone(), record));
            if needs_repair {
                repaired.push(key);
            }
        }
        {
            let mut sessions = self.lock_sessions();
            for (key, record) in records {
                sessions.insert(key, record);
            }
        }
        for key in &repaired {
            self.persist(key).await?;
        }
        Ok(())
    }

    /// Writes one session back to the directory.
    async fn persist(&self, key: &SessionKey) -> Result<(), ServiceError> {
        let stored = self.lock_sessions().get(key).map(|record| StoredSession {
            key: key.to_string(),
            north: record.id,
            title: record.title.clone(),
            model: format!("{}/{}", record.spec.provider, record.spec.model),
            created_at: record.created_at,
            phase: match record.phase {
                norte_code_protocol::Phase::Idle => "idle",
                norte_code_protocol::Phase::Streaming => "streaming",
                norte_code_protocol::Phase::AwaitingTool => "awaiting_tool",
                norte_code_protocol::Phase::AwaitingApproval => "awaiting_approval",
            }
            .to_owned(),
            stats: serde_json::json!({
                "billed": record.billed,
                "requests": record.requests,
                "cost": record.cost,
                "unpriced": record.unpriced,
                "input_tokens": record.input_tokens,
                "output_tokens": record.output_tokens,
                "budget": record.budget,
            }),
            denied_tools: record.denied_tools.clone(),
        });
        if let Some(stored) = stored {
            self.directory.save_session(&stored).await?;
        }
        Ok(())
    }

    /// Builds an engine for one spec over the shared stores.
    fn engine(&self, spec: &ModelSpec) -> Result<Engine, ServiceError> {
        self.engine_with_budget(spec, None)
    }

    /// Builds an engine whose requests pass through the budget layer.
    fn engine_with_budget(
        &self,
        spec: &ModelSpec,
        budget: Option<Arc<Mutex<BudgetBook>>>,
    ) -> Result<Engine, ServiceError> {
        let model: Arc<dyn Model> = self.factory.build(spec, &spec.capabilities)?;
        let mut builder = EngineBuilder::new()
            .store(Arc::clone(&self.store))
            .blobs(Arc::clone(&self.blobs))
            .model(model)
            .tools(self.tools()?)
            .policies(self.policies())
            .config(EngineConfig {
                max_iterations: self.config.max_iterations.unwrap_or(usize::MAX),
                ..EngineConfig::default()
            })
            .tool_metadata({
                let effects = Arc::clone(&self.effects);
                let activity = Arc::clone(&self.activity);
                move |info| {
                    effects.intent(info.session, info.call_id, info.tool.as_str());
                    activity
                        .arguments
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(info.call_id, info.arguments.clone());
                    BTreeMap::new()
                }
            });
        if let Some(book) = budget {
            builder = builder.layers(LayerStack::new(vec![Arc::new(BudgetLayer { book })]));
        }
        Ok(builder.build()?)
    }

    /// Measures the static prompt material: the system prompt and the tool
    /// declarations. No token estimate is made, because only the provider
    /// counts tokens.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the configured tool registry cannot be
    /// built.
    pub fn prompt_size(&self) -> Result<PromptSize, ServiceError> {
        let tool_chars = self
            .tools()?
            .specs()
            .iter()
            .map(|spec| serde_json::to_string(spec).map_or(0, |json| json.chars().count()))
            .sum();
        Ok(PromptSize {
            system_chars: self.system_prompt.chars().count(),
            tool_chars,
        })
    }

    /// Registers one rich tool with the activity recorder attached.
    fn register_tool<A>(
        &self,
        tools: &mut ToolRegistry,
        declaration: ToolDeclaration<A>,
    ) -> Result<(), ServiceError>
    where
        A: ToolArgs + serde::de::DeserializeOwned + std::fmt::Debug + 'static,
    {
        tools.register_tool_declaration(recording_tool(declaration, Arc::clone(&self.activity)))?;
        Ok(())
    }

    /// The tool registry every engine shares, rebuilt cheaply per turn because
    /// Norte's registry is immutable and not cloneable.
    fn tools(&self) -> Result<ToolRegistry, ServiceError> {
        let mut tools = ToolRegistry::new();
        self.register_tool(&mut tools, ReadTool::new().into_declaration())?;
        self.register_tool(&mut tools, WriteTool::new().into_declaration())?;
        self.register_tool(&mut tools, EditTool::new().into_declaration())?;

        let mut env = BTreeMap::new();
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_owned(), path);
        }
        let command = CommandTool::new()
            .with_env(env)
            .with_secrets(self.secrets.clone())
            .with_blobs(Arc::clone(&self.blobs))
            .into_declaration();
        self.register_tool(&mut tools, command)?;
        self.register_tool(
            &mut tools,
            GlobTool::new(&self.workspace_root).into_declaration(),
        )?;
        self.register_tool(
            &mut tools,
            ListDirTool::new(&self.workspace_root).into_declaration(),
        )?;
        self.register_tool(
            &mut tools,
            GrepTool::new(&self.workspace_root).into_declaration(),
        )?;
        self.register_tool(
            &mut tools,
            TodoTool::new(Arc::clone(&self.todo)).into_declaration(),
        )?;
        self.register_tool(
            &mut tools,
            AskUserTool::new(Arc::clone(&self.questions)).into_declaration(),
        )?;
        if self.config.tools.web_fetch {
            self.register_tool(&mut tools, WebFetchTool::new().into_declaration())?;
        }

        if self.config.skills.enabled {
            let skills = self.skills();
            self.register_tool(
                &mut tools,
                SkillTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            self.register_tool(
                &mut tools,
                SkillResourceTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            if self.config.skills.allow_scripts {
                self.register_tool(
                    &mut tools,
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
        PolicySet::new(vec![
            Arc::new(EffectJournalPolicy {
                journal: Arc::clone(&self.effects),
            }),
            Arc::new(WorkspacePolicy {
                root: self.workspace_root.clone(),
            }),
            Arc::new(ApprovalModePolicy {
                mode: self.config.approval_mode,
                skills: self.skills(),
                honor_allowed_tools: self.config.skills.honor_allowed_tools,
                rules: Arc::clone(&self.approval_rules),
            }),
        ])
    }

    /// Selects the config file global approval rules are written to.
    ///
    /// The CLI calls this after loading `--config`, so "always" lands in the
    /// file the user actually named. Without it, the platform user config path
    /// is used.
    pub fn set_config_path(&self, path: Option<PathBuf>) {
        *self
            .config_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = path;
    }

    /// Writes one tool-wide rule to the user config and applies it in memory.
    fn remember_global_rule(&self, tool: &str, allow: bool) -> Result<(), ServiceError> {
        let path = self
            .config_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .or_else(norte_code_config::user_config_path)
            .ok_or_else(|| {
                ServiceError::Approval(
                    "no user config path; set --config before choosing a global rule".to_owned(),
                )
            })?;
        norte_code_config::update_user_approval(&path, tool, allow)?;
        if allow {
            self.approval_rules.allow_global(tool);
        } else {
            self.approval_rules.deny_global(tool);
        }
        Ok(())
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
    #[allow(clippy::too_many_lines)]
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
                self.rename(&session, &title).await?;
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::SessionRenamed { title },
                ))
            }
            Kind::DeleteSession { session } => {
                self.delete(&session).await?;
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::SessionDeleted,
                ))
            }
            Kind::SendMessage {
                session,
                text,
                budget,
                reasoning,
            } => {
                self.start_turn(request_id, session, Some(text), Some(budget), reasoning)
                    .await
            }
            Kind::Resume { session } => {
                self.start_turn(request_id, Some(session), None, None, None)
                    .await
            }
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
            Kind::SetModel { session, model } => {
                self.set_model_event(request_id, session, model).await
            }
            Kind::SetBudget { session, budget } => {
                self.set_budget(request_id, session, budget).await
            }
            Kind::SetReasoning { .. } => Err(ServiceError::Unsupported(
                "reasoning overrides arrive in a later milestone",
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
                scope,
            } => {
                self.deny(request_id, session, approval, reason, scope)
                    .await
            }
            Kind::Revoke {
                session,
                approval,
                grant,
            } => self.revoke(request_id, session, approval, grant).await,
            Kind::AnswerQuestion {
                session,
                question,
                answer,
            } => {
                self.answer_question(request_id, session, question, answer)
                    .await
            }
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

    async fn set_model_event(
        &self,
        request_id: u64,
        session: SessionKey,
        model: String,
    ) -> Result<EventStream, ServiceError> {
        let spec = ModelSpec::from_catalog(&self.config, &self.catalog(), &model, None)?;
        self.ensure_model_supports_budget(&session, &spec).await?;
        self.set_model(&session, spec).await?;
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
        let global = matches!(scope, Some(norte_code_protocol::GrantScope::Always));
        if global {
            self.remember_global_rule(record.tool.as_str(), true)?;
        }
        let grant = match (scope, remember) {
            (Some(norte_code_protocol::GrantScope::Always), _) => GrantScope::Global,
            (Some(norte_code_protocol::GrantScope::Session), _) => GrantScope::Session(nid),
            (Some(norte_code_protocol::GrantScope::Tool), _) | (_, Some(true)) => {
                GrantScope::Tool(record.tool.clone())
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
        let resumed = self
            .start_turn(request_id, Some(session), None, None, None)
            .await?;
        Ok(Box::pin(head.chain(resumed)))
    }

    async fn deny(
        self: &Arc<Self>,
        request_id: u64,
        session: SessionKey,
        approval: String,
        reason: Option<String>,
        scope: Option<norte_code_protocol::DenyScope>,
    ) -> Result<EventStream, ServiceError> {
        let (nid, spec, _) = self.require_record(&session)?;
        let engine = self.engine(&spec)?;
        let id: ApprovalId = approval.parse().map_err(ServiceError::Norte)?;
        let record = engine
            .pending_approvals(nid)
            .await?
            .into_iter()
            .find(|record| record.id == id)
            .ok_or(ServiceError::Norte(norte::Error::ApprovalNotFound(id)))?;
        let tool = record.tool.as_str();
        match scope {
            Some(norte_code_protocol::DenyScope::ToolSession) => {
                self.approval_rules.deny_session(nid, tool);
                if let Some(record) = self.lock_sessions().get_mut(&session) {
                    if !record.denied_tools.iter().any(|denied| denied == tool) {
                        record.denied_tools.push(tool.to_owned());
                    }
                }
                self.persist(&session).await?;
            }
            Some(norte_code_protocol::DenyScope::ToolGlobal) => {
                self.remember_global_rule(tool, false)?;
            }
            Some(norte_code_protocol::DenyScope::Once) | None => {}
        }
        engine.deny(id, "user", reason.unwrap_or_default()).await?;
        let head = single(
            request_id,
            Some(session.clone()),
            ServerEventKind::ApprovalResolved {
                approval: approval.clone(),
                approved: false,
            },
        );
        let resumed = self
            .start_turn(request_id, Some(session), None, None, None)
            .await?;
        Ok(Box::pin(head.chain(resumed)))
    }

    async fn answer_question(
        &self,
        request_id: u64,
        session: SessionKey,
        question: String,
        answer: String,
    ) -> Result<EventStream, ServiceError> {
        let (nid, _, _) = self.require_record(&session)?;
        if !self.questions.answer(Some(nid), &question, answer).await {
            return Err(ServiceError::UnknownQuestion(question));
        }
        Ok(single(
            request_id,
            Some(session),
            ServerEventKind::QuestionAnswered { question },
        ))
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
            input_tokens: 0,
            output_tokens: 0,
            budget: self.configured_session_budget(),
            denied_tools: Vec::new(),
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
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
            input_tokens: 0,
            output_tokens: 0,
            budget: self.configured_session_budget(),
            denied_tools: Vec::new(),
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
        Ok(key)
    }

    /// Starts a turn (or a resume) and returns the stream of its events.
    ///
    /// The outer option distinguishes a new message (which replaces the
    /// message budget) from a resume (which keeps the one already in force).
    #[allow(clippy::option_option, clippy::too_many_arguments)]
    async fn start_turn(
        self: &Arc<Self>,
        request_id: u64,
        session: Option<SessionKey>,
        input: Option<String>,
        message_budget: Option<Option<Budget>>,
        reasoning: Option<ProtocolReasoning>,
    ) -> Result<EventStream, ServiceError> {
        let (key, nid, spec) = if let Some(key) = session {
            let (nid, spec, _) = self.require_record(&key)?;
            (key, nid, spec)
        } else {
            let key = self.create_session(None).await?;
            let (nid, spec, _) = self.require_record(&key)?;
            (key, nid, spec)
        };
        let reasoning = resolve_reasoning(
            reasoning.as_ref(),
            &self.config.reasoning.default,
            &spec.capabilities.reasoning,
        )?;
        let configured_message = configured_budget(self.config.budgets.message.as_ref());
        let message = message_budget.map(|budget| budget.or(configured_message.clone()));
        if let Some(budget) = message.as_ref().and_then(Option::as_ref) {
            self.validate_budget(&key, budget).await?;
        }
        let existing = self.lock_budget_states().get(&key).cloned();
        let book = if let Some(message) = message {
            let book = self.new_budget_book(&key, message).await?;
            self.lock_budget_states()
                .insert(key.clone(), Arc::clone(&book));
            book
        } else if let Some(book) = existing {
            book
        } else {
            let book = self.new_budget_book(&key, configured_message).await?;
            self.lock_budget_states()
                .insert(key.clone(), Arc::clone(&book));
            book
        };
        let engine = self.engine_with_budget(&spec, Some(book))?;
        let state = engine.session(nid).await?;
        let turn = state.turn + 1;
        // A resumable park: after a restart the engine will not re-emit the
        // pending approvals, so surface them from the store before the turn.
        let restored: Vec<ServerEvent> = if state.phase == norte::Phase::AwaitingApproval {
            let mut restored = Vec::new();
            for record in engine.pending_approvals(nid).await? {
                let arguments = call_arguments(&engine, nid, record.call_id).await;
                restored.push(ServerEvent::new(
                    request_id,
                    Some(key.clone()),
                    ServerEventKind::ApprovalRequested {
                        turn,
                        approval: record.id.to_string(),
                        call_id: record.call_id.to_string(),
                        tool: record.tool.to_string(),
                        arguments,
                    },
                ));
            }
            restored
        } else {
            Vec::new()
        };
        let (tx, rx) = mpsc::channel::<ServerEvent>(256);
        let notify = Arc::new(Notify::new());
        self.lock_cancels().insert(key.clone(), notify.clone());

        let this = Arc::clone(self);
        let task_key = key.clone();
        tokio::spawn(async move {
            let result = this
                .run_turn(
                    &engine, request_id, &task_key, nid, turn, input, reasoning, &tx, &notify,
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

        Ok(Box::pin(futures_util::stream::iter(restored).chain(
            futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|event| (event, rx))
            }),
        )))
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_turn(
        &self,
        engine: &Engine,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        turn: u64,
        input: Option<String>,
        reasoning: ReasoningRequest,
        tx: &mpsc::Sender<ServerEvent>,
        notify: &Notify,
    ) -> Result<(), ServiceError> {
        let mut turn_builder = engine.turn(nid).reasoning(reasoning);
        if let Some(text) = input {
            turn_builder = turn_builder.input(Message::user(text));
        }
        for (key, value) in &self.config.extensions {
            turn_builder = turn_builder.extension(key.clone(), value.clone());
        }
        let mut stream = turn_builder.start();
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
        let mut questions = self.questions.subscribe();
        loop {
            tokio::select! {
                biased;
                () = notify.notified() => {
                    cancelled = true;
                }
                question = questions.recv() => {
                    if let Ok(question) = question {
                        if question.session == Some(nid) {
                            if !emit(
                                tx,
                                ServerEvent::new(
                                    request_id,
                                    Some(key.clone()),
                                    ServerEventKind::QuestionAsked {
                                        turn,
                                        question: question.id,
                                        prompt: question.question,
                                        options: question.options,
                                        multiple: question.multiple,
                                    },
                                ),
                            )
                            .await
                            {
                                return Ok(());
                            }
                            self.set_phase(key, norte_code_protocol::Phase::AwaitingTool).await;
                        }
                    }
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
            .settle(engine, request_id, key, nid, &mut stats, tx, true)
            .await;
        self.store_stats(key, &stats).await;

        match outcome {
            Ok(outcome) => {
                let phase = to_phase(outcome.session.phase);
                self.set_phase(key, phase).await;
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
                self.set_phase(key, phase).await;
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
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
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
                let arguments = if let Some(arguments) = self.activity.started(call_id) {
                    Some(arguments)
                } else {
                    call_arguments(engine, nid, *call_id).await
                };
                Some(ServerEventKind::ToolStarted {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    arguments,
                })
            }
            EngineEvent::ToolCallFinished {
                call_id,
                tool,
                is_error,
                duration_ms,
                ..
            } => {
                self.effects
                    .finished(nid, *call_id, tool.as_str(), *is_error);
                let recorded = self.activity.finished(call_id);
                let error = if *is_error {
                    if let Some(text) = recorded.as_ref().and_then(output_text) {
                        Some(text)
                    } else {
                        tool_error(engine, nid, call_id).await
                    }
                } else {
                    None
                };
                let (output, truncated) = if *is_error {
                    (None, false)
                } else {
                    let text = recorded
                        .as_ref()
                        .and_then(output_text)
                        .or(tool_result_text(engine, nid, call_id).await);
                    text.map_or((None, false), |text| {
                        let (text, truncated) = shorten_tool_output(text);
                        (Some(text), truncated)
                    })
                };
                Some(ServerEventKind::ToolFinished {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    is_error: *is_error,
                    error,
                    output,
                    truncated,
                    duration_ms: *duration_ms,
                })
            }
            EngineEvent::ApprovalRequested {
                approval,
                call_id,
                tool,
                ..
            } => {
                let arguments = call_arguments(engine, nid, *call_id).await;
                Some(ServerEventKind::ApprovalRequested {
                    turn: stats.turn,
                    approval: approval.to_string(),
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    arguments,
                })
            }
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
            EngineEvent::LayerShortCircuited { layer, .. } if layer == "budget" => self
                .budget_stopped(key)
                .map(|status| ServerEventKind::BudgetExceeded {
                    scope: status.scope,
                    unit: status.unit,
                    spent: status.spent,
                    limit: status.limit,
                    action: status.action,
                }),
            EngineEvent::Error { message, .. } => {
                if self.budget_stopped(key).is_some() {
                    None
                } else {
                    Some(ServerEventKind::Error {
                        code: ErrorCode::Provider,
                        message: message.clone(),
                    })
                }
            }
            EngineEvent::RequestReconstructed { .. } => {
                return self
                    .settle(engine, request_id, key, nid, stats, tx, false)
                    .await;
            }
            _ => None,
        };
        if let Some(kind) = kind {
            return emit(tx, ServerEvent::new(request_id, Some(key.clone()), kind)).await;
        }
        true
    }

    /// Reports every model request committed since the last look.
    #[allow(clippy::too_many_arguments)]
    async fn settle(
        &self,
        engine: &Engine,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        stats: &mut Stats,
        tx: &mpsc::Sender<ServerEvent>,
        final_check: bool,
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
            stats.input_tokens += origin.usage.input_tokens.unwrap_or(0);
            stats.output_tokens += origin.usage.output_tokens.unwrap_or(0);
            let record = CostRecord {
                session: key.to_string(),
                turn: stats.turn,
                index: stats.requests,
                model: self.model_label(key),
                input_tokens: origin.usage.input_tokens,
                output_tokens: origin.usage.output_tokens,
                reasoning_tokens: origin.usage.reasoning_tokens,
                cache_read_tokens: origin.usage.cache_read_tokens,
                cache_write_tokens: origin.usage.cache_write_tokens,
                cost_usd: cost,
                recorded_at: Utc::now(),
            };
            if let Err(error) = self.directory.record_cost(&record).await {
                tracing::warn!(session = %key, %error, "could not record request cost");
            }
            let notices =
                self.lock_budget_states()
                    .get(key)
                    .cloned()
                    .map_or_else(Vec::new, |book| {
                        let mut book = book.lock().unwrap_or_else(PoisonError::into_inner);
                        book.record(&record);
                        if final_check {
                            book.notices()
                        } else {
                            book.warnings()
                        }
                    });
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
            for notice in notices {
                let kind = match notice {
                    BudgetNotice::Warning(warning) => ServerEventKind::BudgetWarning {
                        scope: warning.scope,
                        unit: warning.unit,
                        spent: warning.spent,
                        limit: warning.limit,
                    },
                    BudgetNotice::Exceeded(exceeded) => ServerEventKind::BudgetExceeded {
                        scope: exceeded.scope,
                        unit: exceeded.unit,
                        spent: exceeded.spent,
                        limit: exceeded.limit,
                        action: exceeded.action,
                    },
                };
                if !emit(tx, ServerEvent::new(request_id, Some(key.clone()), kind)).await {
                    stats.billed = history.len();
                    return false;
                }
            }
        }
        stats.billed = history.len();
        true
    }

    /// The `provider/model` a session currently uses.
    fn model_label(&self, key: &SessionKey) -> String {
        self.lock_sessions()
            .get(key)
            .map_or_else(String::new, |record| record.spec.label())
    }

    /// The budget decision that stopped the turn, if the layer made one.
    fn budget_stopped(&self, key: &SessionKey) -> Option<norte_code_cost::BudgetStatus> {
        self.lock_budget_states().get(key).and_then(|book| {
            book.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .stopped()
                .cloned()
        })
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
                input_tokens: 0,
                output_tokens: 0,
            },
            |record| Stats {
                turn,
                billed: record.billed,
                requests: record.requests,
                cost: record.cost,
                unpriced: record.unpriced,
                input_tokens: record.input_tokens,
                output_tokens: record.output_tokens,
            },
        )
    }

    async fn store_stats(&self, key: &SessionKey, stats: &Stats) {
        if let Some(record) = self.lock_sessions().get_mut(key) {
            record.billed = stats.billed;
            record.requests = stats.requests;
            record.cost = stats.cost;
            record.unpriced = stats.unpriced;
            record.input_tokens = stats.input_tokens;
            record.output_tokens = stats.output_tokens;
        }
        if let Err(error) = self.persist(key).await {
            tracing::warn!(session = %key, %error, "could not persist session counters");
        }
    }

    async fn set_phase(&self, key: &SessionKey, phase: norte_code_protocol::Phase) {
        if let Some(record) = self.lock_sessions().get_mut(key) {
            record.phase = phase;
        }
        if let Err(error) = self.persist(key).await {
            tracing::warn!(session = %key, %error, "could not persist session phase");
        }
    }

    /// Validates and sets the session's budget.
    ///
    /// A USD budget over a provider that never reported cost cannot be
    /// measured, so it is refused instead of silently comparing against zero.
    async fn set_budget(
        &self,
        request_id: u64,
        key: SessionKey,
        budget: Budget,
    ) -> Result<EventStream, ServiceError> {
        self.validate_budget(&key, &budget).await?;
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.budget = Some(budget.clone());
        }
        self.persist(&key).await?;
        if let Some(book) = self.lock_budget_states().get(&key).cloned() {
            book.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .set_session_budget(Some(budget.clone()));
        }
        Ok(single(
            request_id,
            Some(key),
            ServerEventKind::BudgetChanged {
                budget: Some(budget),
            },
        ))
    }

    /// Refuses a budget that the session's history says cannot be measured.
    async fn validate_budget(&self, key: &SessionKey, budget: &Budget) -> Result<(), ServiceError> {
        if !budget.limit.is_finite() || budget.limit <= 0.0 {
            return Err(ServiceError::Budget(
                "the limit must be a positive, finite number".to_owned(),
            ));
        }
        if budget.unit != BudgetUnit::Usd {
            return Ok(());
        }
        let model = self.model_label(key);
        let unpriced_in_session = {
            let sessions = self.lock_sessions();
            sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?
                .unpriced
        };
        let unpriced_model = self
            .directory
            .costs(Some(key.as_str()))
            .await?
            .iter()
            .filter(|record| record.model == model && record.cost_usd.is_none())
            .count();
        if unpriced_in_session > 0 || unpriced_model > 0 {
            return Err(ServiceError::Budget(format!(
                "`{model}` has requests with no reported cost; use a tokens or requests \
                 budget, or a provider that reports cost"
            )));
        }
        Ok(())
    }

    /// Blocks a switch that would leave a USD budget unmeasurable.
    async fn ensure_model_supports_budget(
        &self,
        key: &SessionKey,
        spec: &ModelSpec,
    ) -> Result<(), ServiceError> {
        let (budget, unpriced) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (record.budget.clone(), record.unpriced)
        };
        if budget
            .as_ref()
            .is_none_or(|budget| budget.unit != BudgetUnit::Usd)
        {
            return Ok(());
        }
        let model = spec.label();
        let costs = self.directory.costs(Some(key.as_str())).await?;
        let new_model_costs: Vec<_> = costs
            .into_iter()
            .filter(|record| record.model == model)
            .collect();
        let unpriced_new = new_model_costs
            .iter()
            .filter(|record| record.cost_usd.is_none())
            .count();
        if unpriced > 0 || unpriced_new > 0 {
            return Err(ServiceError::Budget(format!(
                "the session has a USD budget, but `{model}` has requests without a \
                 reported cost; set a tokens or requests budget before switching"
            )));
        }
        Ok(())
    }

    async fn set_model(&self, key: &SessionKey, spec: ModelSpec) -> Result<(), ServiceError> {
        {
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
        }
        self.persist(key).await
    }

    async fn rename(&self, key: &SessionKey, title: &str) -> Result<(), ServiceError> {
        {
            let mut lock = self.lock_sessions();
            let record = lock
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.title = Some(title.to_owned());
        }
        self.persist(key).await
    }

    async fn delete(&self, key: &SessionKey) -> Result<(), ServiceError> {
        self.lock_sessions()
            .remove(key)
            .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
        self.directory.delete_session(key.as_str()).await?;
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

    fn lock_budget_states(&self) -> MutexGuard<'_, BTreeMap<SessionKey, Arc<Mutex<BudgetBook>>>> {
        self.budget_states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Builds the budget state for a turn from the session and the day's ledger.
    async fn new_budget_book(
        &self,
        key: &SessionKey,
        message: Option<Budget>,
    ) -> Result<Arc<Mutex<BudgetBook>>, ServiceError> {
        let (session_budget, session_summary) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (
                record.budget.clone(),
                CostSummary {
                    requests: record.requests,
                    input_tokens: record.input_tokens,
                    output_tokens: record.output_tokens,
                    reported_cost_usd: record.cost,
                    not_reported: record.unpriced,
                    ..CostSummary::default()
                },
            )
        };
        let today = Utc::now().date_naive();
        let costs = self.directory.costs(None).await?;
        let global_summary = summarize(
            costs
                .iter()
                .filter(|record| record.recorded_at.date_naive() == today),
        );
        Ok(Arc::new(Mutex::new(BudgetBook::new(
            message,
            session_budget,
            configured_budget(self.config.budgets.global_daily.as_ref()),
            CostSummary::default(),
            session_summary,
            global_summary,
        ))))
    }

    fn configured_session_budget(&self) -> Option<Budget> {
        configured_budget(self.config.budgets.session.as_ref())
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

/// Bridges the question broker's durable log onto the session directory.
struct DirectoryQuestions {
    directory: Arc<dyn SessionDirectory>,
}

impl DirectoryQuestions {
    fn new(directory: Arc<dyn SessionDirectory>) -> Self {
        Self { directory }
    }
}

#[async_trait]
impl QuestionStore for DirectoryQuestions {
    async fn load(&self, call_id: &str) -> Option<DurableQuestion> {
        match self.directory.question(call_id).await {
            Ok(question) => question.map(|stored| DurableQuestion {
                id: stored.id,
                session: stored.session,
                call_id: stored.call_id,
                question: stored.question,
                options: stored.options,
                multiple: stored.multiple,
                answer: stored.answer,
            }),
            Err(error) => {
                tracing::warn!(%error, "could not read a stored question");
                None
            }
        }
    }

    async fn save(&self, question: &DurableQuestion) {
        let stored = StoredQuestion {
            id: question.id.clone(),
            session: question.session,
            call_id: question.call_id.clone(),
            question: question.question.clone(),
            options: question.options.clone(),
            multiple: question.multiple,
            answer: question.answer.clone(),
        };
        if let Err(error) = self.directory.save_question(&stored).await {
            tracing::warn!(%error, "could not store a pending question");
        }
    }

    async fn answer(&self, call_id: &str, answer: &str) -> bool {
        match self.directory.answer_question(call_id, answer).await {
            Ok(recorded) => recorded,
            Err(error) => {
                tracing::warn!(%error, "could not store a question answer");
                false
            }
        }
    }
}

/// Re-roots tool paths into the workspace and refuses an escape.
///
/// This is a gate on arguments, not a sandbox: a `command` can still reach
/// outside the workspace. It stops the common mistake of a model reading
/// `../../etc/passwd`, resolves symbolic links before comparing them with the
/// root, and refuses FIFOs, sockets and devices so a tool cannot block forever.
struct WorkspacePolicy {
    root: PathBuf,
}

/// What a path argument is expected to name when it already exists.
#[derive(Clone, Copy)]
enum PathKind {
    /// A regular file (or a path that does not exist yet, for a write).
    File,
    /// A directory (or, for a path that does not exist, a future one).
    Directory,
    /// Either a regular file or a directory, but never a FIFO, socket or device.
    FileOrDirectory,
}

impl Policy for WorkspacePolicy {
    fn name(&self) -> &'static str {
        "workspace-root"
    }

    fn decide(&self, view: &PolicyView<'_>) -> Decision {
        let tool = view.call.name.as_str();
        match tool {
            "read" | "write" | "edit" => self.apply(view, "path", false, PathKind::File),
            "glob" | "list_dir" => self.apply(view, "path", true, PathKind::Directory),
            "grep" => self.apply(view, "path", true, PathKind::FileOrDirectory),
            "command" => self.apply(view, "cwd", false, PathKind::Directory),
            _ => Decision::Allow,
        }
    }
}

impl WorkspacePolicy {
    /// Re-roots one path argument, then refuses a symlink escape or a special
    /// file. The kind check is what keeps a FIFO from blocking `read` forever.
    fn apply(
        &self,
        view: &PolicyView<'_>,
        key: &str,
        default_to_root: bool,
        kind: PathKind,
    ) -> Decision {
        let arguments = match transform_path(&view.call.arguments, key, &self.root, default_to_root)
        {
            Ok(arguments) => arguments,
            Err(reason) => return Decision::Deny(reason),
        };
        match check_path_kind(&arguments, key, kind) {
            Ok(()) => Decision::Transform(arguments),
            Err(reason) => Decision::Deny(reason),
        }
    }
}

/// Resolves `key` in `arguments` under `root`, rejecting an escape.
///
/// With `default_to_root`, a missing path becomes the root itself; otherwise a
/// missing key is left alone.
fn transform_path(
    arguments: &serde_json::Value,
    key: &str,
    root: &Path,
    default_to_root: bool,
) -> std::result::Result<serde_json::Value, String> {
    let Some(object) = arguments.as_object() else {
        return Err("tool arguments must be a JSON object".to_owned());
    };
    let mut object = object.clone();
    let Some(raw) = object.get(key) else {
        if default_to_root {
            object.insert(
                key.to_owned(),
                serde_json::Value::String(root.display().to_string()),
            );
            return Ok(serde_json::Value::Object(object));
        }
        return Ok(serde_json::Value::Object(object));
    };
    let Some(raw) = raw.as_str() else {
        return Err(format!("`{key}` must be a string path"));
    };
    let path = Path::new(raw);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let resolved = resolve_inside(root, &joined)
        .map_err(|reason| format!("`{raw}` cannot be used: {reason}"))?;
    object.insert(
        key.to_owned(),
        serde_json::Value::String(resolved.display().to_string()),
    );
    Ok(serde_json::Value::Object(object))
}

/// Resolves symlinks in the part of `path` that exists, and checks the result
/// against the canonical workspace root.
///
/// A lexical check is not enough: a repository can carry `link -> /etc`, and
/// the OS follows it after the policy has approved the argument. Canonicalizing
/// the deepest existing ancestor catches that, while leaving a not-yet-created
/// file name for `write` to create.
fn resolve_inside(root: &Path, path: &Path) -> std::result::Result<PathBuf, String> {
    let canonical =
        resolve_existing_prefix(path).map_err(|error| format!("`{}`: {error}", path.display()))?;
    if !canonical.starts_with(root) {
        return Err(format!(
            "is outside the workspace root `{}`; use a path inside it",
            root.display()
        ));
    }
    Ok(canonical)
}

/// Canonicalizes the longest existing prefix of `path` and reattaches the
/// missing tail. A dangling symlink is refused rather than followed, because
/// its target could be outside the workspace and would be created there.
fn resolve_existing_prefix(path: &Path) -> std::io::Result<PathBuf> {
    let mut cursor = path;
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(cursor) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::symlink_metadata(cursor) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(std::io::Error::other(format!(
                            "{} is a symbolic link whose target does not exist",
                            cursor.display()
                        )));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                let Some(name) = cursor.file_name() else {
                    return Err(error);
                };
                missing.push(name.to_os_string());
                let Some(parent) = cursor.parent() else {
                    return Err(error);
                };
                cursor = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Refuses a path that exists and is not the kind the tool can handle.
///
/// The check follows symlinks (the policy has already established that the
/// target is inside the workspace). Its main job is a FIFO: `read` opens it
/// synchronously, and opening a FIFO with no writer blocks forever.
fn check_path_kind(
    arguments: &serde_json::Value,
    key: &str,
    kind: PathKind,
) -> std::result::Result<(), String> {
    let Some(raw) = arguments.get(key).and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    let path = Path::new(raw);
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("`{raw}`: {error}")),
    };
    let file_type = metadata.file_type();
    let accepted = match kind {
        PathKind::File => file_type.is_file(),
        PathKind::Directory => file_type.is_dir(),
        PathKind::FileOrDirectory => file_type.is_file() || file_type.is_dir(),
    };
    if accepted {
        return Ok(());
    }
    Err(format!(
        "`{raw}` is not a {}; special files such as FIFOs, sockets and devices are refused \
         because they can block a tool indefinitely",
        match kind {
            PathKind::File => "regular file",
            PathKind::Directory => "directory",
            PathKind::FileOrDirectory => "regular file or directory",
        }
    ))
}

/// The tools whose effects are not safely repeatable after a crash.
const JOURNALED_TOOLS: [&str; 4] = ["write", "edit", "command", "skill_script"];

fn is_journaled(tool: &str) -> bool {
    JOURNALED_TOOLS.contains(&tool)
}

/// Intent/done records for effectful tools, keyed by session and call id.
///
/// Norte guarantees commit-at-most-once, not effect-at-most-once. The intent is
/// written by the engine's tool-metadata callback immediately before the
/// executor runs; a row still at `intent` when the process starts means a crash
/// may have happened mid-effect.
struct EffectJournal {
    directory: Arc<dyn SessionDirectory>,
    epoch: String,
    suspects: Mutex<BTreeSet<(SessionId, CallId)>>,
}

impl EffectJournal {
    fn new(directory: Arc<dyn SessionDirectory>) -> Self {
        Self {
            directory,
            epoch: uuid::Uuid::new_v4().to_string(),
            suspects: Mutex::new(BTreeSet::new()),
        }
    }

    /// Loads the calls a previous process started without finishing.
    async fn load(&self) -> Result<(), ServiceError> {
        let pending = self.directory.unfinished_effects().await?;
        let mut suspects = self.suspects.lock().unwrap_or_else(PoisonError::into_inner);
        for effect in pending {
            suspects.insert((effect.session, effect.call_id));
        }
        Ok(())
    }

    fn intent(&self, session: SessionId, call_id: CallId, tool: &str) {
        if !is_journaled(tool) {
            return;
        }
        if let Err(error) = self
            .directory
            .effect_started(session, call_id, tool, &self.epoch)
        {
            tracing::warn!(%error, %call_id, "could not journal a tool intent");
        }
    }

    fn finished(&self, session: SessionId, call_id: CallId, tool: &str, is_error: bool) {
        if !is_journaled(tool) {
            return;
        }
        if let Err(error) = self.directory.effect_finished(session, call_id, is_error) {
            tracing::warn!(%error, %call_id, "could not journal a tool result");
        }
    }

    fn is_suspect(&self, session: SessionId, call_id: CallId) -> bool {
        self.suspects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&(session, call_id))
    }
}

/// Refuses to replay an effectful call a crashed process may have run.
struct EffectJournalPolicy {
    journal: Arc<EffectJournal>,
}

impl Policy for EffectJournalPolicy {
    fn name(&self) -> &'static str {
        "effect-journal"
    }

    fn decide(&self, view: &PolicyView<'_>) -> Decision {
        let tool = view.call.name.as_str();
        if is_journaled(tool) && self.journal.is_suspect(view.session.id, view.call.call_id) {
            return Decision::Deny(format!(
                "`{tool}` call {} started before a restart and may already have taken effect, \
                 so it is not run again. Inspect the workspace; to retry, ask in a new turn \
                 so the model gets a fresh call.",
                view.call.call_id
            ));
        }
        Decision::Allow
    }
}

/// Global and per-session approval rules.
///
/// Global rules live in the config file; session denials are mirrored into the
/// logical session so a restart keeps them. Allows for one session use Norte's
/// grants instead, so they travel with the session's durable state.
#[derive(Debug, Default)]
struct ApprovalRules {
    global_allow: Mutex<BTreeSet<String>>,
    global_deny: Mutex<BTreeSet<String>>,
    session_deny: Mutex<BTreeMap<SessionId, BTreeSet<String>>>,
}

impl ApprovalRules {
    fn new(config: &norte_code_config::ApprovalsConfig) -> Self {
        Self {
            global_allow: Mutex::new(config.allow.iter().cloned().collect()),
            global_deny: Mutex::new(config.deny.iter().cloned().collect()),
            session_deny: Mutex::new(BTreeMap::new()),
        }
    }

    fn denies(&self, session: SessionId, tool: &str) -> bool {
        if self
            .global_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(tool)
        {
            return true;
        }
        self.session_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&session)
            .is_some_and(|tools| tools.contains(tool))
    }

    fn allows(&self, session: SessionId, tool: &str) -> bool {
        !self.denies(session, tool)
            && self
                .global_allow
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(tool)
    }

    fn allow_global(&self, tool: &str) {
        self.global_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(tool);
        self.global_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tool.to_owned());
    }

    fn deny_global(&self, tool: &str) {
        self.global_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(tool);
        self.global_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tool.to_owned());
    }

    fn deny_session(&self, session: SessionId, tool: &str) {
        self.session_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(session)
            .or_default()
            .insert(tool.to_owned());
    }

    fn load_session(&self, session: SessionId, tools: &[String]) {
        self.session_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session, tools.iter().cloned().collect());
    }
}

/// Parks the tools a human must approve, according to the session's mode.
struct ApprovalModePolicy {
    mode: ApprovalMode,
    skills: Arc<SkillRegistry>,
    honor_allowed_tools: bool,
    rules: Arc<ApprovalRules>,
}

impl Policy for ApprovalModePolicy {
    fn name(&self) -> &'static str {
        "approval-mode"
    }

    fn decide(&self, view: &PolicyView<'_>) -> Decision {
        let tool = view.call.name.as_str();
        if self.rules.denies(view.session.id, tool) {
            return Decision::Deny(format!(
                "`{tool}` is denied by the approval rules; change the rule before calling it"
            ));
        }
        if self.rules.allows(view.session.id, tool) {
            return Decision::Allow;
        }
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

/// Resolves the request's reasoning setting against the model's declaration.
///
/// An explicit setting wins over the configured default. Every setting is
/// checked before the turn starts: an unsupported effort or budget is a typed
/// error, never a silent downgrade.
/// Resolves the effective reasoning setting without turning it into a Norte
/// request.
///
/// This is what debug output shows: an explicit `--reasoning` wins over the
/// configured default, and an implicit `auto` becomes medium (or the closest
/// lower declared effort) when the model supports reasoning.
///
/// # Errors
///
/// Returns [`ServiceError::Reasoning`] when the setting is malformed, the
/// model does not support it, or a budget exceeds the model's cap.
pub fn effective_reasoning(
    requested: Option<&ProtocolReasoning>,
    configured: &serde_json::Value,
    capabilities: &ReasoningCapability,
) -> Result<ProtocolReasoning, ServiceError> {
    let setting = if let Some(setting) = requested {
        setting.clone()
    } else {
        let configured = parse_configured_reasoning(configured).map_err(ServiceError::Reasoning)?;
        // The implicit default is medium: it leaves room for a useful answer
        // without asking every model for its most expensive effort. When a
        // model does not declare medium, prefer the closest effort below it
        // rather than jumping to high. Models with no declared efforts keep
        // `auto`, which is the only safe request.
        if matches!(configured, ProtocolReasoning::Automatic) {
            implicit_effort(capabilities).map_or(configured, |effort| ProtocolReasoning::Effort {
                effort: effort_name(effort).to_owned(),
            })
        } else {
            configured
        }
    };
    validate_reasoning(&setting, capabilities)?;
    Ok(setting)
}

/// Resolves the effective setting and maps it to the engine's request type.
fn resolve_reasoning(
    requested: Option<&ProtocolReasoning>,
    configured: &serde_json::Value,
    capabilities: &ReasoningCapability,
) -> Result<ReasoningRequest, ServiceError> {
    let setting = effective_reasoning(requested, configured, capabilities)?;
    validate_reasoning(&setting, capabilities)
}

/// The effort an implicit `auto` resolves to.
///
/// Medium is the default. When the model does not declare it, the closest
/// lower effort avoids the top end; high is used only when it is the sole
/// declared effort.
fn implicit_effort(capabilities: &ReasoningCapability) -> Option<ReasoningEffort> {
    if !capabilities.supported {
        return None;
    }
    [
        ReasoningEffort::Medium,
        ReasoningEffort::Low,
        ReasoningEffort::Minimal,
        ReasoningEffort::High,
    ]
    .into_iter()
    .find(|effort| capabilities.efforts.contains(effort))
}

/// Parses `reasoning.default` from the config.
fn parse_configured_reasoning(value: &serde_json::Value) -> Result<ProtocolReasoning, String> {
    match value {
        serde_json::Value::Null => Ok(ProtocolReasoning::Automatic),
        serde_json::Value::String(text) => parse_reasoning_name(text),
        serde_json::Value::Number(number) => {
            let tokens = number
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| "a reasoning budget must be a positive integer".to_owned())?;
            Ok(ProtocolReasoning::Budget { tokens })
        }
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(kind)) = map.get("reasoning") {
                match kind.as_str() {
                    "automatic" | "auto" => return Ok(ProtocolReasoning::Automatic),
                    "disabled" | "off" => return Ok(ProtocolReasoning::Disabled),
                    "effort" => {
                        let effort = map
                            .get("effort")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| "`reasoning.effort` must be a string".to_owned())?;
                        return parse_reasoning_name(effort);
                    }
                    "budget" => {
                        let tokens = map
                            .get("tokens")
                            .and_then(serde_json::Value::as_u64)
                            .and_then(|value| u32::try_from(value).ok())
                            .filter(|value| *value > 0)
                            .ok_or_else(|| {
                                "`reasoning.tokens` must be a positive integer".to_owned()
                            })?;
                        return Ok(ProtocolReasoning::Budget { tokens });
                    }
                    other => {
                        return Err(format!(
                            "unknown reasoning setting `{other}`; use `auto`, `off`, an effort or a budget"
                        ));
                    }
                }
            }
            if let Some(tokens) = map.get("budget").or_else(|| map.get("tokens")) {
                let tokens = tokens
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or_else(|| "`reasoning.budget` must be a positive integer".to_owned())?;
                return Ok(ProtocolReasoning::Budget { tokens });
            }
            if let Some(effort) = map.get("effort").and_then(serde_json::Value::as_str) {
                return parse_reasoning_name(effort);
            }
            Err("expected `auto`, `off`, an effort object or a budget object".to_owned())
        }
        _ => Err("expected a string, a number or an object".to_owned()),
    }
}

/// Parses `auto`, `off` or one of Norte's four effort names.
fn parse_reasoning_name(text: &str) -> Result<ProtocolReasoning, String> {
    let normalized = text.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "auto" | "automatic" => Ok(ProtocolReasoning::Automatic),
        "off" | "none" | "disabled" => Ok(ProtocolReasoning::Disabled),
        _ => parse_reasoning_effort(text)
            .map(|effort| ProtocolReasoning::Effort {
                effort: effort_name(effort).to_owned(),
            })
            .ok_or_else(|| {
                format!(
                    "unknown reasoning setting `{text}`; use `auto`, `off`, `minimal`, `low`, `medium` or `high`"
                )
            }),
    }
}

/// Maps an effort name to Norte's enum. The catalogue's top-end aliases map
/// down, as they do when the catalogue is loaded.
fn parse_reasoning_effort(name: &str) -> Option<ReasoningEffort> {
    match name.trim().to_ascii_lowercase().as_str() {
        "minimal" => Some(ReasoningEffort::Minimal),
        "low" => Some(ReasoningEffort::Low),
        "medium" => Some(ReasoningEffort::Medium),
        "high" | "xhigh" | "max" => Some(ReasoningEffort::High),
        _ => None,
    }
}

/// Checks one setting against the model's declared reasoning capabilities.
fn validate_reasoning(
    setting: &ProtocolReasoning,
    capabilities: &ReasoningCapability,
) -> Result<ReasoningRequest, ServiceError> {
    match setting {
        ProtocolReasoning::Automatic => Ok(ReasoningRequest::Automatic),
        ProtocolReasoning::Disabled => {
            if capabilities.supported && !capabilities.can_disable {
                return Err(ServiceError::Reasoning(
                    "the model does not allow turning reasoning off".to_owned(),
                ));
            }
            Ok(ReasoningRequest::Disabled)
        }
        ProtocolReasoning::Effort { effort } => {
            if !capabilities.supported {
                return Err(ServiceError::Reasoning(
                    "the model does not support reasoning; omit the setting or use `auto`"
                        .to_owned(),
                ));
            }
            let Some(effort) = parse_reasoning_effort(effort) else {
                return Err(ServiceError::Reasoning(format!(
                    "unknown effort `{effort}`; the model supports {}",
                    effort_choices(capabilities)
                )));
            };
            if !capabilities.efforts.contains(&effort) {
                return Err(ServiceError::Reasoning(format!(
                    "the model does not support effort `{}`; it supports {}",
                    effort_name(effort),
                    effort_choices(capabilities)
                )));
            }
            Ok(ReasoningRequest::Effort(effort))
        }
        ProtocolReasoning::Budget { tokens } => {
            if !capabilities.supported || !capabilities.supports_budget {
                return Err(ServiceError::Reasoning(
                    "the model does not accept a reasoning token budget".to_owned(),
                ));
            }
            if *tokens == 0 {
                return Err(ServiceError::Reasoning(
                    "the reasoning budget must be at least one token".to_owned(),
                ));
            }
            if let Some(max) = capabilities.max_budget {
                if *tokens > max {
                    return Err(ServiceError::Reasoning(format!(
                        "the model accepts at most {max} reasoning tokens"
                    )));
                }
            }
            Ok(ReasoningRequest::Budget(*tokens))
        }
    }
}

/// The effort names a model accepts, for an error message.
fn effort_choices(capabilities: &ReasoningCapability) -> String {
    if capabilities.efforts.is_empty() {
        return "no explicit efforts (use `auto`)".to_owned();
    }
    capabilities
        .efforts
        .iter()
        .map(|effort| format!("`{}`", effort_name(*effort)))
        .collect::<Vec<_>>()
        .join(", ")
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

/// The text a tool returned, whether it succeeded or failed.
async fn tool_result_text(
    engine: &Engine,
    session: SessionId,
    call_id: &norte::CallId,
) -> Option<String> {
    let history = engine.history(session).await.ok()?;
    for message in history.iter().rev() {
        for part in &message.parts {
            if let norte::Part::ToolResult(result) = part {
                if result.call_id == *call_id {
                    return Some(
                        result
                            .parts
                            .iter()
                            .filter_map(norte::Part::as_text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
            }
        }
    }
    None
}

/// The most tool output the wire carries for a verbose renderer.
const TOOL_OUTPUT_LIMIT: usize = 4_000;

/// Shortens a tool result on a character boundary.
fn shorten_tool_output(text: String) -> (String, bool) {
    if text.chars().count() <= TOOL_OUTPUT_LIMIT {
        return (text, false);
    }
    let mut shortened: String = text.chars().take(TOOL_OUTPUT_LIMIT).collect();
    shortened.push_str("\n… [truncated]");
    (shortened, true)
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

/// Converts a validated config budget to the wire shape.
///
/// Config validation already rejects unknown units and actions; a programmatic
/// `Config` can still carry one, so an unusable budget is dropped rather than
/// guessed at.
fn configured_budget(budget: Option<&BudgetConfig>) -> Option<Budget> {
    let budget = budget?;
    let unit = match budget.unit.as_str() {
        "usd" => BudgetUnit::Usd,
        "tokens" => BudgetUnit::Tokens,
        "requests" => BudgetUnit::Requests,
        _ => return None,
    };
    let action = match budget.action.as_str() {
        "warn" => norte_code_protocol::BudgetAction::Warn,
        "ask" => norte_code_protocol::BudgetAction::Ask,
        "stop" => norte_code_protocol::BudgetAction::Stop,
        _ => return None,
    };
    Some(Budget {
        unit,
        limit: budget.limit,
        action,
    })
}

/// Stops a model request before it is sent when a budget is exhausted.
///
/// Norte has no price feed, so this is a check against the ledger's measured
/// spend, not a worst-case estimate. A request can still overshoot by one,
/// because the provider only reports its cost after it answers.
struct BudgetLayer {
    book: Arc<Mutex<BudgetBook>>,
}

#[async_trait]
impl Layer for BudgetLayer {
    fn name(&self) -> &'static str {
        "budget"
    }

    async fn wrap(
        &self,
        context: &LayerContext<'_>,
        request: ModelRequest,
        next: &dyn Continuation,
    ) -> norte::Result<ModelStream> {
        let stop = {
            let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
            book.stop()
        };
        if let Some(status) = stop {
            let unit = match status.unit {
                BudgetUnit::Usd => "USD",
                BudgetUnit::Tokens => "tokens",
                BudgetUnit::Requests => "requests",
            };
            let detail = format!(
                "{:?} budget: spent {:.6} of {:.6} {unit}; raise the budget and resume",
                status.scope, status.spent, status.limit
            );
            context.short_circuit(
                "budget",
                ShortCircuitReason::Custom {
                    name: "budget_exceeded".to_owned(),
                    detail: detail.clone(),
                },
            );
            return Err(norte::Error::Other(format!("budget exceeded: {detail}")));
        }
        next.call(context, request).await
    }
}

/// The arguments a pending call would run with, for showing a human what they
/// are approving.
///
/// The approval record is authoritative: when a policy rewrote the arguments,
/// it holds the rewritten object, while history still has what the model wrote.
/// Older records predate that field, so history is the fallback. Missing
/// history is not an error: the fields are optional.
async fn call_arguments(
    engine: &Engine,
    session: SessionId,
    call_id: CallId,
) -> Option<serde_json::Value> {
    if let Ok(records) = engine.pending_approvals(session).await {
        if let Some(arguments) = records
            .into_iter()
            .find(|record| record.call_id == call_id)
            .and_then(|record| record.arguments)
        {
            return Some(arguments);
        }
    }
    let history = engine.history(session).await.ok()?;
    history
        .iter()
        .flat_map(|message| message.tool_calls())
        .find(|call| call.call_id == call_id)
        .map(|call| call.arguments.clone())
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
    use norte_code_store_sqlite::SqliteStore;
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
            .unwrap_or_else(|error| panic!("request failed: {error}"))
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
    async fn two_sessions_run_turns_concurrently() {
        struct FreshFactory {
            reference: ModelRef,
        }

        impl ModelFactory for FreshFactory {
            fn build(
                &self,
                _spec: &ModelSpec,
                _capabilities: &Capabilities,
            ) -> Result<Arc<dyn Model>, ServiceError> {
                Ok(
                    Arc::new(ScriptedModel::new(self.reference.clone()).turn_text("ok"))
                        as Arc<dyn Model>,
                )
            }
        }

        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let service = Arc::new(
            Service::new(
                config(),
                catalog(),
                spec("scripted"),
                Arc::new(FreshFactory { reference }),
                Arc::new(InMemoryStore::new()),
                Arc::new(InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );

        let mut keys = Vec::new();
        for title in ["a", "b"] {
            let events = drain(
                &service,
                ClientRequestKind::CreateSession {
                    title: Some(title.to_owned()),
                },
            )
            .await;
            keys.push(events[0].session.clone().expect("session"));
        }

        let first = service
            .submit(ClientRequest {
                request_id: 1,
                kind: ClientRequestKind::SendMessage {
                    session: Some(keys[0].clone()),
                    text: "one".to_owned(),
                    budget: None,
                    reasoning: None,
                },
            })
            .await
            .expect("first turn");
        let second = service
            .submit(ClientRequest {
                request_id: 2,
                kind: ClientRequestKind::SendMessage {
                    session: Some(keys[1].clone()),
                    text: "two".to_owned(),
                    budget: None,
                    reasoning: None,
                },
            })
            .await
            .expect("second turn");

        let (first, second) = tokio::join!(first.collect::<Vec<_>>(), second.collect::<Vec<_>>());
        for events in [first, second] {
            assert!(events.iter().any(|event| matches!(
                event.kind,
                ServerEventKind::TurnFinished {
                    committed: true,
                    ..
                }
            )));
        }
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

    #[test]
    fn workspace_paths_are_rerooted_and_escapes_are_refused() {
        let root = Path::new("/tmp/norte-workspace");
        let relative = transform_path(
            &serde_json::json!({"path": "src/lib.rs", "content": "x"}),
            "path",
            root,
            false,
        )
        .expect("relative path");
        assert_eq!(
            relative.get("path").and_then(serde_json::Value::as_str),
            Some("/tmp/norte-workspace/src/lib.rs")
        );
        assert!(
            transform_path(
                &serde_json::json!({"path": "../../etc/passwd"}),
                "path",
                root,
                false
            )
            .is_err()
        );
        assert!(
            transform_path(
                &serde_json::json!({"path": "/etc/passwd"}),
                "path",
                root,
                false
            )
            .is_err()
        );
        let missing = transform_path(
            &serde_json::json!({"pattern": "**/*.rs"}),
            "path",
            root,
            true,
        )
        .expect("default path");
        assert_eq!(
            missing.get("path").and_then(serde_json::Value::as_str),
            Some("/tmp/norte-workspace")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_workspace_cannot_reach_outside_it() {
        let base = std::env::temp_dir().join(format!("norte-policy-{}", uuid::Uuid::new_v4()));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(outside.join("secret"), "do not read").expect("secret");
        let root = std::fs::canonicalize(&root).expect("canonical root");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");

        let error = transform_path(
            &serde_json::json!({"path": "link/secret"}),
            "path",
            &root,
            false,
        )
        .expect_err("a symlink escape must be refused");
        assert!(error.contains("outside the workspace root"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_is_refused_instead_of_followed() {
        let base = std::env::temp_dir().join(format!("norte-policy-{}", uuid::Uuid::new_v4()));
        let root = base.join("root");
        std::fs::create_dir_all(&root).expect("root");
        let root = std::fs::canonicalize(&root).expect("canonical root");
        std::os::unix::fs::symlink(root.join("missing"), root.join("dangling")).expect("symlink");

        let error = transform_path(
            &serde_json::json!({"path": "dangling"}),
            "path",
            &root,
            false,
        )
        .expect_err("a dangling symlink must be refused");
        assert!(error.contains("target does not exist"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_stays_inside_the_workspace_is_followed() {
        let base = std::env::temp_dir().join(format!("norte-policy-{}", uuid::Uuid::new_v4()));
        let root = base.join("root");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("target"), "inside").expect("target");
        let root = std::fs::canonicalize(&root).expect("canonical root");
        std::os::unix::fs::symlink(root.join("target"), root.join("link")).expect("symlink");

        let arguments = transform_path(&serde_json::json!({"path": "link"}), "path", &root, false)
            .expect("an inside symlink is allowed");
        assert_eq!(
            arguments.get("path").and_then(serde_json::Value::as_str),
            Some(root.join("target").to_str().expect("utf-8"))
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_before_a_tool_can_block_on_it() {
        let base = std::env::temp_dir().join(format!("norte-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).expect("base");
        let fifo = base.join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(status.success());
        let arguments = serde_json::json!({"path": fifo});
        let error = check_path_kind(&arguments, "path", PathKind::File)
            .expect_err("a FIFO must be refused");
        assert!(error.contains("not a regular file"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn reasoning_settings_are_checked_against_the_model() {
        let capabilities = ReasoningCapability {
            supported: true,
            can_disable: true,
            efforts: BTreeSet::from([
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ]),
            supports_budget: true,
            max_budget: Some(4096),
        };
        let automatic = serde_json::json!("auto");

        assert_eq!(
            resolve_reasoning(
                Some(&ProtocolReasoning::Effort {
                    effort: "medium".to_owned()
                }),
                &automatic,
                &capabilities
            )
            .expect("medium is declared"),
            ReasoningRequest::Effort(ReasoningEffort::Medium)
        );
        assert!(
            resolve_reasoning(
                Some(&ProtocolReasoning::Effort {
                    effort: "x".to_owned()
                }),
                &automatic,
                &capabilities
            )
            .is_err()
        );
        assert!(
            resolve_reasoning(
                Some(&ProtocolReasoning::Budget { tokens: 5000 }),
                &automatic,
                &capabilities
            )
            .is_err()
        );
        assert_eq!(
            resolve_reasoning(
                Some(&ProtocolReasoning::Budget { tokens: 1024 }),
                &automatic,
                &capabilities
            )
            .expect("a budget under the cap"),
            ReasoningRequest::Budget(1024)
        );
    }

    #[test]
    fn the_implicit_default_prefers_medium() {
        let capabilities = ReasoningCapability {
            supported: true,
            can_disable: false,
            efforts: BTreeSet::from([ReasoningEffort::Low, ReasoningEffort::Medium]),
            supports_budget: false,
            max_budget: None,
        };
        assert_eq!(
            resolve_reasoning(None, &serde_json::json!("auto"), &capabilities)
                .expect("medium is declared"),
            ReasoningRequest::Effort(ReasoningEffort::Medium)
        );

        let without_medium = ReasoningCapability {
            efforts: BTreeSet::from([ReasoningEffort::Low]),
            ..capabilities
        };
        assert_eq!(
            resolve_reasoning(None, &serde_json::json!("auto"), &without_medium)
                .expect("fall back to the closest lower effort"),
            ReasoningRequest::Effort(ReasoningEffort::Low)
        );

        let unsupported = ReasoningCapability {
            supported: false,
            ..without_medium
        };
        assert_eq!(
            resolve_reasoning(None, &serde_json::json!("auto"), &unsupported)
                .expect("a non-reasoning model stays automatic"),
            ReasoningRequest::Automatic
        );
    }

    #[test]
    fn configured_reasoning_accepts_efforts_and_budgets() {
        assert_eq!(
            parse_configured_reasoning(&serde_json::json!("high")).expect("effort"),
            ProtocolReasoning::Effort {
                effort: "high".to_owned()
            }
        );
        assert_eq!(
            parse_configured_reasoning(&serde_json::json!({"budget": 2048})).expect("budget"),
            ProtocolReasoning::Budget { tokens: 2048 }
        );
        assert!(parse_configured_reasoning(&serde_json::json!({"budget": 0})).is_err());
    }

    fn usage(input: u64, output: u64, cost: Option<f64>) -> Usage {
        let mut usage = Usage::default();
        usage.input_tokens = Some(input);
        usage.output_tokens = Some(output);
        usage.vendor = cost.map_or_else(BTreeMap::new, |cost| {
            BTreeMap::from([("cost".to_owned(), serde_json::json!(cost))])
        });
        usage
    }

    async fn persistent_service(model: Arc<ScriptedModel>) -> (Arc<Service>, Arc<SqliteStore>) {
        let store = Arc::new(SqliteStore::in_memory().expect("store"));
        let service = Service::with_directory(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            store.clone(),
            store.clone(),
            store.clone(),
            "system",
        )
        .await
        .expect("service");
        (Arc::new(service), store)
    }

    #[tokio::test]
    async fn the_cost_ledger_records_every_request_with_reported_or_missing_cost() {
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let model = Arc::new(
            ScriptedModel::new(reference)
                .turn(vec![
                    norte::StreamEvent::Usage(usage(100, 20, Some(0.25))),
                    norte::StreamEvent::TextDelta {
                        index: 0,
                        text: "first".to_owned(),
                    },
                    norte::StreamEvent::Finish(FinishReason::Stop),
                ])
                .turn(vec![
                    norte::StreamEvent::Usage(usage(50, 10, None)),
                    norte::StreamEvent::TextDelta {
                        index: 0,
                        text: "second".to_owned(),
                    },
                    norte::StreamEvent::Finish(FinishReason::Stop),
                ]),
        );
        let (service, store) = persistent_service(model).await;
        let first = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "one".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        let key = first[0].session.clone().expect("session");
        let second = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: Some(key.clone()),
                text: "two".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        assert!(second.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::RequestReconstructed { cost_usd: None, .. }
        )));
        let costs = store.costs(Some(key.as_str())).await.expect("costs");
        assert_eq!(costs.len(), 2);
        assert_eq!(costs[0].index, 1);
        assert_eq!(costs[0].input_tokens, Some(100));
        assert_eq!(costs[0].cost_usd, Some(0.25));
        assert!(costs[1].cost_usd.is_none());
        assert_eq!(costs[1].index, 2);
    }

    #[tokio::test]
    async fn a_session_budget_stops_the_next_request() {
        let mut config = config();
        config.budgets.session = Some(BudgetConfig {
            unit: "requests".to_owned(),
            limit: 1.0,
            action: "stop".to_owned(),
        });
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let call = CallId::new();
        let model = Arc::new(
            ScriptedModel::new(reference)
                .turn_tool_call(call, "todo", &serde_json::json!({"todos": []}))
                .turn_text("done"),
        );
        let store = Arc::new(SqliteStore::in_memory().expect("store"));
        let service = Arc::new(
            Service::with_directory(
                config,
                catalog(),
                spec("scripted"),
                Arc::new(ScriptedFactory { model }),
                store.clone(),
                store.clone(),
                store.clone(),
                "system",
            )
            .await
            .expect("service"),
        );
        let events = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "work".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::BudgetExceeded {
                scope: norte_code_protocol::BudgetScope::Session,
                action: norte_code_protocol::BudgetAction::Stop,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                committed: false,
                ..
            }
        )));
        let key = events[0].session.clone().expect("session");
        let changed = drain(
            &service,
            ClientRequestKind::SetBudget {
                session: key.clone(),
                budget: Budget {
                    unit: BudgetUnit::Requests,
                    limit: 10.0,
                    action: norte_code_protocol::BudgetAction::Stop,
                },
            },
        )
        .await;
        assert!(changed.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::BudgetChanged { budget: Some(_) }
        )));
        let resumed = drain(
            &service,
            ClientRequestKind::Resume {
                session: key.clone(),
            },
        )
        .await;
        assert!(resumed.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                committed: true,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn vendor_extensions_reach_the_model_request() {
        let mut config = config();
        config.extensions.insert(
            "openai.chat:provider.order".to_owned(),
            serde_json::json!(["deepinfra"]),
        );
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
        let seen = Arc::clone(&model);
        let service = Arc::new(
            Service::new(
                config,
                catalog(),
                spec("scripted"),
                Arc::new(ScriptedFactory { model }),
                Arc::new(InMemoryStore::new()),
                Arc::new(InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "hello".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        let requests = seen.requests();
        assert_eq!(
            requests[0].extensions["openai.chat:provider.order"],
            serde_json::json!(["deepinfra"])
        );
    }

    #[tokio::test]
    async fn a_usd_budget_is_refused_when_the_provider_does_not_report_cost() {
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let model = Arc::new(ScriptedModel::new(reference).turn(vec![
            norte::StreamEvent::Usage(usage(10, 5, None)),
            norte::StreamEvent::TextDelta {
                index: 0,
                text: "again".to_owned(),
            },
            norte::StreamEvent::Finish(FinishReason::Stop),
        ]));
        let (service, _) = persistent_service(model).await;
        let events = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "one".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        let key = events[0].session.clone().expect("session");
        let result = service
            .submit(ClientRequest {
                request_id: 8,
                kind: ClientRequestKind::SetBudget {
                    session: key,
                    budget: Budget {
                        unit: BudgetUnit::Usd,
                        limit: 1.0,
                        action: norte_code_protocol::BudgetAction::Stop,
                    },
                },
            })
            .await;
        let Err(error) = result else {
            panic!("the unmeasurable budget should have been refused");
        };
        assert!(error.to_string().contains("no reported cost"));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn a_write_parks_for_approval_and_resumes() {
        let target = std::env::current_dir()
            .expect("working directory")
            .join("target")
            .join(format!("norte-code-write-{}.txt", std::process::id()));
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
        let (approval, arguments) = first
            .iter()
            .find_map(|event| match &event.kind {
                ServerEventKind::ApprovalRequested {
                    approval,
                    arguments,
                    ..
                } => Some((approval.clone(), arguments.clone())),
                _ => None,
            })
            .expect("approval request");
        assert_eq!(
            arguments.expect("approval arguments")["path"],
            target.display().to_string()
        );
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
        let started = second
            .iter()
            .find_map(|event| match &event.kind {
                ServerEventKind::ToolStarted {
                    tool, arguments, ..
                } if tool == "write" => arguments.clone(),
                _ => None,
            })
            .expect("tool start arguments");
        assert_eq!(started["path"], target.display().to_string());
        let finished = second
            .iter()
            .find_map(|event| match &event.kind {
                ServerEventKind::ToolFinished {
                    tool,
                    is_error: false,
                    output,
                    ..
                } if tool == "write" => output.clone(),
                _ => None,
            })
            .expect("tool output");
        assert!(
            finished.contains("wrote") && finished.contains(&target.display().to_string()),
            "unexpected write output: {finished}"
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("written file"),
            "written by a tool"
        );
        let _ = std::fs::remove_file(&target);
    }

    fn write_model(call: CallId, target: &std::path::Path) -> Arc<ScriptedModel> {
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        Arc::new(
            ScriptedModel::new(reference)
                .turn_tool_call(
                    call,
                    "write",
                    &serde_json::json!({
                        "path": target.display().to_string(),
                        "content": "rules",
                        "overwrite": true
                    }),
                )
                .turn_text("done"),
        )
    }

    #[tokio::test]
    async fn a_global_allow_skips_the_approval_prompt() {
        let target = std::env::current_dir()
            .expect("working directory")
            .join("target")
            .join(format!(
                "norte-code-global-allow-{}.txt",
                std::process::id()
            ));
        let _ = std::fs::remove_file(&target);
        let mut config = config();
        config.approvals.allow = vec!["write".to_owned()];
        let service = Arc::new(
            Service::new(
                config,
                catalog(),
                spec("scripted"),
                Arc::new(ScriptedFactory {
                    model: write_model(CallId::new(), &target),
                }),
                Arc::new(InMemoryStore::new()),
                Arc::new(InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let events = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "write it".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, ServerEventKind::ApprovalRequested { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::ToolFinished {
                ref tool,
                is_error: false,
                ..
            } if tool == "write"
        )));
        assert_eq!(
            std::fs::read_to_string(&target).expect("written file"),
            "rules"
        );
        let _ = std::fs::remove_file(&target);
    }

    #[tokio::test]
    async fn a_global_deny_refuses_without_asking() {
        let target = std::env::current_dir()
            .expect("working directory")
            .join("target")
            .join(format!("norte-code-global-deny-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&target);
        let mut config = config();
        config.approvals.deny = vec!["write".to_owned()];
        let service = Arc::new(
            Service::new(
                config,
                catalog(),
                spec("scripted"),
                Arc::new(ScriptedFactory {
                    model: write_model(CallId::new(), &target),
                }),
                Arc::new(InMemoryStore::new()),
                Arc::new(InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );
        let events = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "write it".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, ServerEventKind::ApprovalRequested { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::ToolFinished {
                ref tool,
                is_error: true,
                ..
            } if tool == "write"
        )));
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn a_global_rule_is_persisted_and_applied() {
        let path = std::env::temp_dir().join(format!(
            "norte-code-approval-rules-{}-{}.json",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_file(&path);
        let service = service();
        service.set_config_path(Some(path.clone()));
        let session = SessionId::new();

        service
            .remember_global_rule("write", false)
            .expect("deny rule");
        assert!(service.approval_rules.denies(session, "write"));
        let loaded = Config::load(Some(&path), None).expect("config");
        assert_eq!(loaded.config.approvals.deny, ["write"]);

        service
            .remember_global_rule("write", true)
            .expect("allow rule");
        assert!(!service.approval_rules.denies(session, "write"));
        assert!(service.approval_rules.allows(session, "write"));
        let loaded = Config::load(Some(&path), None).expect("config");
        assert_eq!(loaded.config.approvals.allow, ["write"]);
        assert!(loaded.config.approvals.deny.is_empty());
        let _ = std::fs::remove_file(&path);
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
    async fn ask_user_parks_the_turn_until_the_client_answers() {
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let call = CallId::new();
        let model = Arc::new(
            ScriptedModel::new(reference)
                .turn_tool_call(
                    call,
                    "ask_user",
                    &serde_json::json!({
                        "question": "Which file should I edit?",
                        "options": ["src/lib.rs", "src/main.rs"],
                        "multiple": false
                    }),
                )
                .turn_text("I will wait for the answer."),
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
        let request = ClientRequest {
            request_id: 7,
            kind: ClientRequestKind::SendMessage {
                session: None,
                text: "do the work".to_owned(),
                budget: None,
                reasoning: None,
            },
        };
        let mut stream = service.submit(request).await.expect("turn");
        let mut session = None;
        let mut asked = None;
        let mut finished = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("turn should not hang")
        {
            session = session.or(event.session.clone());
            if let ServerEventKind::QuestionAsked {
                question,
                prompt,
                options,
                multiple,
                ..
            } = &event.kind
            {
                asked = Some(question.clone());
                assert_eq!(prompt, "Which file should I edit?");
                assert_eq!(options, &["src/lib.rs", "src/main.rs"]);
                assert!(!multiple);
            }
            if matches!(event.kind, ServerEventKind::TurnFinished { .. }) {
                finished = true;
                break;
            }
            if let Some(question) = asked.clone() {
                let response = ClientRequest {
                    request_id: 8,
                    kind: ClientRequestKind::AnswerQuestion {
                        session: session.clone().expect("session"),
                        question,
                        answer: "src/lib.rs".to_owned(),
                    },
                };
                let mut ack = service.submit(response).await.expect("answer");
                while ack.next().await.is_some() {}
                asked = None;
            }
        }
        assert!(finished, "the turn should finish after the answer");
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

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn a_parked_approval_survives_a_restart() {
        let directory = std::env::temp_dir().join(format!(
            "norte-code-approval-restart-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).expect("directory");
        let path = directory.join("sessions.db");
        let target = std::env::current_dir()
            .expect("working directory")
            .join("target")
            .join(format!(
                "norte-code-restart-{}-{}.txt",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
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
                        "content": "survived",
                        "overwrite": true
                    }),
                )
                .turn_text("done"),
        );

        let (store, first) = sqlite_service(&path, Arc::clone(&model)).await;
        let events = drain(
            &first,
            ClientRequestKind::SendMessage {
                session: None,
                text: "write it".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        let session = events[0].session.clone().expect("session");
        let approval = events
            .iter()
            .find_map(|event| match &event.kind {
                ServerEventKind::ApprovalRequested { approval, .. } => Some(approval.clone()),
                _ => None,
            })
            .expect("approval request");
        assert!(events.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::TurnFinished {
                phase: norte_code_protocol::Phase::AwaitingApproval,
                ..
            }
        )));
        drop(first);
        drop(store);

        let (store, second) = sqlite_service(&path, model).await;
        let listed = drain(&second, ClientRequestKind::ListSessions).await;
        let ServerEventKind::SessionList { sessions } = &listed[0].kind else {
            panic!("expected a list");
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session, session);
        assert_eq!(
            sessions[0].phase,
            norte_code_protocol::Phase::AwaitingApproval
        );

        let resumed = drain(
            &second,
            ClientRequestKind::Resume {
                session: session.clone(),
            },
        )
        .await;
        assert!(resumed.iter().any(|event| matches!(
            &event.kind,
            ServerEventKind::ApprovalRequested { approval: pending, .. } if pending == &approval
        )));

        let approved = drain(
            &second,
            ClientRequestKind::Approve {
                session,
                approval,
                arguments: None,
                scope: None,
                remember: None,
            },
        )
        .await;
        assert!(approved.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::ToolFinished {
                ref tool,
                is_error: false,
                ..
            } if tool == "write"
        )));
        assert_eq!(
            std::fs::read_to_string(&target).expect("written file"),
            "survived"
        );
        drop(second);
        drop(store);
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn a_write_that_started_before_a_crash_is_not_repeated() {
        let directory = std::env::temp_dir().join(format!(
            "norte-code-effect-restart-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).expect("directory");
        let path = directory.join("sessions.db");
        let target = std::env::current_dir()
            .expect("working directory")
            .join("target")
            .join(format!(
                "norte-code-effect-{}-{}.txt",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
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
                        "content": "should not appear",
                        "overwrite": true
                    }),
                )
                .turn_text("done"),
        );

        let (store, first) = sqlite_service(&path, Arc::clone(&model)).await;
        let events = drain(
            &first,
            ClientRequestKind::SendMessage {
                session: None,
                text: "write it".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;
        let session = events[0].session.clone().expect("session");
        let approval = events
            .iter()
            .find_map(|event| match &event.kind {
                ServerEventKind::ApprovalRequested { approval, .. } => Some(approval.clone()),
                _ => None,
            })
            .expect("approval request");
        let north = store
            .sessions()
            .await
            .expect("stored sessions")
            .first()
            .expect("one session")
            .north;
        // Pretend the previous process died after the effect but before the
        // result was recorded.
        store
            .effect_started(north, call, "write", "crashed-process")
            .expect("intent");
        drop(first);
        drop(store);

        let (store, second) = sqlite_service(&path, model).await;
        let approved = drain(
            &second,
            ClientRequestKind::Approve {
                session,
                approval,
                arguments: None,
                scope: None,
                remember: None,
            },
        )
        .await;
        assert!(approved.iter().any(|event| matches!(
            event.kind,
            ServerEventKind::ToolFinished {
                ref tool,
                is_error: true,
                ..
            } if tool == "write"
        )));
        assert!(
            !target.exists(),
            "a write that may have run before the crash must not run again"
        );
        assert!(
            store
                .unfinished_effects()
                .await
                .expect("journal")
                .is_empty(),
            "the refusal is recorded as a finished effect"
        );
        drop(second);
        drop(store);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn a_pending_question_survives_a_restart() {
        let directory = std::env::temp_dir().join(format!(
            "norte-code-question-restart-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).expect("directory");
        let path = directory.join("sessions.db");
        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let call = CallId::new();
        let model = Arc::new(
            ScriptedModel::new(reference)
                .turn_tool_call(
                    call,
                    "ask_user",
                    &serde_json::json!({
                        "question": "Which file?",
                        "options": ["src/lib.rs", "src/main.rs"],
                        "multiple": true
                    }),
                )
                .turn_text("done"),
        );

        let (store, first) = sqlite_service(&path, Arc::clone(&model)).await;
        let request = ClientRequest {
            request_id: 7,
            kind: ClientRequestKind::SendMessage {
                session: None,
                text: "go".to_owned(),
                budget: None,
                reasoning: None,
            },
        };
        let mut stream = first.submit(request).await.expect("turn");
        let mut session = None;
        let mut asked = None;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("the turn should park")
        {
            session = session.or(event.session.clone());
            if let ServerEventKind::QuestionAsked { question, .. } = &event.kind {
                asked = Some(question.clone());
                break;
            }
        }
        let session = session.expect("session");
        let asked = asked.expect("question");
        drop(stream);
        drop(first);
        drop(store);

        let (store, second) = sqlite_service(&path, model).await;
        let mut resumed = second
            .submit(ClientRequest {
                request_id: 8,
                kind: ClientRequestKind::Resume {
                    session: session.clone(),
                },
            })
            .await
            .expect("resume");

        let mut reasked = None;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(5), resumed.next())
            .await
            .expect("the resume should re-ask")
        {
            if let ServerEventKind::QuestionAsked {
                question,
                options,
                multiple,
                ..
            } = &event.kind
            {
                assert_eq!(options, &["src/lib.rs", "src/main.rs"]);
                assert!(multiple);
                reasked = Some(question.clone());
                break;
            }
        }
        assert_eq!(reasked.as_deref(), Some(asked.as_str()));

        let answered = drain(
            &second,
            ClientRequestKind::AnswerQuestion {
                session: session.clone(),
                question: asked,
                answer: "src/lib.rs".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            answered[0].kind,
            ServerEventKind::QuestionAnswered { .. }
        ));

        let mut finished = false;
        while let Some(event) = tokio::time::timeout(Duration::from_secs(5), resumed.next())
            .await
            .expect("the resume should finish")
        {
            if matches!(event.kind, ServerEventKind::TurnFinished { .. }) {
                finished = true;
                break;
            }
        }
        assert!(finished, "the resumed turn should finish after the answer");
        drop(second);
        drop(store);
        let _ = std::fs::remove_dir_all(directory);
    }

    async fn sqlite_service(
        path: &std::path::Path,
        model: Arc<ScriptedModel>,
    ) -> (Arc<SqliteStore>, Arc<Service>) {
        let store = Arc::new(SqliteStore::open(path).expect("open"));
        let session_store: Arc<dyn Store> = store.clone();
        let blobs: Arc<dyn BlobStore> = store.clone();
        let directory: Arc<dyn SessionDirectory> = store.clone();
        let service = Service::with_directory(
            config(),
            catalog(),
            spec("scripted"),
            Arc::new(ScriptedFactory { model }),
            session_store,
            blobs,
            directory,
            "system",
        )
        .await
        .expect("service");
        (store, Arc::new(service))
    }

    /// A symlink inside the workspace must not become a way out of it, even
    /// when the tool call goes through the whole engine.
    ///
    /// The unit tests above exercise the policy directly. This one drives a
    /// real `read` call through the engine, so the denial is proven on the
    /// path a model actually takes.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_through_an_escaping_symlink_is_denied_by_the_engine() {
        let base = std::env::temp_dir().join(format!(
            "norte-escape-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(outside.join("secret"), "do not read").expect("secret");

        // The service takes its workspace root from the current directory, so
        // the symlink has to live in it.
        let root = std::env::current_dir().expect("working directory");
        let link = root.join(format!("norte-escape-link-{}.tmp", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");

        let reference = ModelRef::new(
            ProviderId::new("test").expect("provider"),
            ModelId::new("scripted").expect("model"),
        );
        let call = CallId::new();
        let model = Arc::new(
            ScriptedModel::new(reference)
                .turn_tool_call(
                    call,
                    "read",
                    &serde_json::json!({ "path": link.join("secret").display().to_string() }),
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

        let events = drain(
            &service,
            ClientRequestKind::SendMessage {
                session: None,
                text: "read it".to_owned(),
                budget: None,
                reasoning: None,
            },
        )
        .await;

        let finished = events
            .iter()
            .find_map(|event| match &event.kind {
                ServerEventKind::ToolFinished { error, .. } => Some(error.clone()),
                _ => None,
            })
            .expect("the read should have been attempted and failed");
        let message = finished.unwrap_or_default();
        assert!(
            message.contains("outside the workspace root"),
            "the escape must be refused, not read: {message}"
        );

        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&base);
    }
}
