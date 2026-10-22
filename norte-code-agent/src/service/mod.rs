//! The in-process service: it owns the Norte engines, maps the client protocol
//! onto turns, and streams the engine's events back as [`ServerEvent`]s.
//!
//! An engine is cheap and shares the store and blob store, so each turn builds
//! one for the model the session selected. That is what makes a mid-session
//! model switch a change of model rather than a change of session.

mod approvals;
mod budgets;
mod compaction;
mod effects;
mod mapping;
mod models;
mod prompt;
mod questions;
mod sandbox;
mod sessions;
mod skills;
mod steering;
mod tooling;
mod workspace;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use chrono::Utc;
use futures_core::Stream;
use futures_util::StreamExt;
use norte::{
    ApprovalId, BlobStore, Engine, EngineBuilder, EngineConfig, EngineEvent, FinishReason, GrantId,
    GrantScope, InMemoryBlobStore, InMemoryStore, Layer, LayerStack, Message, Model, PolicySet,
    ReasoningRequest, SessionId, Store, ToolArgs, ToolDeclaration, ToolRegistry, Usage,
};
use norte_code_cost::{BudgetBook, BudgetNotice, CostRecord};
use norte_code_skills::{
    SkillRegistry, SkillResourceTool, SkillScriptConfig, SkillScriptTool, SkillTool,
};
use norte_code_store_sqlite::{InMemoryDirectory, SessionDirectory, StoredSession};
use norte_code_tools::{
    AskUserTool, GlobTool, GrepTool, ListDirTool, QuestionBroker, TodoState, TodoTool, WebFetchTool,
};
use norte_tools::{CommandLimits, CommandTool, EditTool, ReadTool, WriteTool};
use tokio::sync::{Notify, mpsc};

use norte_code_catalog::{Catalog, LoadOptions, load_catalog};
use norte_code_config::Config;
use norte_code_protocol::{
    Budget, ErrorCode, GrantInfo, GrantScope as ProtocolGrantScope, ModelInfo,
    Reasoning as ProtocolReasoning, ServerEvent, ServerEventKind, SessionKey,
};

use crate::ServiceError;
use crate::provider::{ModelFactory, ModelSpec};
use crate::telemetry::{Telemetry, TelemetryLayer, TelemetryObserver};

