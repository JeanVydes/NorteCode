//! The in-process service: it owns the Norte engines, maps the client protocol
//! onto turns, and streams the engine's events back as [`ServerEvent`]s.
//!
//! An engine is cheap and shares the store and blob store, so each turn builds
//! one for the model the session selected. That is what makes a mid-session
//! model switch a change of model rather than a change of session.

mod approvals;
mod compaction;
mod effects;
mod mapping;
mod models;
mod prompt;
mod questions;
mod sandbox;
mod skills;
mod tooling;
mod workspace;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_core::Stream;
use futures_util::StreamExt;
use norte::{
    ApprovalId, BlobStore, Engine, EngineBuilder, EngineConfig, EngineEvent, FinishReason, GrantId,
    GrantScope, InMemoryBlobStore, InMemoryStore, Layer, LayerStack, Message, Model, PolicySet,
    ReasoningRequest, Role, SessionId, Store, ToolArgs, ToolDeclaration, ToolRegistry, Usage,
};
use norte_code_cost::{BudgetBook, BudgetNotice, CostRecord, CostSummary, summarize};
use norte_code_skills::{
    SkillRegistry, SkillResourceTool, SkillScriptConfig, SkillScriptTool, SkillTool,
};
use norte_code_store_sqlite::{InMemoryDirectory, SessionDirectory, StoredSession};
use norte_code_tools::{
    AskUserTool, GlobTool, GrepTool, ListDirTool, QuestionBroker, TodoState, TodoTool, WebFetchTool,
};
use norte_tools::{CommandTool, EditTool, ReadTool, WriteTool};
use tokio::sync::{Notify, mpsc};

use norte_code_catalog::{Catalog, LoadOptions, load_catalog};
use norte_code_config::Config;
use norte_code_protocol::{
    Budget, BudgetUnit, ErrorCode, ModelInfo, Reasoning as ProtocolReasoning, ServerEvent,
    ServerEventKind, SessionInfo, SessionKey,
};

use crate::ServiceError;
use crate::provider::{ModelFactory, ModelSpec};
use crate::telemetry::{Telemetry, TelemetryLayer, TelemetryObserver};
use approvals::{ApprovalModePolicy, ApprovalRules};
use compaction::{
    COMPACTION_SAFE_POINT, CompactionGate, CompactionLayer, SUMMARY_PROMPT, compaction_reasoning,
    is_compaction_abort, is_compaction_message, recent_start, used_context,
};
use effects::{EffectJournal, EffectJournalPolicy};
use mapping::{
    BudgetLayer, call_arguments, configured_budget, emit, error_code, reported_cost,
    request_metrics, retry_plan, shorten_tool_output, single, to_delta, to_phase, to_usage,
    tool_error, tool_result_text,
};
use models::resolve_reasoning;
use prompt::{compose_system_prompt, load_project_instructions};
use questions::DirectoryQuestions;
use sandbox::{sandbox_name, sandboxed_command};
use skills::{collect_secrets, discover_skills};
use tooling::{ToolActivity, output_text, recording_tool};
use workspace::WorkspacePolicy;

pub use models::{effective_reasoning, model_list};
pub use prompt::PromptSize;

