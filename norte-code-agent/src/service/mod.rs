//! The in-process service: it owns the Norte engines, maps the client protocol
//! onto turns, and streams the engine's events back as [`ServerEvent`]s.
//!
//! An engine is cheap and shares the store and blob store, so each turn builds
//! one for the model the session selected. That is what makes a mid-session
//! model switch a change of model rather than a change of session.

mod approvals;
mod background;
mod budgets;
mod compaction;
mod detach;
mod effects;
mod jobs;
mod manage;
mod mapping;
mod mcp;
mod models;
mod prompt;
mod questions;
mod sandbox;
mod screenshots;
mod sessions;
mod skills;
mod steering;
mod titling;
mod tooling;
mod turns;
mod workspace;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use futures_core::Stream;
use norte::harness::TurnInterceptor;
use norte::harness::{
    Harness, HarnessBuilder, HarnessConfig, InMemoryBlobStore, InMemoryStore, PolicySet, Store,
};
use norte::http::{HttpClient, HttpMedia};
use norte::{
    AnyFunction, BlobStore, Layer, LayerStack, MediaAdapter, Model, ToolArgs, ToolDeclaration,
    ToolRegistry,
};
use norte_code_cost::{BudgetBook, CostRecord};
use norte_code_skills::{
    SkillRegistry, SkillResourceTool, SkillScriptConfig, SkillScriptTool, SkillTool,
};
use norte_code_store_sqlite::{InMemoryDirectory, SessionDirectory, StoredSession};
use norte_code_tools::{
    AskUserTool, GlobTool, GrepTool, ListDirTool, QuestionBroker, TodoState, TodoTool, WebFetchTool,
};
use norte_tools::{
    BrowserTool, EditTool, ReadTool, ShellLimits, ShellRequest, ShellTool, WriteTool,
};
use tokio::sync::Notify;

use norte_code_catalog::Catalog;
use norte_code_config::Config;
use norte_code_protocol::{Reasoning, ServerEvent, ServerEventKind, SessionKey};

use crate::ServiceError;
use crate::provider::{ModelFactory, ModelSpec};
use crate::telemetry::{Telemetry, TelemetryLayer, TelemetryObserver};

use self::screenshots::{ScreenshotInterceptor, Screenshots, publishing_tool};
use self::sessions::SessionRecord;
use self::steering::{Steered, Steering, SteeringInterceptor};
use approvals::{ApprovalModePolicy, ApprovalRules};
use background::ShellCalls;
use compaction::{CompactionGate, CompactionLayer};
use detach::DetachRegistry;
use effects::{EffectJournal, EffectJournalPolicy};
use jobs::{JobRegistry, job_info};
use manage::ManageJobs;
use mapping::{BudgetLayer, single};
use mcp::McpServers;
use models::resolve_reasoning;
#[cfg(test)]
use prompt::skills_block;
use prompt::{compose_system_prompt, load_project_instructions};
use questions::DirectoryQuestions;
use sandbox::sandboxed_command;
use skills::{collect_secrets, discover_skills};
use tooling::{ToolActivity, edit_source, recording_tool};
use workspace::WorkspacePolicy;

