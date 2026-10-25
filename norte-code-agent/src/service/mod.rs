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
mod mcp;
mod models;
mod prompt;
mod questions;
mod sandbox;
mod sessions;
mod skills;
mod steering;
mod tooling;
mod turns;
mod workspace;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use futures_core::Stream;
use norte::{
    AnyFunction, BlobStore, Engine, EngineBuilder, EngineConfig, InMemoryBlobStore, InMemoryStore,
    Layer, LayerStack, Model, PolicySet, Store, ToolArgs, ToolDeclaration, ToolRegistry,
    TurnInterceptor,
};
use norte_code_cost::{BudgetBook, CostRecord};
use norte_code_skills::{
    SkillRegistry, SkillResourceTool, SkillScriptConfig, SkillScriptTool, SkillTool,
};
use norte_code_store_sqlite::{InMemoryDirectory, SessionDirectory, StoredSession};
use norte_code_tools::{
    AskUserTool, GlobTool, GrepTool, ListDirTool, QuestionBroker, TodoState, TodoTool, WebFetchTool,
};
use norte_tools::{BrowserTool, CommandLimits, CommandTool, EditTool, ReadTool, WriteTool};
use tokio::sync::Notify;

use norte_code_catalog::Catalog;
use norte_code_config::Config;
use norte_code_protocol::{ServerEvent, ServerEventKind, SessionKey};

use crate::ServiceError;
use crate::provider::{ModelFactory, ModelSpec};
use crate::telemetry::{Telemetry, TelemetryLayer, TelemetryObserver};

use self::sessions::SessionRecord;
use self::steering::{Steered, Steering, SteeringInterceptor};
use approvals::{ApprovalModePolicy, ApprovalRules};
use compaction::{CompactionGate, CompactionLayer};
use effects::{EffectJournal, EffectJournalPolicy};
use mapping::{BudgetLayer, single};
use mcp::McpServers;
use models::resolve_reasoning;
use prompt::{compose_system_prompt, load_project_instructions};
use questions::DirectoryQuestions;
use sandbox::sandboxed_command;
use skills::{collect_secrets, discover_skills};
use tooling::{ToolActivity, edit_source, recording_tool};
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
#[derive(Debug, Clone, Default)]
struct Stats {
    turn: u64,
    billed: usize,
    requests: u64,
    cost: f64,
    unpriced: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    /// The request telemetry that has not been matched to a committed message
    /// yet.
    ///
    /// The telemetry arrives when a provider answers and the message is
    /// committed after that; a settle that runs in between keeps the telemetry
    /// here instead of dropping it, so the request that follows is measured
    /// with its own numbers.
    pending: std::collections::VecDeque<norte_code_store_sqlite::RequestRecord>,
}

/// Where the catalogue is refreshed from.
#[derive(Clone, Debug)]
struct CatalogSettings {
    url: String,
    ttl: Duration,
    offline: bool,
    cache_dir: PathBuf,
}

/// One MCP server, as the startup block reports it.
#[derive(Clone, Debug)]
pub struct McpServerStatus {
    /// The name the configuration gave it.
    pub name: String,
    /// The program that was started.
    pub command: String,
    /// The tools it published, by the name the model sees.
    pub tools: Vec<String>,
}

/// A configuration that was re-read, with the keys it did not recognise.
#[derive(Debug)]
pub struct ReloadedConfig {
    /// The configuration now in force.
    pub config: Config,
    /// Unknown keys, as dotted paths, for the caller to report.
    pub warnings: Vec<String>,
}