#[cfg(test)]
use mapping::token_rate;
#[cfg(test)]
use models::parse_configured_reasoning;
#[cfg(test)]
use prompt::{MAX_PROJECT_INSTRUCTIONS_BYTES, PROJECT_INSTRUCTIONS_FILE};
#[cfg(test)]
use sandbox::{macos_profile, sandbox_request, sandboxed_arguments};
#[cfg(test)]
use workspace::{PathKind, check_path_kind, transform_path};

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
    /// Tools this session allows without asking again.
    allowed_tools: Vec<String>,
    /// The Norte session this one was compacted from, when it was.
    compacted_from: Option<SessionId>,
    /// Whether a compaction is running, so a concurrent turn does not repeat it.
    compacting: bool,
    /// The history length written by the last compaction. A session that has
    /// not grown past it has nothing new to summarise.
    compaction_baseline: usize,
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
    project_path: Option<PathBuf>,
    project_chars: usize,
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
    telemetry: Arc<Telemetry>,
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
        let project = load_project_instructions(&workspace_root);
        let project_path = project.as_ref().map(|project| project.path.clone());
        let project_chars = project
            .as_ref()
            .map_or(0, |project| project.text.chars().count());
        let system_prompt = compose_system_prompt(system_prompt.into(), project.as_ref());
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
            system_prompt,
            project_path,
            project_chars,
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
            telemetry: Arc::new(Telemetry::default()),
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
            self.approval_rules.load_session(
                entry.north,
                &entry.denied_tools,
                &entry.allowed_tools,
            );
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
                allowed_tools: entry.allowed_tools.clone(),
                compacted_from: entry.compacted_from,
                compacting: false,
                compaction_baseline: stats
                    .get("compaction_baseline")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(0),
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
                "compaction_baseline": record.compaction_baseline,
            }),
            denied_tools: record.denied_tools.clone(),
            allowed_tools: record.allowed_tools.clone(),
            compacted_from: record.compacted_from,
        });
        if let Some(stored) = stored {
            self.directory.save_session(&stored).await?;
        }
        Ok(())
    }

    /// Builds an engine for one spec over the shared stores.
    fn engine(&self, spec: &ModelSpec) -> Result<Engine, ServiceError> {
        self.engine_with_budget(spec, None, None)
    }

    /// Builds an engine whose requests pass through the budget and telemetry
    /// layers, plus an optional eager-compaction layer.
    fn engine_with_budget(
        &self,
        spec: &ModelSpec,
        budget: Option<Arc<Mutex<BudgetBook>>>,
        compaction: Option<Arc<CompactionGate>>,
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
            })
            .observers(vec![
                Arc::new(TelemetryObserver::new(Arc::clone(&self.telemetry)))
                    as Arc<dyn norte::Observer>,
            ]);
        let mut layers: Vec<Arc<dyn Layer>> = Vec::new();
        if let Some(book) = budget {
            layers.push(Arc::new(BudgetLayer { book }));
        }
        if let Some(gate) = compaction {
            layers.push(Arc::new(CompactionLayer { gate }));
        }
        layers.push(Arc::new(TelemetryLayer::new(
            Arc::clone(&self.telemetry),
            spec.clone(),
            self.config.tools.sandbox.enabled,
            self.config.tools.sandbox.network,
        )));
        builder = builder.layers(LayerStack::new(layers));
        Ok(builder.build()?)
    }

    /// The eager-compaction gate, when enabled and the model declares a context
    /// window. Without a window there is no threshold to compare against.
    fn compaction_gate(&self, spec: &ModelSpec) -> Option<Arc<CompactionGate>> {
        if !self.config.compaction.enabled || self.config.compaction.trigger != "eager" {
            return None;
        }
        let max_context = spec.capabilities.max_context?;
        Some(Arc::new(CompactionGate::new(
            max_context,
            self.config.compaction.threshold,
            self.config.compaction.keep_recent,
        )))
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
            project_chars: self.project_chars,
            project_path: self.project_path.clone(),
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
        let command = if self.config.tools.sandbox.enabled {
            sandboxed_command(command, &self.workspace_root, &self.config.tools.sandbox)
        } else {
            command
        };
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

    /// Re-scans the skill roots and returns how many valid skills remain.
    ///
    /// The next engine build registers the refreshed tools; a turn already in
    /// flight keeps the registry it started with.
    pub fn refresh_skills(&self) -> usize {
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
        count
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
            Kind::Compact { session } => self.compact_event_stream(request_id, session).await,
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
        let count = self.refresh_skills();
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
            allowed_tools: Vec::new(),
            compacted_from: None,
            compacting: false,
            compaction_baseline: 0,
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
        Ok(key)
    }

    /// Checks whether the session has crossed the automatic compaction
    /// threshold at a turn boundary. Eager mode uses [`CompactionGate`] to make
    /// the same decision before a model request inside a turn.
    #[allow(clippy::cast_precision_loss)]
    async fn compaction_due(
        &self,
        key: &SessionKey,
        spec: &ModelSpec,
        nid: SessionId,
    ) -> Result<bool, ServiceError> {
        if !self.config.compaction.enabled {
            return Ok(false);
        }
        if self
            .lock_sessions()
            .get(key)
            .is_none_or(|record| record.compacting)
        {
            return Ok(false);
        }
        let Some(max_context) = spec.capabilities.max_context else {
            return Ok(false);
        };
        let engine = self.engine(spec)?;
        let state = engine.session(nid).await?;
        if state.phase != norte::Phase::Idle {
            return Ok(false);
        }
        let history = engine.history(nid).await?;
        let baseline = self
            .lock_sessions()
            .get(key)
            .map_or(0, |record| record.compaction_baseline);
        if history.len() <= baseline
            || history.len() <= self.config.compaction.keep_recent.saturating_add(1)
        {
            return Ok(false);
        }
        let used = used_context(&history);
        Ok(used as f64 >= max_context as f64 * self.config.compaction.threshold)
    }

    /// Runs one compaction and reports its events, including a failure.
    async fn perform_compaction(
        &self,
        request_id: u64,
        key: &SessionKey,
        reason: &str,
    ) -> Result<Vec<ServerEvent>, ServiceError> {
        if !self.begin_compaction(key) {
            return Err(ServiceError::Compaction(
                "another compaction is already running".to_owned(),
            ));
        }
        let result = self.compact_core(key).await;
        self.end_compaction(key);
        let mut events = vec![ServerEvent::new(
            request_id,
            Some(key.clone()),
            ServerEventKind::CompactionStarted {
                reason: reason.to_owned(),
            },
        )];
        match result {
            Ok((_old, new)) => events.push(ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::CompactionFinished {
                    compacted: true,
                    north_session: Some(new.to_string()),
                    error: None,
                },
            )),
            Err(error) => {
                events.push(ServerEvent::new(
                    request_id,
                    Some(key.clone()),
                    ServerEventKind::CompactionFinished {
                        compacted: false,
                        north_session: None,
                        error: Some(error.to_string()),
                    },
                ));
            }
        }
        Ok(events)
    }

    fn begin_compaction(&self, key: &SessionKey) -> bool {
        let mut sessions = self.lock_sessions();
        let Some(record) = sessions.get_mut(key) else {
            return false;
        };
        if record.compacting {
            return false;
        }
        record.compacting = true;
        true
    }

    fn end_compaction(&self, key: &SessionKey) {
        if let Some(record) = self.lock_sessions().get_mut(key) {
            record.compacting = false;
        }
    }

    /// The client-facing manual compaction, as a stream of events.
    async fn compact_event_stream(
        &self,
        request_id: u64,
        key: SessionKey,
    ) -> Result<EventStream, ServiceError> {
        self.require_record(&key)?;
        let events = self.perform_compaction(request_id, &key, "manual").await?;
        Ok(Box::pin(futures_util::stream::iter(events)))
    }

    /// The algorithm: summarise with the old transcript as the prefix, then
    /// seed a fresh Norte session with the summary, the todo list and the last
    /// `keep_recent` messages. The old session is never modified.
    async fn compact_core(&self, key: &SessionKey) -> Result<(SessionId, SessionId), ServiceError> {
        let (old, spec, _) = self.require_record(key)?;
        let engine = self.engine(&spec)?;
        let state = engine.session(old).await?;
        if state.phase != norte::Phase::Idle {
            return Err(ServiceError::Compaction(
                "the Norte session is not idle; resolve the turn first".to_owned(),
            ));
        }
        let history = engine.history(old).await?;
        let conversation: Vec<Message> = history
            .iter()
            .filter(|message| message.role != Role::System)
            .cloned()
            .collect();
        if conversation.is_empty() {
            return Err(ServiceError::Compaction(
                "there is no conversation to compact yet".to_owned(),
            ));
        }
        let summary_spec = self.summary_spec(&spec)?;
        let book = self.budget_book(key).await?;
        let summary_engine = self.engine_with_budget(&summary_spec, Some(book), None)?;
        let scratch = summary_engine.create().await?;
        let mut request_messages = history.clone();
        request_messages.push(Message::user(SUMMARY_PROMPT));
        summary_engine
            .append_history(scratch.id, request_messages)
            .await?;
        let reasoning = compaction_reasoning(
            &summary_spec.capabilities.reasoning,
            &self.config.compaction.reasoning,
        );
        let outcome = summary_engine
            .turn(scratch.id)
            .reasoning(reasoning)
            .tools(std::iter::empty::<norte::ToolName>())
            .start()
            .finish()
            .await?;
        let message = outcome.message.ok_or_else(|| {
            ServiceError::Compaction("the summary request committed no message".to_owned())
        })?;
        let summary = message.text();
        if summary.trim().is_empty() {
            return Err(ServiceError::Compaction(
                "the model returned an empty summary".to_owned(),
            ));
        }
        self.record_summary_usage(key, scratch.id, state.turn, &message)
            .await?;

        let start = recent_start(&conversation, self.config.compaction.keep_recent);
        let recent = conversation[start..].to_vec();
        let fresh = engine.create().await?;
        let mut seed = vec![Message::system(self.system_prompt.clone())];
        seed.push(Message::user(format!(
            "Summary of the earlier conversation, produced by Norte Code. Treat it as \
             context, not as a new request:\n\n{summary}"
        )));
        let todos = self.todo.text(Some(old));
        if !todos.starts_with("(the task list is empty)") {
            seed.push(Message::user(format!("Current task list:\n{todos}")));
        }
        seed.extend(recent);
        engine.append_history(fresh.id, seed).await?;
        let seeded = engine.history(fresh.id).await?.len();
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.id = fresh.id;
            record.compacted_from = Some(old);
            record.billed = seeded;
            record.compaction_baseline = seeded;
            record.phase = norte_code_protocol::Phase::Idle;
            self.approval_rules
                .load_session(fresh.id, &record.denied_tools, &record.allowed_tools);
        }
        self.todo.carry(old, fresh.id);
        self.persist(key).await?;
        Ok((old, fresh.id))
    }

    fn summary_spec(&self, session: &ModelSpec) -> Result<ModelSpec, ServiceError> {
        match &self.config.compaction.model {
            Some(reference) => {
                ModelSpec::from_catalog(&self.config, &self.catalog(), reference, None)
            }
            None => Ok(session.clone()),
        }
    }

    async fn budget_book(
        &self,
        key: &SessionKey,
    ) -> Result<Arc<Mutex<norte_code_cost::BudgetBook>>, ServiceError> {
        if let Some(book) = self.lock_budget_states().get(key).cloned() {
            return Ok(book);
        }
        let message = configured_budget(self.config.budgets.message.as_ref());
        let book = self.new_budget_book(key, message).await?;
        self.lock_budget_states()
            .insert(key.clone(), Arc::clone(&book));
        Ok(book)
    }

    async fn record_summary_usage(
        &self,
        key: &SessionKey,
        scratch: SessionId,
        turn: u64,
        message: &Message,
    ) -> Result<(), ServiceError> {
        let Some(origin) = &message.origin else {
            return Ok(());
        };
        for record in self.telemetry_for(key, scratch) {
            self.record_request(key, &record).await;
        }
        let cost = reported_cost(&origin.usage);
        let index = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.requests += 1;
            record.cost += cost.unwrap_or(0.0);
            if cost.is_none() {
                record.unpriced += 1;
            }
            record.input_tokens += origin.usage.input_tokens.unwrap_or(0);
            record.output_tokens += origin.usage.output_tokens.unwrap_or(0);
            record.requests
        };
        let ledger = CostRecord {
            session: key.to_string(),
            turn,
            index,
            model: origin.model.to_string(),
            input_tokens: origin.usage.input_tokens,
            output_tokens: origin.usage.output_tokens,
            reasoning_tokens: origin.usage.reasoning_tokens,
            cache_read_tokens: origin.usage.cache_read_tokens,
            cache_write_tokens: origin.usage.cache_write_tokens,
            cost_usd: cost,
            recorded_at: Utc::now(),
        };
        self.directory.record_cost(&ledger).await?;
        self.budget_notices(key, &ledger, true);
        self.persist(key).await?;
        Ok(())
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
            allowed_tools: Vec::new(),
            compacted_from: Some(source),
            compacting: false,
            compaction_baseline: 0,
        };
        self.lock_sessions().insert(key.clone(), record);
        self.persist(&key).await?;
        Ok(key)
    }

    /// Starts a turn (or a resume) and returns the stream of its events.
    ///
    /// The outer option distinguishes a new message (which replaces the
    /// message budget) from a resume (which keeps the one already in force).
    #[allow(
        clippy::option_option,
        clippy::too_many_arguments,
        clippy::too_many_lines
    )]
    async fn start_turn(
        self: &Arc<Self>,
        request_id: u64,
        session: Option<SessionKey>,
        input: Option<String>,
        message_budget: Option<Option<Budget>>,
        reasoning: Option<ProtocolReasoning>,
    ) -> Result<EventStream, ServiceError> {
        let (key, mut nid, mut spec) = if let Some(key) = session {
            let (nid, spec, _) = self.require_record(&key)?;
            (key, nid, spec)
        } else {
            let key = self.create_session(None).await?;
            let (nid, spec, _) = self.require_record(&key)?;
            (key, nid, spec)
        };
        let mut leading = Vec::new();
        if self.compaction_due(&key, &spec, nid).await? {
            leading = match self.perform_compaction(request_id, &key, "threshold").await {
                Ok(events) => events,
                Err(error) => vec![ServerEvent::error(
                    request_id,
                    Some(key.clone()),
                    ErrorCode::Conflict,
                    error.to_string(),
                )],
            };
            let (new_nid, new_spec, _) = self.require_record(&key)?;
            nid = new_nid;
            spec = new_spec;
        }
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
        let gate = self.compaction_gate(&spec);
        let engine = self.engine_with_budget(&spec, Some(Arc::clone(&book)), gate.clone())?;
        let state = engine.session(nid).await?;
        let turn = state.turn + 1;
        // A resumable park: after a restart the engine will not re-emit the
        // pending approvals, so surface them from the store before the turn.
        let restored: Vec<ServerEvent> = if state.phase == norte::Phase::AwaitingApproval {
            let mut restored = Vec::new();
            for record in engine.pending_approvals(nid).await? {
                let arguments = call_arguments(&engine, nid, record.call_id).await;
                let sandbox = (record.tool.as_str() == "command"
                    && self.config.tools.sandbox.enabled)
                    .then(|| sandbox_name().to_owned());
                restored.push(ServerEvent::new(
                    request_id,
                    Some(key.clone()),
                    ServerEventKind::ApprovalRequested {
                        turn,
                        approval: record.id.to_string(),
                        call_id: record.call_id.to_string(),
                        tool: record.tool.to_string(),
                        arguments,
                        sandbox,
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
                    engine,
                    Some(book),
                    gate,
                    request_id,
                    &task_key,
                    nid,
                    turn,
                    input,
                    reasoning,
                    &tx,
                    &notify,
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

        Ok(Box::pin(
            futures_util::stream::iter(leading)
                .chain(futures_util::stream::iter(restored))
                .chain(futures_util::stream::unfold(rx, |mut rx| async move {
                    rx.recv().await.map(|event| (event, rx))
                })),
        ))
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_turn(
        &self,
        mut engine: Engine,
        budget: Option<Arc<Mutex<BudgetBook>>>,
        mut gate: Option<Arc<CompactionGate>>,
        request_id: u64,
        key: &SessionKey,
        nid: SessionId,
        turn: u64,
        input: Option<String>,
        reasoning: ReasoningRequest,
        tx: &mpsc::Sender<ServerEvent>,
        notify: &Notify,
    ) -> Result<(), ServiceError> {
        let mut current_nid = nid;
        let mut input = input;
        let mut stats = self.stats(key, turn);
        let mut questions = self.questions.subscribe();
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

        let mut compacted_eagerly = false;
        let mut retries = 0u32;
        loop {
            let mut turn_builder = engine.turn(current_nid).reasoning(reasoning);
            if let Some(text) = input.take() {
                turn_builder = turn_builder.input(Message::user(text));
            }
            for (key, value) in &self.config.extensions {
                turn_builder = turn_builder.extension(key.clone(), value.clone());
            }
            let mut stream = turn_builder.start();

            let mut cancelled = false;
            let mut interrupted = false;
            let mut failure: Option<String> = None;
            let mut failure_code = None;
            loop {
                tokio::select! {
                    biased;
                    () = notify.notified() => {
                        cancelled = true;
                        interrupted = true;
                    }
                    question = questions.recv() => {
                        if let Ok(question) = question {
                            if question.session == Some(current_nid) {
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
                                if is_compaction_abort(&event) {
                                    failure = Some(COMPACTION_SAFE_POINT.to_owned());
                                    break;
                                }
                                // The engine reports a fatal error as an event and
                                // again as the turn's result. Wait for the result:
                                // the retry policy decides whether the client sees
                                // an error at all.
                                if !matches!(event, EngineEvent::Error { .. })
                                    && !self.handle(
                                        &engine,
                                        request_id,
                                        key,
                                        current_nid,
                                        &event,
                                        &mut stats,
                                        tx,
                                        gate.as_deref(),
                                    )
                                    .await
                                {
                                    return Ok(());
                                }
                            }
                            Some(Err(error)) => {
                                failure_code = Some(error_code(&error));
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
                .settle(
                    &engine,
                    request_id,
                    key,
                    current_nid,
                    &mut stats,
                    tx,
                    true,
                    gate.as_deref(),
                )
                .await;
            self.store_stats(key, &stats).await;

            let compaction_abort = failure.as_deref().is_some_and(is_compaction_message)
                || matches!(
                    &outcome,
                    Err(error) if is_compaction_message(&error.to_string())
                );
            if !cancelled && !interrupted && compaction_abort {
                match self.perform_compaction(request_id, key, "threshold").await {
                    Ok(events) => {
                        for event in events {
                            if !emit(tx, event).await {
                                return Ok(());
                            }
                        }
                    }
                    Err(error) => {
                        let phase = norte_code_protocol::Phase::Idle;
                        self.set_phase(key, phase).await;
                        emit(
                            tx,
                            ServerEvent::error(
                                request_id,
                                Some(key.clone()),
                                ErrorCode::Internal,
                                error.to_string(),
                            ),
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
                        return Ok(());
                    }
                }
                let (new_nid, new_spec, _) = self.require_record(key)?;
                if new_nid == current_nid {
                    // The summary failed; the old session is still the active
                    // one and the turn stopped at the safe point.
                    let phase = norte_code_protocol::Phase::Idle;
                    self.set_phase(key, phase).await;
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
                    return Ok(());
                }
                if let Some(record) = self.lock_sessions().get(key) {
                    stats.billed = record.billed;
                    stats.requests = record.requests;
                    stats.cost = record.cost;
                    stats.unpriced = record.unpriced;
                    stats.input_tokens = record.input_tokens;
                    stats.output_tokens = record.output_tokens;
                }
                engine = self.engine_with_budget(&new_spec, budget.clone(), None)?;
                current_nid = new_nid;
                input = None;
                gate = None;
                compacted_eagerly = true;
                continue;
            }

            // A transient failure is retried by resuming the same session with
            // no new input, so the user's message is never appended twice. The
            // engine left the session streaming from that message.
            if !cancelled && !interrupted {
                if let Err(error) = &outcome {
                    if let Some((delay, reason)) = retry_plan(error, &self.config.retry, retries) {
                        if let Ok(session) = engine.session(current_nid).await {
                            self.set_phase(key, to_phase(session.phase)).await;
                        }
                        let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
                        if !emit(
                            tx,
                            ServerEvent::new(
                                request_id,
                                Some(key.clone()),
                                ServerEventKind::RetryScheduled {
                                    turn,
                                    attempt: retries + 1,
                                    delay_ms,
                                    reason,
                                },
                            ),
                        )
                        .await
                        {
                            return Ok(());
                        }
                        let waited = tokio::select! {
                            biased;
                            () = notify.notified() => false,
                            () = tokio::time::sleep(delay) => true,
                        };
                        if !waited {
                            let phase = engine
                                .session(current_nid)
                                .await
                                .map_or(norte_code_protocol::Phase::Idle, |s| to_phase(s.phase));
                            self.set_phase(key, phase).await;
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
                            return Ok(());
                        }
                        retries += 1;
                        input = None;
                        continue;
                    }
                }
            }

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
                    if !compacted_eagerly
                        && !interrupted
                        && outcome.session.phase == norte::Phase::Idle
                        && self.budget_stopped(key).is_none()
                    {
                        if let Ok((current, spec, _)) = self.require_record(key) {
                            if self
                                .compaction_due(key, &spec, current)
                                .await
                                .unwrap_or(false)
                            {
                                match self.perform_compaction(request_id, key, "threshold").await {
                                    Ok(events) => {
                                        for event in events {
                                            if !emit(tx, event).await {
                                                break;
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        let _ = emit(
                                            tx,
                                            ServerEvent::error(
                                                request_id,
                                                Some(key.clone()),
                                                ErrorCode::Internal,
                                                error.to_string(),
                                            ),
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    let code = failure_code.unwrap_or_else(|| error_code(&error));
                    let message = failure.unwrap_or_else(|| error.to_string());
                    let message = if code == ErrorCode::Conflict {
                        format!(
                            "{message}; the session had already advanced, so this turn did not commit. \
                             Resume the session to continue from the latest commit."
                        )
                    } else {
                        message
                    };
                    let phase = engine
                        .session(current_nid)
                        .await
                        .map_or(norte_code_protocol::Phase::Idle, |s| to_phase(s.phase));
                    self.set_phase(key, phase).await;
                    emit(
                        tx,
                        ServerEvent::error(request_id, Some(key.clone()), code, message),
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
            break;
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
        gate: Option<&CompactionGate>,
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
                let sandbox = (tool.as_str() == "command" && self.config.tools.sandbox.enabled)
                    .then(|| sandbox_name().to_owned());
                Some(ServerEventKind::ApprovalRequested {
                    turn: stats.turn,
                    approval: approval.to_string(),
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    arguments,
                    sandbox,
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
            EngineEvent::RequestReconstructed { .. } => {
                return self
                    .settle(engine, request_id, key, nid, stats, tx, false, gate)
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
        gate: Option<&CompactionGate>,
    ) -> bool {
        let Ok(history) = engine.history(nid).await else {
            return true;
        };
        if let Some(gate) = gate {
            gate.update(used_context(&history));
        }
        let mut ok = std::collections::VecDeque::new();
        for record in self.telemetry_for(key, nid) {
            if record.status == "ok" {
                ok.push_back(record);
            } else {
                self.record_request(key, &record).await;
            }
        }
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
            let telemetry = ok
                .pop_front()
                .unwrap_or_else(|| self.request_from_usage(key, nid, stats.turn, &origin.usage));
            let metrics = request_metrics(&telemetry);
            self.record_request(key, &telemetry).await;
            let notices = self.budget_notices(key, &record, final_check);
            let event = ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::RequestReconstructed {
                    turn: stats.turn,
                    index: stats.requests,
                    usage: to_usage(&origin.usage),
                    cost_usd: cost,
                    truncated: matches!(origin.finish_reason, FinishReason::Length),
                    duration_ms: metrics.duration_ms,
                    first_token_ms: metrics.first_token_ms,
                    tokens_per_second: metrics.tokens_per_second,
                    cache_hit: metrics.cache_hit,
                    tools: metrics.tools,
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
        for record in ok {
            self.record_request(key, &record).await;
        }
        true
    }

    /// Takes the requests the telemetry layer finished since the last look.
    fn telemetry_for(
        &self,
        key: &SessionKey,
        nid: SessionId,
    ) -> Vec<norte_code_store_sqlite::RequestRecord> {
        let mut records = self.telemetry.take(nid);
        records.sort_by_key(|record| record.started_at);
        for record in &mut records {
            record.session = key.to_string();
        }
        records
    }

    fn budget_notices(
        &self,
        key: &SessionKey,
        record: &CostRecord,
        final_check: bool,
    ) -> Vec<BudgetNotice> {
        self.lock_budget_states()
            .get(key)
            .cloned()
            .map_or_else(Vec::new, |book| {
                let mut book = book.lock().unwrap_or_else(PoisonError::into_inner);
                book.record(record);
                if final_check {
                    book.notices()
                } else {
                    book.warnings()
                }
            })
    }

    async fn record_request(
        &self,
        key: &SessionKey,
        record: &norte_code_store_sqlite::RequestRecord,
    ) {
        if let Err(error) = self.directory.record_request(record).await {
            tracing::warn!(session = %key, %error, "could not record request telemetry");
        }
    }

    fn request_from_usage(
        &self,
        key: &SessionKey,
        nid: SessionId,
        turn: u64,
        usage: &Usage,
    ) -> norte_code_store_sqlite::RequestRecord {
        let model = self.model_label(key);
        let (provider, model) = model.split_once('/').map_or_else(
            || (String::new(), model.clone()),
            |(p, m)| (p.to_owned(), m.to_owned()),
        );
        norte_code_store_sqlite::RequestRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session: key.to_string(),
            north_session: nid.to_string(),
            turn,
            provider,
            model,
            started_at: Utc::now(),
            duration_ms: 0,
            first_token_ms: None,
            last_token_ms: None,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            estimated_input_tokens: None,
            cache_hit: usage.cache_read_tokens.unwrap_or(0) > 0,
            tool_calls: Vec::new(),
            reasoning: None,
            sandbox: self.config.tools.sandbox.enabled,
            sandbox_network: self.config.tools.sandbox.network,
            params: serde_json::Value::Null,
            extensions: serde_json::Value::Null,
            metadata: serde_json::json!({"source": "history"}),
            cost_usd: reported_cost(usage),
            finish_reason: None,
            status: "ok".to_owned(),
            error: None,
        }
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

#[cfg(test)]
mod tests;