pub use approvals::{RuleScope, ToolRule, ToolStatus};
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
    /// Whether its tools reach the model right now.
    pub active: bool,
    /// Names the configuration asked for that the server does not publish.
    pub filtered: Vec<String>,
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
    /// The browser screenshots waiting to be attached to the next request.
    screenshots: Arc<Screenshots>,
    /// The shell calls running in the background, by job id.
    jobs: Arc<JobRegistry>,
    /// The detach token of the call each session's turn is waiting on.
    detaches: Arc<DetachRegistry>,
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

    /// Starts a session without a message.
    ///
    /// A client that offers a startup choice — begin a new session, continue the
    /// last one, pick one from a list — needs the session to exist *before* the
    /// first message decides it. Until now the only ways in were a message, which
    /// creates one as a side effect, and a resume, which needs a session that
    /// already exists; so a client that asked for the list first, then for the new
    /// session, ended up with two: the empty one the list returned and the one the
    /// first message created. The key comes back here, and the message that
    /// follows names it.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the engine cannot create the session or the
    /// directory cannot record it.
    pub async fn start_session(&self, title: Option<String>) -> Result<SessionKey, ServiceError> {
        self.create_session(title).await
    }

    /// The sessions the client can see, newest first.
    #[must_use]
    pub fn sessions(&self) -> Vec<norte_code_protocol::SessionInfo> {
        self.list_sessions()
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
        let secrets = collect_secrets(&config, &catalog);
        // Discovered before the prompt is composed, so their names can go into
        // it: a model cannot activate a skill it has no way of knowing about.
        // Only the names: the descriptions are what `skill(action: list)`
        // returns, and paying for them on every request to pick one out of a
        // list the session most often does not need is the cost the prompt
        // block stays clear of.
        let skills = Arc::new(discover_skills(&config, &workspace_root));
        // By precedence, not by name: the prompt's budget is spent on the
        // skills closest to this workspace first, so a bundled set cannot crowd
        // out the one the session was set up for.
        let skill_index: Vec<String> = skills
            .skills_by_precedence()
            .iter()
            .map(|skill| skill.name.clone())
            .collect();
        let system_prompt =
            compose_system_prompt(&system_prompt.into(), project.as_ref(), &skill_index);
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
            screenshots: Arc::new(Screenshots::default()),
            jobs: Arc::new(JobRegistry::default()),
            detaches: Arc::new(DetachRegistry::default()),
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
    fn engine(&self, spec: &ModelSpec) -> Result<Harness, ServiceError> {
        self.engine_with_budget(spec, None, None)
    }

    /// Builds an engine whose requests pass through the budget and telemetry
    /// layers, plus an optional eager-compaction layer.
    fn engine_with_budget(
        &self,
        spec: &ModelSpec,
        budget: Option<Arc<Mutex<BudgetBook>>>,
        compaction: Option<Arc<CompactionGate>>,
    ) -> Result<Harness, ServiceError> {
        let model: Arc<dyn Model> = self.factory.build(spec, &spec.capabilities)?;
        // The adapter is what turns a `Blob` back into bytes on the way out. A
        // message may carry media the client stored — a pasted image — and the
        // wire only takes inline bytes or a URL, so without this the part
        // reaches the provider as a blob id it has never heard of. The engine
        // resolves it here, and always the same way, so a cached prefix does not
        // move (FR-RECON-4).
        let media: Arc<dyn MediaAdapter> = Arc::new(HttpMedia::new(&HttpClient::new()?));
        let mut builder = HarnessBuilder::new()
            .store(Arc::clone(&self.store))
            .blobs(Arc::clone(&self.blobs))
            .media_adapter(media)
            .model(model)
            .tools(self.tools()?)
            .policies(self.policies())
            .config(HarnessConfig {
                max_iterations: self.config().max_iterations.unwrap_or(usize::MAX),
                ..HarnessConfig::default()
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
                    as Arc<dyn norte::harness::Observer>,
            ])
            .interceptors({
                // The steering comes first: a message the host holds belongs
                // in the history before anything the host's own interceptor
                // decides about the turn.
                let mut interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![Arc::new(
                    SteeringInterceptor::new(Arc::clone(&self.steering)),
                )];
                // Read from the spec the engine was built with — never
                // rebuilt, so the check the interceptor makes and the
                // capabilities the model is handed are the same ones.
                interceptors.push(Arc::new(ScreenshotInterceptor::new(
                    Arc::clone(&self.screenshots),
                    &spec.capabilities,
                )));
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

    /// The shell, and the manager for the jobs it can leave behind.
    ///
    /// One shell with one set of limits: the sandbox, the secrets, the output
    /// bounds and the time bound are the same whether a call is waited for or let
    /// go of, and `detach` on the request is what says which.
    fn register_shell(&self, tools: &mut ToolRegistry) -> Result<(), ServiceError> {
        let mut env = BTreeMap::new();
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_owned(), path);
        }
        // The zero the host grants, because one of the two shapes a call can
        // take is a process nobody waits for: a server started for the rest of
        // the session needs no bound, and its own bound is what ends it if it
        // has one. A call that asks for the same zero *and* holds the turn is
        // the one shape this host refuses, and the wrapper is where it is
        // refused: it drops the field, so the request falls through to the
        // default above. Keeping that rule in one place is the reason the two
        // shells became one.
        let limits = ShellLimits {
            max_model_bytes: self.config().tools.command_max_model_bytes,
            allow_indefinite: true,
            ..ShellLimits::default()
        };
        let secrets = self
            .secrets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        // One builder for both, so nothing but the flag can drift apart.
        let build = |limits: ShellLimits| -> ToolDeclaration<ShellRequest> {
            let declaration = ShellTool::new()
                .with_limits(limits)
                .with_env(env.clone())
                .with_secrets(secrets.clone())
                .with_blobs(Arc::clone(&self.blobs))
                .into_declaration();
            if self.config().tools.confinement.sandbox.enabled {
                sandboxed_command(
                    declaration,
                    &self.workspace_root,
                    &self.config().tools.confinement.sandbox,
                )
            } else {
                declaration
            }
        };
        // One shell. Whether a call holds the turn or is let go of is the
        // request's `detach` field, so there is nothing here to keep in step:
        // the sandbox, the secrets, the output bounds and the limits are one
        // set, and the flag is the only thing that differs.
        let command = build(limits);
        let command = ShellCalls::declaration(
            command,
            Arc::clone(&self.jobs),
            Arc::clone(&self.steering),
            Arc::clone(&self.detaches),
        );
        self.register_tool(tools, command)?;
        self.register_tool(
            tools,
            ManageJobs::new(Arc::clone(&self.jobs)).into_declaration(),
        )?;
        Ok(())
    }

    /// The tool registry every engine shares, rebuilt cheaply per turn because
    /// Norte's registry is immutable and not cloneable.
    ///
    /// Public so a host can report what the model can reach without asking the
    /// engine, which is what `/debug` shows.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when a declaration is invalid or a name is
    /// already taken.
    pub fn tools(&self) -> Result<ToolRegistry, ServiceError> {
        let mut tools = ToolRegistry::new();
        self.register_tool(&mut tools, ReadTool::new().into_declaration())?;
        self.register_tool(&mut tools, WriteTool::new().into_declaration())?;
        self.register_tool(&mut tools, EditTool::new().into_declaration())?;
        // The browser's capture is promoted into a user message: the tool
        // answers with the PNG as base64 in its result, and a tool result
        // carries text only on the chat completions wire. The wrapper reads the
        // capture out before the model sees the answer, and the interceptor
        // below attaches it.
        let browser = BrowserTool::new().into_declaration();
        let browser = if self.config().tools.screenshots.enabled {
            publishing_tool(
                browser,
                Arc::clone(&self.screenshots),
                Arc::clone(&self.blobs),
                screenshots::preprocessor(&self.config().tools.screenshots),
            )
        } else {
            browser
        };
        self.register_tool(&mut tools, browser)?;

        self.register_shell(&mut tools)?;
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
                        active: server.is_active(),
                        filtered: server.filtered.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Turns one MCP server off or on for the rest of the process.
    ///
    /// The process stays up, so turning it back on is instant; what changes is
    /// whether its tools are put in front of the model.
    ///
    /// Returns the state the server is in afterwards, or `None` when no server
    /// by that name connected.
    pub fn set_mcp_active(&self, name: &str, active: bool) -> Option<bool> {
        let mut servers = self.mcp_servers.write().ok()?;
        servers.set_active(name, active)
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

    /// The config file the process was pointed at, if any.
    ///
    /// Kept here rather than threaded through every caller, so a report can say
    /// which file is in force. `None` means the platform default path is used.
    #[must_use]
    pub fn config_path(&self) -> Option<PathBuf> {
        self.config_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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

    /// Removes one tool-wide rule from the user config and forgets it in memory.
    ///
    /// It is what "ask again" means: the rule is dropped from the file and from
    /// the live registry together, so a call to the tool is decided by the mode
    /// from the next turn on and the rule does not come back at the next start.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Approval`] when there is no config path to write
    /// to, which is the same condition a global rule has.
    fn forget_global_rule(&self, tool: &str) -> Result<(), ServiceError> {
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
        norte_code_config::forget_user_approval(&path, tool)?;
        self.approval_rules.forget_global(tool);
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

    /// Retries the MCP servers that failed, returning how many answered.
    ///
    /// The ones that are connected are left alone: they keep their process and
    /// their session.
    pub async fn retry_mcp(&self) -> usize {
        let root = self.workspace_root.clone();
        // A `std` lock must not be held across an await, so the config is read
        // and the retry runs without one; the lock is taken only to install the
        // result.
        let configured = self.lock_config().mcp_servers.clone();
        let pending = match self.mcp_servers.read() {
            Ok(servers) => servers.failed_to_retry(&configured),
            Err(_) => return 0,
        };
        let mut recovered = Vec::new();
        let mut still_failed = Vec::new();
        for (name, config) in pending {
            match McpServers::connect_configured(&name, &config, &root).await {
                Ok(server) => recovered.push(server),
                Err(reason) => still_failed.push((name, reason)),
            }
        }
        let count = recovered.len();
        if let Ok(mut servers) = self.mcp_servers.write() {
            servers.install(recovered, still_failed);
        }
        count
    }

    /// The skills registry in use.
    #[must_use]
    pub fn skills(&self) -> Arc<SkillRegistry> {
        Arc::clone(&self.lock_skills())
    }

    /// The `provider/model` reference a session is on, if it is known.
    ///
    /// A client needs it to change one half of the reference while keeping the
    /// other: `/provider` swaps the provider and keeps the model, and it cannot
    /// read that from anywhere of its own while a turn is running.
    #[must_use]
    pub fn session_model(&self, session: &SessionKey) -> Option<String> {
        self.lock_sessions()
            .get(session)
            .map(|record| record.spec.label())
    }

    /// The model a session is on right now, with its capabilities.
    ///
    /// The whole spec and not just its label: `/debug` reports what the model in
    /// play can do, and after a `/model` those are the new model's capabilities,
    /// not the ones the process started with.
    #[must_use]
    pub fn session_spec(&self, session: &SessionKey) -> Option<ModelSpec> {
        self.lock_sessions()
            .get(session)
            .map(|record| record.spec.clone())
    }

    /// The reasoning override a session carries, if it has one.
    ///
    /// `None` is not "no reasoning": it means the session set none, so the
    /// answer is the config's default as the model resolves it — a different
    /// thing, because a model that cannot reason turns `auto` into `off`.
    #[must_use]
    pub fn session_reasoning(&self, session: &SessionKey) -> Option<Reasoning> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.reasoning.clone())
    }

    /// Whether the session has a background job still running.
    ///
    /// The CLI asks before deciding what `Ctrl-C` means: it ends a job when
    /// there is one and does nothing otherwise. A plain `bool` rather than a
    /// borrow, so the answer does not hold the sessions lock across a redraw.
    #[must_use]
    pub fn has_running_jobs(&self, session: &SessionKey) -> bool {
        self.lock_sessions()
            .get(session)
            .is_some_and(|record| self.jobs.any_running(record.id))
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
        // The provider and the model are two config keys and never a split
        // reference: joined here only because the resolver takes the pair as
        // `provider/model` and the model keeps every slash it has.
        let spec = match (&next.default_provider, &next.default_model) {
            (Some(provider), Some(model)) => {
                Self::default_spec_for(&next, &catalog, &format!("{provider}/{model}"))?
            }
            // No default: keep the one in force rather than fail, since a config
            // that only changes unrelated settings must still reload.
            _ => self.default_spec(),
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

    /// Resolves a `provider/model` reference the way a run does.
    ///
    /// [`ModelSpec::from_catalog`] only knows the catalogue, and a configured
    /// provider is not in it: `river` and every other private endpoint is
    /// declared in `config.json` and nowhere else. A reload that went straight
    /// to the catalogue therefore failed on a default model that a run resolves
    /// without complaint — `run --provider river` worked while `/refresh` said
    /// the provider was unknown, which is the same reference answered two ways.
    ///
    /// So the catalogue is tried first and a configured provider falls back the
    /// same way the command line does. A reference that is neither — a typo, or
    /// a catalogue model that does not exist — is still an error.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the reference is not `provider/model`, or
    /// when it names neither a catalogue provider nor a configured one.
    fn default_spec_for(
        config: &Config,
        catalog: &Catalog,
        reference: &str,
    ) -> Result<ModelSpec, ServiceError> {
        match ModelSpec::from_catalog(config, catalog, reference, None) {
            Err(ServiceError::UnknownProvider(provider)) => {
                // A provider the catalogue does not list has to be one the host
                // configured, or there is nothing to resolve a key or an
                // endpoint from.
                if !config.providers.contains_key(&provider) {
                    return Err(ServiceError::UnknownProvider(provider));
                }
                // No fallback environment names: a configured provider names its
                // key variable in `api_key_env`, and `api_key_for` reads that
                // before it reaches this list. Passing guesses here would let a
                // provider resolve a key from a variable it never declared.
                ModelSpec::resolve(config, reference, None, &[])
            }
            // The provider is in the catalogue and the model is not: a fresh
            // release, or a model named in the config and nowhere else. The
            // provider still supplies the wire, the endpoint and the key, so the
            // reference resolves with conservative capabilities instead of
            // failing. This is what lets `/provider` name a provider whose
            // catalogue does not carry the model the session was on: the model
            // stays whatever it was, and the person is told to switch it.
            Err(ServiceError::UnknownModel { provider, .. })
                if catalog.provider(&provider).is_some() =>
            {
                ModelSpec::from_provider(config, catalog, reference, None)
            }
            other => other,
        }
    }

    /// The default spec in force.
    ///
    /// The model a new session opens on, and the one `/debug` reports when it
    /// is asked before any message has named a session.
    #[must_use]
    pub fn default_spec(&self) -> ModelSpec {
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

    /// Replaces every session name in a request with the session it names.
    ///
    /// The only place a short name becomes a full one. It is a match over the
    /// request rather than a method on it so the protocol crate stays free of
    /// the service's session table, and every arm that carries a
    /// [`SessionKey`] is listed: one that is missed would keep answering
    /// "unknown" for a name the footer shows.
    #[allow(clippy::too_many_lines)] // one arm per request variant, which is the enum.
    fn canonical(
        &self,
        kind: norte_code_protocol::ClientRequestKind,
    ) -> Result<norte_code_protocol::ClientRequestKind, ServiceError> {
        use norte_code_protocol::ClientRequestKind as Kind;
        let resolve = |key: SessionKey| self.resolve_key(&key);
        Ok(match kind {
            Kind::SwitchSession { session } => Kind::SwitchSession {
                session: resolve(session)?,
            },
            Kind::ForkSession { session, title } => Kind::ForkSession {
                session: resolve(session)?,
                title,
            },
            Kind::RenameSession { session, title } => Kind::RenameSession {
                session: resolve(session)?,
                title,
            },
            Kind::DeleteSession { session } => Kind::DeleteSession {
                session: resolve(session)?,
            },
            Kind::SendMessage {
                session,
                text,
                attachments,
                budget,
                reasoning,
                replay,
            } => Kind::SendMessage {
                session: session.map(resolve).transpose()?,
                text,
                attachments,
                budget,
                reasoning,
                replay,
            },
            Kind::Steer { session, text, id } => Kind::Steer {
                session: resolve(session)?,
                text,
                id,
            },
            Kind::CancelTurn { session } => Kind::CancelTurn {
                session: resolve(session)?,
            },
            Kind::StopJobs { session, id } => Kind::StopJobs {
                session: resolve(session)?,
                id,
            },
            Kind::DetachTurn { session } => Kind::DetachTurn {
                session: resolve(session)?,
            },
            Kind::ListJobs { session } => Kind::ListJobs {
                session: resolve(session)?,
            },
            Kind::Resume { session } => Kind::Resume {
                session: resolve(session)?,
            },
            Kind::Approve {
                session,
                approval,
                arguments,
                scope,
                remember,
            } => Kind::Approve {
                session: resolve(session)?,
                approval,
                arguments,
                scope,
                remember,
            },
            Kind::Deny {
                session,
                approval,
                reason,
                scope,
            } => Kind::Deny {
                session: resolve(session)?,
                approval,
                reason,
                scope,
            },
            Kind::Revoke {
                session,
                approval,
                grant,
            } => Kind::Revoke {
                session: resolve(session)?,
                approval,
                grant,
            },
            Kind::ListGrants { session } => Kind::ListGrants {
                session: resolve(session)?,
            },
            Kind::AnswerQuestion {
                session,
                question,
                answer,
            } => Kind::AnswerQuestion {
                session: resolve(session)?,
                question,
                answer,
            },
            Kind::SetModel { session, model } => Kind::SetModel {
                session: resolve(session)?,
                model,
            },
            Kind::SetReasoning {
                session,
                scope,
                reasoning,
            } => Kind::SetReasoning {
                session: resolve(session)?,
                scope,
                reasoning,
            },
            Kind::SetBudget { session, budget } => Kind::SetBudget {
                session: resolve(session)?,
                budget,
            },
            Kind::Compact { session } => Kind::Compact {
                session: resolve(session)?,
            },
            Kind::CostReport { session } => Kind::CostReport {
                session: session.map(resolve).transpose()?,
            },
            // Every other request names no session, so there is nothing to
            // resolve and it is returned as it arrived.
            other => other,
        })
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
        mut request: norte_code_protocol::ClientRequest,
    ) -> Result<EventStream, ServiceError> {
        use norte_code_protocol::ClientRequestKind as Kind;
        // A session may be named by a prefix of its id: the footer shows eight
        // characters and a person types back what they can see. Resolve it once
        // here, at the one door every request comes through, so no handler has
        // to know a name may be short — it is handed the canonical key or the
        // error that says why it is not one.
        request.kind = self.canonical(request.kind)?;
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
                attachments,
                budget,
                reasoning,
                replay,
            } => {
                self.start_turn(
                    request_id,
                    session,
                    Some(text),
                    attachments,
                    Some(budget),
                    reasoning,
                    replay,
                )
                .await
            }
            Kind::Resume { session } => {
                // A resume exists to show where the conversation was.
                self.start_turn(
                    request_id,
                    Some(session),
                    None,
                    Vec::new(),
                    None,
                    None,
                    true,
                )
                .await
            }
            Kind::CancelTurn { session } => {
                if let Some(notify) = self.lock_cancels().get(&session).cloned() {
                    notify.notify_one();
                }
                Ok(Box::pin(futures_util::stream::empty()))
            }
            Kind::StopJobs { session, id } => {
                // A job's token is its own: stopping one is not cancelling the
                // turn, and this is the path that says so.
                let stopped = {
                    let sessions = self.lock_sessions();
                    match sessions.get(&session) {
                        Some(record) => match id.as_deref() {
                            Some(id) => usize::from(self.jobs.cancel(id)),
                            None => self.jobs.cancel_session(record.id),
                        },
                        None => 0,
                    }
                };
                Ok(Box::pin(futures_util::stream::iter([ServerEvent::new(
                    request_id,
                    Some(session),
                    ServerEventKind::JobsStopped { stopped },
                )])))
            }
            Kind::DetachTurn { session } => {
                // Detaching is not cancelling: the call stops waiting and the
                // process keeps running. The token is the one the executor
                // published for the session, so no turn state is touched here.
                let detached = self
                    .lock_sessions()
                    .get(&session)
                    .is_some_and(|record| self.detaches.detach(record.id));
                Ok(Box::pin(futures_util::stream::iter([ServerEvent::new(
                    request_id,
                    Some(session),
                    ServerEventKind::TurnStopped {
                        reason: if detached {
                            "the running call was detached; its process keeps going".to_owned()
                        } else {
                            "there was no running call to detach".to_owned()
                        },
                    },
                )])))
            }
            Kind::ListJobs { session } => {
                let jobs = match self.lock_sessions().get(&session) {
                    Some(record) => self.jobs.list(Some(record.id)),
                    None => Vec::new(),
                };
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::JobList {
                        jobs: jobs.iter().map(job_info).collect(),
                    },
                ))
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