use self::sessions::SessionRecord;
use self::steering::{Steered, Steering, SteeringInterceptor};
use approvals::{ApprovalModePolicy, ApprovalRules};
use compaction::{
    COMPACTION_SAFE_POINT, CompactionGate, CompactionLayer, is_compaction_abort,
    is_compaction_message, used_context,
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
use tooling::{ToolActivity, edit_source, output_text, recording_tool};
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
    /// The steering messages the running turns have not reached.
    steering: Arc<Steering>,
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
            steering: Arc::new(Steering::default()),
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
    /// Builds an engine for one spec over the shared stores.
    fn engine(&self, spec: &ModelSpec) -> Result<Engine, ServiceError> {
        self.engine_with_budget(spec, None, None)
    }

    /// The runtime-awareness block stored when a session starts.
    fn runtime_context(&self, spec: &ModelSpec) -> String {
        self.runtime_context_with(spec, false)
    }

    /// The block appended when the session switches to `spec`.
    fn runtime_update(&self, spec: &ModelSpec) -> String {
        self.runtime_context_with(spec, true)
    }

    fn runtime_context_with(&self, spec: &ModelSpec, update: bool) -> String {
        let catalog = self.catalog();
        let model = catalog.find(&spec.provider, &spec.model);
        let config_path = self
            .config_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let user_config = config_path.or_else(norte_code_config::user_config_path);
        prompt::runtime_context(
            &prompt::RuntimeFacts {
                spec,
                model,
                config: &self.config,
                workspace: &self.workspace_root,
                user_config: user_config.as_deref(),
            },
            update,
        )
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
                let workspace_root = self.workspace_root.clone();
                move |info| {
                    effects.intent(info.session, info.call_id, info.tool.as_str());
                    if info.tool.as_str() == "edit" {
                        if let Some(source) = edit_source(&workspace_root, &info.arguments) {
                            activity.snapshot(info.call_id, source);
                        }
                    }
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
            ])
            .interceptors(vec![Arc::new(SteeringInterceptor::new(Arc::clone(
                &self.steering,
            )))]);
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
            system_chars: self
                .system_prompt
                .chars()
                .count()
                .saturating_add(self.runtime_context(&self.default_spec).chars().count()),
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
            .with_limits(CommandLimits {
                max_model_bytes: self.config.tools.command_max_model_bytes,
                ..CommandLimits::default()
            })
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
            Kind::SetReasoning {
                session,
                scope,
                reasoning,
            } => {
                self.set_reasoning(request_id, session, scope, reasoning)
                    .await
            }
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
            Kind::ListGrants { session } => self.list_grants_event(request_id, session).await,
            Kind::AnswerQuestion {
                session,
                question,
                answer,
            } => {
                self.answer_question(request_id, session, question, answer)
                    .await
            }
            Kind::Steer { session, text, id } => self.steer(request_id, session, text, id).await,
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
        let spec = self.resolve_model_switch(&session, &model).await?;
        match self.set_model(&session, spec).await {
            Ok(()) => Ok(single(
                request_id,
                Some(session),
                ServerEventKind::ModelChanged { model },
            )),
            // A live turn owns the boundary. Accept the request now and keep it
            // durable; `start_turn` applies it when the session is idle.
            Err(ServiceError::Unsupported(_)) => {
                {
                    let mut sessions = self.lock_sessions();
                    let record = sessions
                        .get_mut(&session)
                        .ok_or_else(|| ServiceError::UnknownSession(session.to_string()))?;
                    record.pending_model = Some(model.clone());
                }
                self.persist(&session).await?;
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::ModelQueued { model },
                ))
            }
            Err(error) => Err(error),
        }
    }

    /// Resolves a model reference and checks it against the stored reasoning
    /// and budget before the switch is queued or applied.
    async fn resolve_model_switch(
        &self,
        session: &SessionKey,
        model: &str,
    ) -> Result<ModelSpec, ServiceError> {
        let spec = ModelSpec::from_catalog(&self.config, &self.catalog(), model, None)?;
        self.ensure_model_supports_reasoning(session, &spec)?;
        self.ensure_model_supports_budget(session, &spec).await?;
        Ok(spec)
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

    async fn list_grants_event(
        &self,
        request_id: u64,
        session: SessionKey,
    ) -> Result<EventStream, ServiceError> {
        let (nid, _, _) = self.require_record(&session)?;
        let mut grants = Vec::new();
        for record in self.store.approvals(nid).await? {
            for grant in record.grants {
                grants.push(GrantInfo {
                    approval: record.id.to_string(),
                    grant: grant.id.to_string(),
                    tool: record.tool.to_string(),
                    scope: match grant.scope {
                        GrantScope::Tool(_) => ProtocolGrantScope::Tool,
                        GrantScope::Session(_) => ProtocolGrantScope::Session,
                        GrantScope::Global => ProtocolGrantScope::Always,
                        // A one-call grant and any scope Norte may add later
                        // reach the client as the narrowest scope there is.
                        _ => ProtocolGrantScope::Once,
                    },
                    issued_at: grant.issued_at.to_rfc3339(),
                    revoked: !grant.is_active(),
                });
            }
        }
        Ok(single(
            request_id,
            Some(session),
            ServerEventKind::GrantList { grants },
        ))
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
        if let Some(changed) = self.apply_pending_model(request_id, &key).await? {
            leading.push(changed);
            let (new_nid, new_spec, _) = self.require_record(&key)?;
            nid = new_nid;
            spec = new_spec;
        }
        if self.compaction_due(&key, &spec, nid).await? {
            // The compaction joins what the boundary already reported: a model
            // switch that landed first must not be dropped.
            let events = match self.perform_compaction(request_id, &key, "threshold").await {
                Ok(events) => events,
                Err(error) => vec![ServerEvent::error(
                    request_id,
                    Some(key.clone()),
                    ErrorCode::Conflict,
                    error.to_string(),
                )],
            };
            leading.extend(events);
            let (new_nid, new_spec, _) = self.require_record(&key)?;
            nid = new_nid;
            spec = new_spec;
        }
        let (session_reasoning, pending_reasoning, pending_steers) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (
                record.reasoning.clone(),
                record.pending_reasoning.clone(),
                record.pending_steers.clone(),
            )
        };
        // Precedence: an explicit message setting, then a one-shot override,
        // then the session override, then the configured default.
        let requested_reasoning = reasoning
            .or(pending_reasoning.clone())
            .or(session_reasoning);
        let reasoning = resolve_reasoning(
            requested_reasoning.as_ref(),
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
        // The steering messages belong to the turn boundary that is starting
        // now. The interceptor takes them before the turn's input, so they keep
        // their arrival order; a message the engine already holds is not
        // queued twice.
        for steered in &pending_steers {
            if !self.steering.is_pending(nid, &steered.id) {
                self.steering.push(nid, steered.clone());
            }
        }
        if pending_reasoning.is_some() {
            self.clear_pending(&key, pending_reasoning.as_ref(), &[]);
            self.persist(&key).await?;
        }
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
                let source = self.activity.source(call_id);
                Some(ServerEventKind::ToolStarted {
                    turn: stats.turn,
                    call_id: call_id.to_string(),
                    tool: tool.to_string(),
                    arguments,
                    source,
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
            EngineEvent::TurnStopped { reason, .. } => Some(ServerEventKind::TurnStopped {
                reason: reason.clone(),
            }),
            EngineEvent::InterceptionApplied { messages, .. } => {
                // The engine applied the steering messages, so the durable copy
                // in the session is no longer pending.
                let applied = self.steering.applied(nid, *messages);
                let ids: Vec<String> = applied.iter().map(|steered| steered.id.clone()).collect();
                self.clear_applied_steers(key, &applied).await;
                Some(ServerEventKind::SteerApplied { ids })
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

    /// Flattens the catalogue for a picker, applying the config overrides.
    fn list_models(&self) -> Vec<ModelInfo> {
        model_list(&self.config, &self.catalog())
    }

    /// Validates and sets the session's budget.
    ///
    /// A USD budget over a provider that never reported cost cannot be
    /// measured, so it is refused instead of silently comparing against zero.
    async fn steer(
        &self,
        _request_id: u64,
        key: SessionKey,
        text: String,
        id: String,
    ) -> Result<EventStream, ServiceError> {
        // The message takes the request already in flight. The durable copy
        // stays in the session until the engine reports that it applied it, so
        // a crash cannot lose it.
        let (nid, _, _) = self.require_record(&key)?;
        let id = if id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            id
        };
        let steered = Steered::new(id.clone(), text);
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.pending_steers.push(steered.clone());
        }
        self.persist(&key).await?;
        self.steering.push(nid, steered);
        Ok(Box::pin(futures_util::stream::empty()))
    }

    /// Refuses a budget that the session's history says cannot be measured.
    /// Blocks a switch that would make a stored reasoning setting unsupported.
    fn ensure_model_supports_reasoning(
        &self,
        key: &SessionKey,
        spec: &ModelSpec,
    ) -> Result<(), ServiceError> {
        let (session_reasoning, pending_reasoning) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (record.reasoning.clone(), record.pending_reasoning.clone())
        };
        for setting in [session_reasoning.as_ref(), pending_reasoning.as_ref()]
            .into_iter()
            .flatten()
        {
            resolve_reasoning(
                Some(setting),
                &self.config.reasoning.default,
                &spec.capabilities.reasoning,
            )?;
        }
        Ok(())
    }

    async fn set_model(&self, key: &SessionKey, spec: ModelSpec) -> Result<(), ServiceError> {
        let (nid, _, phase) = self.require_record(key)?;
        if phase != norte_code_protocol::Phase::Idle {
            return Err(ServiceError::Unsupported(
                "switch the model at a turn boundary",
            ));
        }
        let engine = self.engine(&spec)?;
        if engine.session(nid).await?.phase != norte::Phase::Idle {
            return Err(ServiceError::Unsupported(
                "switch the model at a turn boundary",
            ));
        }
        engine
            .append_history(nid, vec![Message::system(self.runtime_update(&spec))])
            .await?;
        {
            let mut lock = self.lock_sessions();
            let record = lock
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.spec = spec;
            record.pending_model = None;
        }
        self.persist(key).await
    }

    /// Applies a queued model switch when the session reaches an idle turn
    /// boundary. An approval or question continuation is not a boundary, so it
    /// leaves the request queued for the next real turn.
    async fn apply_pending_model(
        &self,
        request_id: u64,
        key: &SessionKey,
    ) -> Result<Option<ServerEvent>, ServiceError> {
        let Some(model) = self
            .lock_sessions()
            .get(key)
            .and_then(|record| record.pending_model.clone())
        else {
            return Ok(None);
        };
        let spec = match self.resolve_model_switch(key, &model).await {
            Ok(spec) => spec,
            Err(error) => {
                // The queued switch cannot be honoured anymore: the model left
                // the catalogue, or the session's settings refuse it. The turn
                // goes on with the current model; failing here would break
                // every turn of the session from now on.
                tracing::warn!(
                    session = %key,
                    model = %model,
                    %error,
                    "dropping a queued model switch that cannot be honoured"
                );
                self.clear_pending_model(key).await;
                return Ok(None);
            }
        };
        match self.set_model(key, spec).await {
            Ok(()) => Ok(Some(ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::ModelChanged { model },
            ))),
            Err(ServiceError::Unsupported(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Forgets a queued model switch.
    async fn clear_pending_model(&self, key: &SessionKey) {
        {
            let mut sessions = self.lock_sessions();
            let Some(record) = sessions.get_mut(key) else {
                return;
            };
            record.pending_model = None;
        }
        if let Err(error) = self.persist(key).await {
            tracing::warn!(session = %key, %error, "could not store the dropped model switch");
        }
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