/// The Norte Code service.
pub struct Service {
    config: RwLock<Config>,
    catalog: RwLock<Arc<Catalog>>,
    catalog_settings: CatalogSettings,
    default_spec: RwLock<ModelSpec>,
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
    secrets: RwLock<Vec<String>>,
    directory: Arc<dyn SessionDirectory>,
    effects: Arc<EffectJournal>,
    activity: Arc<ToolActivity>,
    sessions: Mutex<BTreeMap<SessionKey, SessionRecord>>,
    cancels: Mutex<BTreeMap<SessionKey, Arc<Notify>>>,
    /// The steering messages the running turns have not reached.
    steering: Arc<Steering>,
    /// The interceptors the host registered, consulted at the safe points of
    /// every turn.
    interceptors: Vec<Arc<dyn TurnInterceptor>>,
    /// The MCP servers that connected, and the tools they published.
    mcp_servers: RwLock<McpServers>,
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
        service.connect_mcp().await;
        service.directory.repair_interrupted().await?;
        service.load_sessions().await?;
        service.effects.load().await?;
        Ok(service)
    }

    /// Registers an interceptor the engine consults during every turn.
    ///
    /// The engine asks the interceptors at the safe points of a turn: before
    /// the turn's input, before every model request and before the turn ends.
    /// An interceptor may let the turn go on, add messages to the history, or
    /// stop the turn with a reason. A stopped turn is reported to the client
    /// as [`ServerEventKind::TurnStopped`], which reads as a decision and not
    /// as a failure.
    ///
    /// The steering interceptor the service owns is always registered; the
    /// host's own interceptors are consulted after it.
    #[must_use]
    pub fn with_interceptor(mut self, interceptor: Arc<dyn TurnInterceptor>) -> Self {
        self.interceptors.push(interceptor);
        self
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
            config: RwLock::new(config),
            catalog: RwLock::new(catalog),
            catalog_settings: settings,
            default_spec: RwLock::new(default_spec),
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
            secrets: RwLock::new(secrets),
            directory,
            effects,
            activity,
            sessions: Mutex::new(BTreeMap::new()),
            cancels: Mutex::new(BTreeMap::new()),
            steering: Arc::new(Steering::default()),
            interceptors: Vec::new(),
            // Connected in `with_directory`, which can await; `new` cannot.
            mcp_servers: RwLock::new(McpServers::none()),
            budget_states: Mutex::new(BTreeMap::new()),
            approval_rules,
            config_path: Mutex::new(norte_code_config::user_config_path()),
            telemetry: Arc::new(Telemetry::default()),
        };
        // Fail at construction, not at the first message, when the model's
        // provider cannot be built.
        let _ = service.engine(
            &service
                .default_spec
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )?;
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
                max_iterations: self.config().max_iterations.unwrap_or(usize::MAX),
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
            .interceptors({
                // The steering comes first: a message the host holds belongs
                // in the history before anything the host's own interceptor
                // decides about the turn.
                let mut interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![Arc::new(
                    SteeringInterceptor::new(Arc::clone(&self.steering)),
                )];
                interceptors.extend(self.interceptors.iter().cloned());
                interceptors
            });
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
            self.config().tools.confinement.sandbox.enabled,
            self.config().tools.confinement.sandbox.network,
        )));
        builder = builder.layers(LayerStack::new(layers));
        Ok(builder.build()?)
    }

    /// The eager-compaction gate, when enabled and the model declares a context
    /// window. Without a window there is no threshold to compare against.
    fn compaction_gate(&self, spec: &ModelSpec) -> Option<Arc<CompactionGate>> {
        if !self.config().compaction.enabled || self.config().compaction.trigger != "eager" {
            return None;
        }
        let max_context = spec.capabilities.max_context?;
        Some(Arc::new(CompactionGate::new(
            max_context,
            self.config().compaction.threshold,
            self.config().compaction.keep_recent,
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
        self.register_tool(&mut tools, BrowserTool::new().into_declaration())?;

        let mut env = BTreeMap::new();
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_owned(), path);
        }
        let command = CommandTool::new()
            .with_limits(CommandLimits {
                max_model_bytes: self.config().tools.command_max_model_bytes,
                ..CommandLimits::default()
            })
            .with_env(env)
            .with_secrets(
                self.secrets
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            )
            .with_blobs(Arc::clone(&self.blobs))
            .into_declaration();
        let command = if self.config().tools.confinement.sandbox.enabled {
            sandboxed_command(
                command,
                &self.workspace_root,
                &self.config().tools.confinement.sandbox,
            )
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
        if self.config().tools.web_fetch {
            self.register_tool(&mut tools, WebFetchTool::new().into_declaration())?;
        }

        if self.config().skills.enabled {
            let skills = self.skills();
            self.register_tool(
                &mut tools,
                SkillTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            self.register_tool(
                &mut tools,
                SkillResourceTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            if self.config().skills.allow_scripts {
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

        // The MCP tools were wrapped when the servers connected; registering
        // them here is what puts them in front of the model.
        let mcp_tools: Vec<Arc<dyn AnyFunction>> = self
            .mcp_servers
            .read()
            .map(|servers| servers.tools())
            .unwrap_or_default();
        for tool in mcp_tools {
            tools.register(tool)?;
        }
        Ok(tools)
    }

    /// Connects the configured MCP servers, once, at startup.
    async fn connect_mcp(&self) {
        let root = self.workspace_root.clone();
        // The servers are read before the first await: a `std` lock must not be
        // held across one.
        let configured = self.lock_config().mcp_servers.clone();
        let connected = McpServers::connect(&configured, &root).await;
        for (name, reason) in &connected.failures {
            tracing::warn!(server = %name, %reason, "an MCP server could not be started");
        }
        if let Ok(mut slot) = self.mcp_servers.write() {
            *slot = connected;
        }
    }

    /// The MCP servers that connected, for the startup line.
    ///
    /// Each one carries the names of the tools it published, because a count
    /// says the server is up and the names are what a person needs to ask the
    /// model for one.
    pub fn mcp_status(&self) -> Vec<McpServerStatus> {
        self.mcp_servers
            .read()
            .map(|servers| {
                servers
                    .servers
                    .iter()
                    .map(|server| McpServerStatus {
                        name: server.name.clone(),
                        command: server.label(),
                        tools: server.tool_names(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The MCP servers that could not be started, for the startup line.
    pub fn mcp_failures(&self) -> Vec<(String, String)> {
        self.mcp_servers
            .read()
            .map(|servers| servers.failures.clone())
            .unwrap_or_default()
    }

    fn policies(&self) -> PolicySet {
        PolicySet::new(vec![
            Arc::new(EffectJournalPolicy {
                journal: Arc::clone(&self.effects),
            }),
            Arc::new(WorkspacePolicy {
                root: self.workspace_root.clone(),
                enforce_root: self.config().tools.confinement.workspace_only,
            }),
            Arc::new(ApprovalModePolicy {
                mode: self.config().approval_mode,
                skills: self.skills(),
                honor_allowed_tools: self.config().skills.honor_allowed_tools,
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
        let registry = Arc::new(discover_skills(&self.lock_config(), &self.workspace_root));
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

    /// The configuration in force.
    ///
    /// A value rather than a borrow: the config is held behind a lock so a
    /// reload can replace it, and it is a small type of strings and numbers.
    /// The lock is held only long enough to copy.
    #[must_use]
    pub fn config(&self) -> Config {
        self.lock_config().clone()
    }

    /// Replaces the configuration.
    ///
    /// The caller rebuilds what was derived from it; this only swaps the value.
    pub(super) fn replace_config(&self, next: Config) {
        *self
            .config
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    }

    /// Re-reads the configuration and rebuilds what it derived.
    ///
    /// Returns the new configuration so the caller can report what changed.
    ///
    /// Rebuilt: the default spec, the secrets registered for redaction, the
    /// skill registry and the global approval rules. Not rebuilt: session state.
    /// A session that already resolved a model keeps it, and the rules the user
    /// granted during this run stay granted; the new default applies to the next
    /// session.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] when the file cannot be read, parsed or
    /// validated, and [`ServiceError::Provider`] when the default model it names
    /// cannot be built. Nothing is replaced in either case.
    pub fn reload_config(&self) -> Result<ReloadedConfig, ServiceError> {
        let path = self
            .config_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let project = self
            .workspace_root
            .ancestors()
            .map(|dir| dir.join(".norte-code").join("config.json"))
            .find(|candidate| candidate.exists());
        let loaded = Config::load(path.as_deref(), project.as_deref())?;
        let next = loaded.config;

        // Everything that can fail happens before the live service is touched.
        let catalog = self.catalog();
        let reference = next.default_model.clone();
        let spec = match reference {
            Some(reference) => ModelSpec::from_catalog(&next, &catalog, &reference, None)?,
            // No default model: keep the one in force rather than fail, since a
            // config that only changes unrelated settings must still reload.
            None => self.default_spec(),
        };
        let secrets = collect_secrets(&next, &catalog);
        let skills = Arc::new(discover_skills(&next, &self.workspace_root));

        self.replace_config(next.clone());
        *self
            .secrets
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = secrets;
        *self
            .default_spec
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = spec;
        *self.lock_skills_write() = skills;
        // Only the global rules come from the config; the per-session grants the
        // user made during this run are session state and stay.
        for tool in &next.approvals.allow {
            self.approval_rules.allow_global(tool);
        }
        for tool in &next.approvals.deny {
            self.approval_rules.deny_global(tool);
        }
        Ok(ReloadedConfig {
            config: next,
            warnings: loaded.warnings,
        })
    }

    /// The default spec in force.
    pub(super) fn default_spec(&self) -> ModelSpec {
        self.default_spec
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The config lock, for reads that must not copy the whole value.
    pub(super) fn lock_config(&self) -> std::sync::RwLockReadGuard<'_, Config> {
        self.config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
