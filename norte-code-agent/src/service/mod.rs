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
mod plans;
mod prompt;
mod questions;
mod sandbox;
mod screenshots;
mod sessions;
mod skills;
mod steering;
mod subagents;
mod tooling;
mod turns;
mod workspace;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::runtime::TurnInterceptor;
use crate::runtime::{Harness, HarnessBuilder, HarnessConfig, InMemoryBlobStore, Store};
use futures_core::Stream;
use futures_util::StreamExt;
use norte::http::{HttpClient, HttpMedia};
use norte::{
    AnyFunction, BlobStore, Layer, LayerStack, MediaAdapter, Model, ToolArgs, ToolDeclaration,
    ToolRegistry,
};
use norte_code_protocol::costs::{BudgetBook, CostRecord};
use norte_code_protocol::skills::{
    SkillRegistry, SkillResourceTool, SkillScriptConfig, SkillScriptTool, SkillTool,
};
use norte_code_protocol::sqlite::{InMemoryDirectory, SessionDirectory, StoredSession};
use norte_code_protocol::tools::{
    AskUserTool, GrepTool, QuestionBroker, TodoState, TodoTool, WebFetchTool,
};
use norte_tools::{
    BrowserTool, EditTool, ReadTool, ShellLimits, ShellRequest, ShellTool, WriteTool,
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use norte_code_protocol::catalog::{Catalog, Models, Providers, load_catalog_files};
use norte_code_protocol::config::{ApiKey, CompactionTrigger, Config};
use norte_code_protocol::keys::KeyStore;
use norte_code_protocol::plugins::ContextUsage;
use norte_code_protocol::{Phase, Reasoning, ServerEvent, ServerEventKind, SessionKey};

use crate::provider::{ModelFactory, ModelSpec};
use crate::telemetry::{Telemetry, TelemetryLayer, TelemetryObserver};
use crate::{DEFAULT_SUBAGENT_SYSTEM_PROMPT, ServiceError};

use self::screenshots::{ScreenshotInterceptor, Screenshots, publishing_tool};
use self::sessions::SessionRecord;
use self::steering::{Steered, Steering, SteeringInterceptor};
use self::subagents::{SubagentOutcome, SubagentRunner, SubagentTool};
use approvals::{ApprovalRules, ToolRuleInterceptor};
use background::ShellCalls;
use compaction::{CompactionGate, CompactionLayer};
use detach::DetachRegistry;
use effects::{EffectJournal, journalled};
use jobs::{JobRegistry, job_info};
use manage::ManageJobs;
use mapping::{BudgetLayer, single};
use mcp::McpServers;
use models::resolve_reasoning;
use plans::{PlanRepository, PlanTool};
use prompt::{compose_system_prompt, load_project_instructions};
use questions::DirectoryQuestions;
use sandbox::sandboxed_command;
use skills::{collect_secrets, discover_skills};
use tooling::{ToolActivity, edit_source, recording_tool};
use workspace::{Confined, PathKind, Workspace, confined};

pub use approvals::{RuleScope, ToolStatus};
pub use models::{effective_reasoning, model_list};
pub use prompt::PromptSize;

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
    /// The input tokens of the last request, which is the context.
    ///
    /// The whole conversation is sent again every time, so the last request's
    /// input tokens are what the context holds — not the sum, which is what
    /// was spent.
    context_used: u64,
    /// The request telemetry that has not been matched to a committed message
    /// yet.
    ///
    /// The telemetry arrives when a provider answers and the message is
    /// committed after that; a settle that runs in between keeps the telemetry
    /// here instead of dropping it, so the request that follows is measured
    /// with its own numbers.
    pending: std::collections::VecDeque<norte_code_protocol::sqlite::RequestRecord>,
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

pub(super) struct CancelRegistration {
    pub(super) generation: u64,
    pub(super) token: CancellationToken,
}

/// The Norte Code service.
pub struct Service {
    /// How many inputs may wait for the turn in flight (RFC 12.1:
    /// `limits.max_queued_inputs`, default 8).
    ///
    /// One more is refused with `SessionBusy` (`ERROR SESSION_BUSY` on the
    /// wire) instead of growing `pending_steers` without bound.
    max_queued_inputs: usize,
    config: RwLock<Config>,
    agent_profiles: RwLock<BTreeMap<String, Config>>,
    catalog: RwLock<Arc<Catalog>>,
    plugin_catalogue: RwLock<(Providers, Models)>,
    default_spec: RwLock<Option<ModelSpec>>,
    store: Arc<dyn Store>,
    blobs: Arc<dyn BlobStore>,
    factory: Arc<dyn ModelFactory>,
    system_prompt: String,
    subagent_system_prompt: String,
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
    plans: Arc<PlanRepository>,
    sessions: Mutex<BTreeMap<SessionKey, SessionRecord>>,
    cancels: Mutex<BTreeMap<SessionKey, CancelRegistration>>,
    next_cancel_generation: AtomicU64,
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
    /// The runner the `subagent` tool asks, set once the service is built.
    subagent_runner: RwLock<Option<Arc<dyn SubagentRunner>>>,
    /// The service reference the shell calls hold, so a finished job can
    /// start a turn of its own when the flag is on. Set the same way the
    /// subagent runner is.
    job_host: Arc<RwLock<Option<Arc<Service>>>>,
    /// Sessions with an auto-turn already running, so a second job end for the
    /// same session does not start a duplicate one at the same time.
    auto_turns: Mutex<BTreeSet<SessionKey>>,
    /// Serializes durable turn boundaries for each session. Steering does not
    /// take this lock; it is deliberately allowed to reach the turn in flight.
    turn_locks: Mutex<BTreeMap<SessionKey, Arc<AsyncMutex<()>>>>,
    /// The request ids handed to auto-turns, so their events are not mistaken
    /// for a client request's.
    auto_request_id: AtomicU64,
    budget_states: Mutex<BTreeMap<SessionKey, Arc<Mutex<BudgetBook>>>>,
    approval_rules: Arc<ApprovalRules>,
    config_path: Mutex<Option<PathBuf>>,
    telemetry: Arc<Telemetry>,
}

/// Whether a name is in a session's withheld list.
///
/// The comparison is exact: a name is what the model calls a tool, and a prefix
/// that matched — `read` for `read_file` — would take away a tool nobody named.
fn withheld(disabled: &[String], name: &str) -> bool {
    disabled.iter().any(|entry| entry == name)
}

impl Service {
    /// Returns the gate that serializes turn starts for one session.
    pub(super) fn turn_lock(&self, key: &SessionKey) -> Arc<AsyncMutex<()>> {
        self.turn_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// Resolves the model and tool policy for a child session.
    async fn subagent_settings(
        &self,
        parent: &SessionKey,
    ) -> Result<
        (
            ModelSpec,
            Vec<String>,
            Arc<Mutex<BudgetBook>>,
            Option<usize>,
        ),
        ServiceError,
    > {
        let (_, parent_spec, _) = self.require_record(parent)?;
        let parent_spec =
            parent_spec.ok_or_else(|| ServiceError::UnknownSession(parent.to_string()))?;
        let parent_config = self.config();
        let target_name = parent_config
            .subagent
            .as_deref()
            .unwrap_or(&parent_config.name);
        let target_config = self
            .agent_profiles
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(target_name)
            .cloned()
            .ok_or_else(|| {
                ServiceError::Config(norte_code_protocol::config::ConfigError::Invalid {
                    path: "subagent".to_owned(),
                    message: format!("does not name a configured agent: {target_name}"),
                })
            })?;
        let spec = match (
            target_config.default_provider.as_deref(),
            target_config.default_model.as_deref(),
        ) {
            (Some(provider), Some(model)) if target_name != parent_config.name => {
                self.default_spec_for(&target_config, &self.lock_catalog(), provider, model)?
            }
            _ => parent_spec,
        };
        let parent_disabled = self
            .lock_sessions()
            .get(parent)
            .map(|record| record.disabled_tools.clone())
            .unwrap_or_default();
        let mut disabled = parent_disabled;
        disabled.extend(target_config.tools.disabled.iter().cloned());
        disabled.extend([
            "ask_user".to_owned(),
            "browser_use".to_owned(),
            "subagent".to_owned(),
        ]);
        disabled.sort();
        disabled.dedup();
        let budget = self.budget_book(parent).await?;
        Ok((spec, disabled, budget, target_config.max_iterations))
    }

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
        default_spec: Option<ModelSpec>,
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
        default_spec: Option<ModelSpec>,
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
        default_spec: Option<ModelSpec>,
        factory: Arc<dyn ModelFactory>,
        store: Arc<dyn Store>,
        blobs: Arc<dyn BlobStore>,
        directory: Arc<dyn SessionDirectory>,
        system_prompt: impl Into<String>,
    ) -> Result<Self, ServiceError> {
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
        let subagent_base = config
            .system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_SUBAGENT_SYSTEM_PROMPT);
        let subagent_system_prompt =
            compose_system_prompt(subagent_base, project.as_ref(), &skill_index);
        let questions = Arc::new(
            QuestionBroker::new()
                .with_store(Arc::new(DirectoryQuestions::new(Arc::clone(&directory)))),
        );
        let effects = Arc::new(EffectJournal::new(Arc::clone(&directory)));
        let activity = Arc::new(ToolActivity::default());
        let plans = Arc::new(PlanRepository::new(Arc::clone(&directory)));
        let approval_rules = Arc::new(ApprovalRules::new(&config.approvals));
        let agent_profiles = BTreeMap::from([(config.name.clone(), config.clone())]);
        let service = Self {
            max_queued_inputs: 8,
            config: RwLock::new(config),
            agent_profiles: RwLock::new(agent_profiles),
            catalog: RwLock::new(catalog),
            plugin_catalogue: RwLock::new((Providers::default(), Models::default())),
            default_spec: RwLock::new(default_spec),
            store,
            blobs,
            factory,
            system_prompt,
            subagent_system_prompt,
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
            plans,
            sessions: Mutex::new(BTreeMap::new()),
            cancels: Mutex::new(BTreeMap::new()),
            next_cancel_generation: AtomicU64::new(1),
            steering: Arc::new(Steering::default()),
            screenshots: Arc::new(Screenshots::default()),
            jobs: Arc::new(JobRegistry::default()),
            detaches: Arc::new(DetachRegistry::default()),
            interceptors: Vec::new(),
            // Connected in `with_directory`, which can await; `new` cannot.
            mcp_servers: RwLock::new(McpServers::none()),
            subagent_runner: RwLock::new(None),
            job_host: Arc::new(RwLock::new(None)),
            auto_turns: Mutex::new(BTreeSet::new()),
            turn_locks: Mutex::new(BTreeMap::new()),
            auto_request_id: AtomicU64::new(1 << 62),
            budget_states: Mutex::new(BTreeMap::new()),
            approval_rules,
            config_path: Mutex::new(norte_code_protocol::config::user_config_path()),
            telemetry: Arc::new(Telemetry::default()),
        };
        // Fail at construction, not at the first message, when the model's
        // provider cannot be built. A run with no configured model carries an
        // empty spec and is validated later, when one is chosen.
        let spec = service
            .default_spec
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(spec) = spec
            && !spec.model.trim().is_empty()
        {
            let _ = service.engine(&spec, None)?;
        }
        Ok(service)
    }

    /// Supplies the service-owned agent definitions used by delegated turns.
    ///
    /// The active service still owns its own sessions; the additional
    /// definitions only select the child model and policy when an agent names
    /// one in `subagent`.
    pub fn with_agent_profiles(self: Arc<Self>, profiles: BTreeMap<String, Config>) -> Arc<Self> {
        *self
            .agent_profiles
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = profiles;
        self
    }

    /// Supplies catalogue entries contributed by server-loaded plugins.
    ///
    /// Plugin code belongs to the host process, so the service keeps only its
    /// catalogue offer. A later catalogue refresh can then rebuild the same
    /// merged view instead of silently dropping plugin providers and models.
    pub fn set_plugin_catalogue(&self, providers: Providers, models: Models) {
        *self.lock_plugin_catalogue_write() = (providers, models);
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
            Some(spec.clone()),
            factory,
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            system_prompt,
        )
    }

    /// Rebuilds the in-memory session map from the durable directory.
    #[allow(clippy::too_many_lines)]
    /// Builds an engine for one spec over the shared stores.
    ///
    /// The session is what decides which tools the engine is built with: the
    /// withheld list is the session's own snapshot, not the configuration's
    /// current value, because the declarations are part of the prompt's prefix
    /// and moving them under a live conversation throws away its cache.
    fn engine(&self, spec: &ModelSpec, key: Option<&SessionKey>) -> Result<Harness, ServiceError> {
        self.engine_with_budget(spec, key, None, None, None)
    }

    /// Builds an engine whose requests pass through the budget and telemetry
    /// layers, plus an optional eager-compaction layer.
    fn engine_with_budget(
        &self,
        spec: &ModelSpec,
        key: Option<&SessionKey>,
        budget: Option<Arc<Mutex<BudgetBook>>>,
        compaction: Option<Arc<CompactionGate>>,
        max_iterations: Option<usize>,
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
            .tools(self.tools(key)?)
            .config(HarnessConfig {
                max_iterations: max_iterations
                    .or(self.config().max_iterations)
                    .unwrap_or(usize::MAX),
                ..HarnessConfig::default()
            })
            .tool_metadata({
                let effects = Arc::clone(&self.effects);
                let activity = Arc::clone(&self.activity);
                let workspace_root = self.workspace_root.clone();
                move |info| {
                    effects.intent(info.session, info.call_id, info.tool.as_str());
                    if info.tool.as_str() == "edit"
                        && let Some(source) = edit_source(&workspace_root, &info.arguments)
                    {
                        activity.snapshot(info.call_id, source);
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
                    as Arc<dyn crate::runtime::Observer>,
            ])
            .interceptors({
                // The steering comes first: a message the host holds belongs
                // in the history before anything the host's own interceptor
                // decides about the turn.
                let mut interceptors: Vec<Arc<dyn TurnInterceptor>> = vec![Arc::new(
                    SteeringInterceptor::new(Arc::clone(&self.steering)),
                )];
                // A rule refuses the call before it runs, and the reason it
                // gives is what the model reads.
                interceptors.push(Arc::new(ToolRuleInterceptor::new(
                    Arc::clone(&self.approval_rules),
                    Arc::clone(&self.activity),
                )));
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
        if !self.config().compaction.enabled
            || self.config().compaction.trigger != CompactionTrigger::Eager
        {
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
        // No session: this measures the static material of a prompt about to be
        // built, which is the configuration's list before anything froze it.
        let tool_chars = self
            .tools(None)?
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
    ///
    /// A tool the configuration disables is not registered at all: it is not in
    /// the schema the model is handed, not in the request, and not in the
    /// registry a call is looked up in. Refusing the call after the model made
    /// it would cost a round trip and teach it nothing, and the declarations are
    /// what the prompt pays for on every request.
    ///
    /// The name is read from the declaration, which is the name the model calls,
    /// so one list covers the host's own tools and the MCP servers' alike.
    fn register_tool<A>(
        &self,
        tools: &mut ToolRegistry,
        disabled: &[String],
        declaration: ToolDeclaration<A>,
    ) -> Result<(), ServiceError>
    where
        A: ToolArgs + serde::de::DeserializeOwned + std::fmt::Debug + 'static,
    {
        if withheld(disabled, declaration.name) {
            return Ok(());
        }
        let name = declaration.name;
        // Two gates: whether a crash may already have run the call, then the
        // workspace confine. Approval rules are evaluated before calls run by
        // the engine's `ToolRuleInterceptor`.
        let declaration = journalled(declaration, Arc::clone(&self.effects));
        let recorded = recording_tool(declaration, Arc::clone(&self.activity));
        let tool = confined(Arc::new(recorded), self.confine(name));
        tools.register(tool)?;
        Ok(())
    }

    /// The path arguments a tool names, and what they are expected to be.
    ///
    /// A tool the host does not know gets no confine: a rule naming a path
    /// field of a tool nobody here wrote would be a guess, and an MCP tool's
    /// paths are its server's business.
    fn confine(&self, tool: &str) -> Workspace {
        let enforce_root = self.config().tools.confinement.workspace_only;
        let fields = match tool {
            "read" | "write" | "edit" => vec![Confined {
                key: "path",
                default_to_root: false,
                kind: PathKind::File,
            }],
            "grep" => vec![Confined {
                key: "path",
                default_to_root: true,
                kind: PathKind::FileOrDirectory,
            }],
            "shell" => vec![Confined {
                key: "cwd",
                default_to_root: false,
                kind: PathKind::Directory,
            }],
            _ => Vec::new(),
        };
        Workspace {
            root: self.workspace_root.clone(),
            enforce_root,
            fields,
        }
    }

    /// The tools withheld from a session.
    ///
    /// A session that exists answers with the list it was made with, which is
    /// what keeps its prompt prefix stable. One that does not exist yet — the
    /// engine a session is created through, before there is a record to freeze
    /// — answers with the configuration, which is the list that is about to be
    /// frozen into it.
    fn disabled_for(&self, session: Option<&SessionKey>) -> Vec<String> {
        session
            .and_then(|key| {
                self.lock_sessions()
                    .get(key)
                    .map(|record| record.disabled_tools.clone())
            })
            .unwrap_or_else(|| self.configured_disabled_tools())
    }

    /// The shell, and the manager for the jobs it can leave behind.
    ///
    /// One shell with one set of limits: the sandbox, the secrets, the output
    /// bounds and the time bound are the same whether a call is waited for or let
    /// go of, and `detach` on the request is what says which.
    fn register_shell(
        &self,
        tools: &mut ToolRegistry,
        disabled: &[String],
    ) -> Result<(), ServiceError> {
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
            Arc::clone(&self.job_host),
        );
        self.register_tool(tools, disabled, command)?;
        self.register_tool(
            tools,
            disabled,
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
    #[allow(clippy::too_many_lines)]
    pub fn tools(&self, session: Option<&SessionKey>) -> Result<ToolRegistry, ServiceError> {
        let disabled = self.disabled_for(session);
        let mut tools = ToolRegistry::new();
        self.register_tool(&mut tools, &disabled, ReadTool::new().into_declaration())?;
        self.register_tool(&mut tools, &disabled, WriteTool::new().into_declaration())?;
        self.register_tool(&mut tools, &disabled, EditTool::new().into_declaration())?;
        self.register_tool(
            &mut tools,
            &disabled,
            PlanTool::new(Arc::clone(&self.plans), Arc::clone(&self.questions)).into_declaration(),
        )?;
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
        self.register_tool(&mut tools, &disabled, browser)?;

        self.register_shell(&mut tools, &disabled)?;
        self.register_tool(
            &mut tools,
            &disabled,
            GrepTool::new(&self.workspace_root).into_declaration(),
        )?;
        self.register_tool(
            &mut tools,
            &disabled,
            TodoTool::new(Arc::clone(&self.todo)).into_declaration(),
        )?;
        self.register_tool(
            &mut tools,
            &disabled,
            AskUserTool::new(Arc::clone(&self.questions)).into_declaration(),
        )?;
        let runner: Arc<dyn SubagentRunner> = self
            .subagent_runner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Arc::new(SubagentRunnerImpl::new(None)) as Arc<dyn SubagentRunner>);
        self.register_tool(
            &mut tools,
            &disabled,
            SubagentTool::new(
                runner,
                Arc::clone(&self.jobs),
                Arc::clone(&self.steering),
                Arc::clone(&self.detaches),
                Arc::clone(&self.job_host),
                3,
            )
            .into_declaration(),
        )?;

        if self.config().tools.web_fetch {
            self.register_tool(
                &mut tools,
                &disabled,
                WebFetchTool::new().into_declaration(),
            )?;
        }

        if self.config().skills.enabled {
            let skills = self.skills();
            self.register_tool(
                &mut tools,
                &disabled,
                SkillTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            self.register_tool(
                &mut tools,
                &disabled,
                SkillResourceTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            if self.config().skills.allow_scripts {
                self.register_tool(
                    &mut tools,
                    &disabled,
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
            // An MCP tool is named by its server — `server__tool` — and the
            // configuration disables it by that same name, so one list covers
            // the host's tools and the servers' alike.
            if withheld(&disabled, tool.name()) {
                continue;
            }
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

    /// Every tool the model can reach, with how a call to it is decided.
    ///
    /// The registry is the list, so a tool that failed to register is absent
    /// and one from an MCP server is present, which is what makes this the
    /// answer to "what can the model do right now". Each entry carries whether
    /// a call to it runs and where that rule comes from: `deny` before `allow`,
    /// which is the order the gate applies, and asking the service rather than
    /// reading the rules in the client is what keeps the two from disagreeing.
    ///
    /// A tool named only by a session rule is in the list when the session is
    /// the one it was set for, and a rule naming a tool the registry does not
    /// have is left out: a list of tools is not a list of names somebody typed.
    pub fn tool_rules(&self, session: Option<&SessionKey>) -> Vec<ToolStatus> {
        // A session that is not open has no session rules, and asking for them
        // is not an error: the list is what the *next* call meets, and a session
        // that was never opened has none.
        let session_id =
            session.and_then(|key| self.lock_sessions().get(key).map(|record| record.id));
        let session_id = session_id.unwrap_or_default();
        // The list a client is shown is the one the session it is on was made
        // with, so `/tools` and the gate agree about what the model can reach.
        let Ok(tools) = self.tools(session) else {
            return Vec::new();
        };
        let mut statuses: Vec<ToolStatus> = tools
            .specs()
            .into_iter()
            .map(|spec| {
                let name = spec.name.to_string();
                let (allowed, scope) = self.approval_rules.status(session_id, &name);
                ToolStatus {
                    name,
                    allowed,
                    scope,
                }
            })
            .collect();
        statuses.sort_by(|left, right| left.name.cmp(&right.name));
        statuses
    }

    /// Sets one rule for a session or the user config.
    ///
    /// A session rule is stored with the logical session, so it survives a
    /// restart. A global rule is written to the user config and replaces the
    /// opposite rule for the same tool. Setting an allow rule is also how a
    /// previous deny is revoked.
    ///
    /// # Errors
    ///
    /// Returns an error when the tool name is empty, the session is unknown, a
    /// session rule is requested without a session, or the config cannot be
    /// written.
    pub async fn set_approval_rule(
        &self,
        session: Option<&SessionKey>,
        tool: &str,
        allow: bool,
        global: bool,
    ) -> Result<(), ServiceError> {
        let tool = tool.trim();
        if tool.is_empty() {
            return Err(ServiceError::Approval(
                "the approval rule needs a tool name".to_owned(),
            ));
        }
        if global {
            self.remember_global_rule(tool, allow)?;
            return Ok(());
        }
        let session = session.ok_or_else(|| {
            ServiceError::Approval(
                "a session rule needs an open session; send a message first".to_owned(),
            )
        })?;
        let (nid, _, _) = self.require_record(session)?;
        {
            let mut records = self.lock_sessions();
            let record = records
                .get_mut(session)
                .ok_or_else(|| ServiceError::UnknownSession(session.to_string()))?;
            record.denied_tools.retain(|denied| denied != tool);
            record.allowed_tools.retain(|allowed| allowed != tool);
            if allow {
                record.allowed_tools.push(tool.to_owned());
            } else {
                record.denied_tools.push(tool.to_owned());
            }
        }
        if allow {
            self.approval_rules.allow_session(nid, tool);
        } else {
            self.approval_rules.deny_session(nid, tool);
        }
        self.persist(session).await
    }

    /// Forgets a tool's rule.
    ///
    /// It is the third state a rule panel offers: a tool is allowed or denied,
    /// and "no rule" is what is left when neither is written down. The rule is
    /// removed from both sides — the live registry and the session or the
    /// config file — because a rule forgotten in memory and kept on disk comes
    /// back at the next restart.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Approval`] when the tool is unnamed or no
    /// session is open and the rule was a session's.
    pub async fn forget_approval_rule(
        &self,
        session: Option<&SessionKey>,
        tool: &str,
        global: bool,
    ) -> Result<(), ServiceError> {
        let tool = tool.trim();
        if tool.is_empty() {
            return Err(ServiceError::Approval(
                "the approval rule needs a tool name".to_owned(),
            ));
        }
        if global {
            return self.forget_global_rule(tool);
        }
        let session = session.ok_or_else(|| {
            ServiceError::Approval(
                "a session rule needs an open session; send a message first".to_owned(),
            )
        })?;
        let (nid, _, _) = self.require_record(session)?;
        {
            let mut records = self.lock_sessions();
            let record = records
                .get_mut(session)
                .ok_or_else(|| ServiceError::UnknownSession(session.to_string()))?;
            record.denied_tools.retain(|denied| denied != tool);
            record.allowed_tools.retain(|allowed| allowed != tool);
        }
        self.approval_rules.forget_session(nid, tool);
        self.persist(session).await
    }

    /// Writes one tool-wide rule to the user config and applies it in memory.
    fn remember_global_rule(&self, tool: &str, allow: bool) -> Result<(), ServiceError> {
        let path = self
            .config_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .or_else(norte_code_protocol::config::user_config_path)
            .ok_or_else(|| {
                ServiceError::Approval(
                    "no user config path; set --config before choosing a global rule".to_owned(),
                )
            })?;
        norte_code_protocol::config::update_user_approval(&path, tool, allow)?;
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
            .or_else(norte_code_protocol::config::user_config_path)
            .ok_or_else(|| {
                ServiceError::Approval(
                    "no user config path; set --config before choosing a global rule".to_owned(),
                )
            })?;
        norte_code_protocol::config::forget_user_approval(&path, tool)?;
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

    /// Rebuilds the MCP connections from the current configuration.
    ///
    /// A server added, removed or disabled in the config takes effect here: the
    /// connections are made from the configuration in force, not the one the
    /// process started with. The on/off state a person set in the panel is
    /// carried across by name, so a server they turned off is not turned back on
    /// by a refresh, and a process that is no longer configured is dropped with
    /// its children.
    ///
    /// Returns how many servers are up afterwards.
    pub async fn reload_mcp(&self) -> usize {
        let root = self.workspace_root.clone();
        let configured = self.lock_config().mcp_servers.clone();
        let was_active = self
            .mcp_servers
            .read()
            .map(|servers| servers.active_by_name())
            .unwrap_or_default();
        let mut connected = McpServers::connect(&configured, &root).await;
        for (name, reason) in &connected.failures {
            tracing::warn!(server = %name, %reason, "an MCP server could not be started");
        }
        // The state a person set wins over the default the connection has: a
        // server carried across the reload keeps whether it was on.
        for server in &mut connected.servers {
            if let Some(active) = was_active.get(&server.name) {
                server.active = *active;
            }
        }
        let up = connected.servers.len();
        if let Ok(mut slot) = self.mcp_servers.write() {
            *slot = connected;
        }
        up
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
            .and_then(|record| record.spec.as_ref().map(ModelSpec::label))
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
            .and_then(|record| record.spec.clone())
    }

    /// How much of its model's context a session is using.
    ///
    /// The used part is what the last request carried — the whole
    /// conversation goes again every time — and the window is what the
    /// catalogue says the model holds. A session that has made no request yet
    /// has used nothing.
    #[must_use]
    pub fn session_context(&self, session: &SessionKey) -> ContextUsage {
        self.lock_sessions()
            .get(session)
            .map_or_else(ContextUsage::default, |record| ContextUsage {
                used: record.context_used,
                window: match &record.spec {
                    Some(s) => s.capabilities.max_context,
                    None => None,
                },
            })
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

    /// The title a session has, if one was set.
    ///
    /// `None` means the session is still known by its key alone. The renderer
    /// reads this so the footer can show the title instead of the key.
    #[must_use]
    pub fn session_title(&self, session: &SessionKey) -> Option<String> {
        self.lock_sessions()
            .get(session)
            .and_then(|record| record.title.clone())
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

    /// The key store the host installed, when it did.
    ///
    /// A session asks it before switching to a provider that has no key, so the
    /// client can prompt for one and save it.
    #[must_use]
    pub fn key_store(&self) -> Option<Arc<dyn KeyStore>> {
        self.lock_config().keys.clone()
    }

    /// Adds a key resolved after startup to the redaction list.
    ///
    /// A provider selected through `/model` never passed `collect_secrets` at
    /// construction, and its key must still be hidden from a tool's output.
    fn remember_secret(&self, key: &ApiKey) {
        let value = key.expose().trim();
        if value.is_empty() {
            return;
        }
        let mut secrets = self
            .secrets
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if secrets.iter().any(|secret| secret == value) {
            return;
        }
        secrets.push(value.to_owned());
        secrets.sort();
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
            .map(|dir| dir.join(".norte-code").join("service.json"))
            .find(|candidate| candidate.exists());
        let current_agent = self.lock_config().name.clone();
        let service =
            norte_code_protocol::config::ServiceConfig::load(path.as_deref(), project.as_deref())?;
        let agent_profiles: BTreeMap<String, Config> = service
            .agents
            .iter()
            .cloned()
            .map(|agent| (agent.name.clone(), agent))
            .collect();
        let mut warnings = Vec::new();
        let config = service
            .agents
            .iter()
            .find(|agent| agent.name == current_agent)
            .cloned()
            .or_else(|| service.default_agent().ok())
            .ok_or_else(|| {
                ServiceError::Config(norte_code_protocol::config::ConfigError::Invalid {
                    path: "agents".to_owned(),
                    message: "no configured agent is available".to_owned(),
                })
            })?;
        // The file never carries the store; the host installed it on the live
        // config, so a reload has to carry it across or a keyring key would
        // disappear from every session the reload rebuilds.
        let mut next = config;
        next.keys.clone_from(&self.lock_config().keys);

        // Everything that can fail happens before the live service is touched.
        // The catalogue is re-read from disk, so a provider or a model added to
        // providers.json / models.json is visible at once: /reload picks up new
        // providers and models without restarting the application.
        let catalog = match load_catalog_files() {
            Ok(catalog) => {
                *self.lock_catalog_write() = Arc::new(catalog);
                self.catalog()
            }
            // A malformed catalogue file must not abort the whole reload:
            // the catalogue already in memory stays in force.
            Err(error) => {
                warnings.push(format!("catalogue did not reload: {error}"));
                self.catalog()
            }
        };
        // The provider and the model are two config keys and never a split
        // reference: the resolver takes the pair as it is, and the model keeps
        // every slash it has.
        let spec = match (&next.default_provider, &next.default_model) {
            (Some(provider), Some(model)) => {
                match self.default_spec_for(&next, &catalog, provider, model) {
                    Ok(spec) => Some(spec),
                    // A default provider that the catalogue no longer carries must
                    // not abort the whole reload: the spec already in force stays,
                    // and a new session will report its own error when it tries to
                    // open on the missing provider.
                    Err(error) => {
                        warnings.push(format!("default provider/model not in catalogue: {error}"));
                        self.default_spec()
                    }
                }
            }
            // No default: clear the spec, since a config that only changes
            // unrelated settings must still reload and there is no model to
            // open a session on.
            _ => None,
        };
        let secrets = collect_secrets(&next, &catalog);
        let skills = Arc::new(discover_skills(&next, &self.workspace_root));

        self.replace_config(next.clone());
        *self
            .agent_profiles
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = agent_profiles;
        *self
            .secrets
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = secrets;
        *self
            .default_spec
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = spec;
        *self.lock_skills_write() = skills;

        // Every open session's spec is rebuilt from the new configuration. A
        // spec is resolved once and carries the provider's settings baked into
        // it — the endpoint, the wire, the headers, whether the prompt cache key
        // is sent — so a config that changed one of them changed nothing for a
        // session already open: the record kept the spec built at startup and
        // the next turn used it. Reloading the file and still sending the field
        // a provider rejects is the worst version of this, because the person
        // did what they were told and the request was refused anyway.
        //
        // The reference is not touched: a session stays on the model it was on,
        // and only the settings around it are re-read. A model that no longer
        // resolves is left as it was rather than failing the whole reload, and
        // the turn reports its own error if it comes to that.
        {
            let models: Vec<(SessionKey, String, String)> = self
                .lock_sessions()
                .iter()
                .filter_map(|(key, record)| {
                    let spec = record.spec.as_ref()?;
                    Some((key.clone(), spec.provider.clone(), spec.model.clone()))
                })
                .collect();
            for (key, provider, model) in models {
                if let Ok(spec) = self.default_spec_for(&next, &catalog, &provider, &model)
                    && let Some(record) = self.lock_sessions().get_mut(&key)
                {
                    record.spec = Some(spec);
                }
            }
        }

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
            warnings,
        })
    }

    /// Resolves a provider and a model the way a run does.
    ///
    /// [`ModelSpec::from_catalog`] only knows the catalogue, and a configured
    /// provider is not in it: `river` and every other private endpoint is
    /// declared in `service.json` and nowhere else. A reload that went straight
    /// to the catalogue therefore failed on a default model that a run resolves
    /// without complaint — `run --provider river` worked while `/refresh` said
    /// the provider was unknown, which is the same pair answered two ways.
    ///
    /// So the catalogue is tried first and a configured provider falls back the
    /// same way the command line does. A pair that is neither — a typo, or
    /// a catalogue model that does not exist — is still an error.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the pair names neither a catalogue
    /// provider nor a configured one.
    fn default_spec_for(
        &self,
        config: &Config,
        catalog: &Catalog,
        provider: &str,
        model: &str,
    ) -> Result<ModelSpec, ServiceError> {
        // The catalogue is the only place a model comes from, so a model it
        // does not carry is not resolved from somewhere else: there is nowhere
        // else. A provider is in `providers.json` or it is unknown, and a model
        // is in `models.json` or it is unknown.
        if provider.trim().is_empty() || model.trim().is_empty() {
            return Err(ServiceError::Unsupported("no model is configured"));
        }
        let spec = ModelSpec::from_catalog(config, catalog, provider, model, None)?;
        // The key is in hand now, so it joins the redaction list even when the
        // spec is rebuilt for a session the startup pass never resolved.
        self.remember_secret(&spec.api_key);
        Ok(spec)
    }

    /// The default spec in force.
    ///
    /// The model a new session opens on, and the one `/debug` reports when it
    /// is asked before any message has named a session.
    #[must_use]
    pub fn default_spec(&self) -> Option<ModelSpec> {
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
            Kind::Steer {
                session,
                text,
                id,
                queue_mode,
            } => Kind::Steer {
                session: resolve(session)?,
                text,
                id,
                queue_mode,
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
            Kind::Resume { session, replay } => Kind::Resume {
                session: resolve(session)?,
                replay,
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
            Kind::SetModel {
                session,
                provider,
                model,
            } => Kind::SetModel {
                session: resolve(session)?,
                provider,
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
            Kind::Resume { session, replay } => {
                self.plans
                    .ensure_session_folder(&session)
                    .await
                    .map_err(ServiceError::Plan)?;
                // A resume exists to show where the conversation was.
                self.start_turn(
                    request_id,
                    Some(session),
                    None,
                    Vec::new(),
                    None,
                    None,
                    replay,
                )
                .await
            }
            Kind::CancelTurn { session } => {
                if let Some(registration) = self.lock_cancels().get(&session) {
                    registration.token.cancel();
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
            Kind::ListTelemetry { session } => {
                self.telemetry_report(request_id, session.as_ref()).await
            }
            Kind::ListTools { session } => Ok(self.tool_list_event(request_id, session.as_ref())),
            Kind::ListMcp => Ok(self.mcp_list_event(request_id)),
            Kind::RefreshMcp => {
                self.reload_mcp().await;
                Ok(self.mcp_list_event(request_id))
            }
            Kind::SetMcpActive { name, active } => {
                if self.set_mcp_active(&name, active).is_none() {
                    return Err(ServiceError::Unsupported("unknown MCP server"));
                }
                Ok(self.mcp_list_event(request_id))
            }
            Kind::SetApprovalRule {
                session,
                tool,
                allow,
                global,
            } => {
                self.set_approval_rule(session.as_ref(), &tool, allow, global)
                    .await?;
                Ok(self.tool_list_event(request_id, session.as_ref()))
            }
            Kind::ForgetApprovalRule {
                session,
                tool,
                global,
            } => {
                self.forget_approval_rule(session.as_ref(), &tool, global)
                    .await?;
                Ok(self.tool_list_event(request_id, session.as_ref()))
            }
            Kind::ListModels => Ok(self.list_models_event(request_id)),
            Kind::ListSkills => Ok(self.skill_list_event(request_id)),
            Kind::RefreshSkills => Ok(self.refresh_skills_event(request_id)),
            Kind::RefreshCatalog => self.reload_catalog(request_id),
            Kind::SetModel {
                session,
                provider,
                model,
            } => {
                self.set_model_event(request_id, session, provider, model)
                    .await
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
            Kind::AnswerQuestion {
                session,
                question,
                answer,
            } => {
                self.answer_question(request_id, session, question, answer)
                    .await
            }
            Kind::Steer {
                session,
                text,
                id,
                queue_mode,
            } => self.steer(request_id, session, text, id, queue_mode).await,
            Kind::Compact { session } => self.compact_event_stream(request_id, session).await,
            Kind::ListPlugins
            | Kind::InstallPlugin { .. }
            | Kind::UpdatePlugins { .. }
            | Kind::RemovePlugin { .. }
            | Kind::SetPluginEnabled { .. } => Ok(single(
                request_id,
                None,
                ServerEventKind::Error {
                    code: norte_code_protocol::ErrorCode::InvalidRequest,
                    message: "plugin lifecycle is managed by the RAEP host".to_owned(),
                },
            )),
        }
    }

    /// The budget decision that stopped the turn, if the layer made one.
    async fn telemetry_report(
        &self,
        request_id: u64,
        session: Option<&SessionKey>,
    ) -> Result<EventStream, ServiceError> {
        let records = self
            .directory
            .requests(session.map(ToString::to_string).as_deref())
            .await
            .map_err(|error| ServiceError::Telemetry(error.to_string()))?;
        Ok(single(
            request_id,
            session.cloned(),
            ServerEventKind::Telemetry { records },
        ))
    }

    fn tool_list_event(&self, request_id: u64, session: Option<&SessionKey>) -> EventStream {
        let tools = self
            .tool_rules(session)
            .into_iter()
            .map(|tool| norte_code_protocol::ToolInfo {
                name: tool.name,
                allowed: tool.allowed,
                scope: match tool.scope {
                    RuleScope::Session => "session",
                    RuleScope::Global => "global",
                    RuleScope::Default => "default",
                }
                .to_owned(),
            })
            .collect();
        single(
            request_id,
            session.cloned(),
            ServerEventKind::ToolList { tools },
        )
    }

    fn mcp_list_event(&self, request_id: u64) -> EventStream {
        let servers = self
            .mcp_status()
            .into_iter()
            .map(|server| norte_code_protocol::McpInfo {
                name: server.name,
                command: server.command,
                tools: server.tools,
                filtered: server.filtered,
                active: server.active,
            })
            .collect();
        let failures = self
            .mcp_failures()
            .into_iter()
            .map(|(name, reason)| norte_code_protocol::McpFailure { name, reason })
            .collect();
        single(
            request_id,
            None,
            ServerEventKind::McpList { servers, failures },
        )
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
        queue_mode: norte_code_protocol::QueueMode,
    ) -> Result<EventStream, ServiceError> {
        // The message takes the request already in flight. The durable copy
        // stays in the session until the engine reports that it applied it, so
        // a crash cannot lose it.
        let (nid, _, _) = self.require_record(&key)?;
        let busy = self.turn_lock(&key).try_lock().is_err();
        match queue_mode {
            norte_code_protocol::QueueMode::Reject if busy => {
                return Err(ServiceError::SessionBusy);
            }
            norte_code_protocol::QueueMode::Interrupt if busy => {
                self.cancel_turn(&key);
            }
            norte_code_protocol::QueueMode::Reject
            | norte_code_protocol::QueueMode::Queue
            | norte_code_protocol::QueueMode::Interrupt => {}
        }
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
            if record.pending_steers.len() >= self.max_queued_inputs {
                return Err(ServiceError::SessionBusy);
            }
            record.pending_steers.push(steered.clone());
        }
        self.persist(&key).await?;
        self.steering.push(nid, steered);
        Ok(Box::pin(futures_util::stream::empty()))
    }

    pub(super) fn lock_cancels(&self) -> MutexGuard<'_, BTreeMap<SessionKey, CancelRegistration>> {
        self.cancels.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn register_cancel(&self, key: SessionKey, token: CancellationToken) -> u64 {
        let generation = self.next_cancel_generation.fetch_add(1, Ordering::Relaxed);
        self.lock_cancels()
            .insert(key, CancelRegistration { generation, token });
        generation
    }

    pub(super) fn remove_cancel(&self, key: &SessionKey, generation: u64) {
        let mut cancels = self.lock_cancels();
        if cancels
            .get(key)
            .is_some_and(|registration| registration.generation == generation)
        {
            cancels.remove(key);
        }
    }

    /// Cancels the turn for `session`, if one is running. Returns whether a
    /// token was found and signalled.
    pub fn cancel_turn(&self, session: &SessionKey) -> bool {
        let cancels = self.lock_cancels();
        if let Some(registration) = cancels.get(session) {
            registration.token.cancel();
            true
        } else {
            false
        }
    }

    fn lock_catalog(&self) -> RwLockReadGuard<'_, Arc<Catalog>> {
        self.catalog.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_catalog_write(&self) -> RwLockWriteGuard<'_, Arc<Catalog>> {
        self.catalog.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_plugin_catalogue(&self) -> RwLockReadGuard<'_, (Providers, Models)> {
        self.plugin_catalogue
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_plugin_catalogue_write(&self) -> RwLockWriteGuard<'_, (Providers, Models)> {
        self.plugin_catalogue
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_skills(&self) -> RwLockReadGuard<'_, Arc<SkillRegistry>> {
        self.skills.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_skills_write(&self) -> RwLockWriteGuard<'_, Arc<SkillRegistry>> {
        self.skills.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Installs the runner the `subagent` tool asks, once the service
    /// exists as an `Arc`.
    ///
    /// `tools` is a `&self` method and cannot produce the `Arc<Self>` the
    /// runner needs; the host that builds the service through
    /// `Service::new` (which returns the service) installs one here.
    pub fn install_subagent_runner(self: &Arc<Self>) {
        *self
            .subagent_runner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::new(SubagentRunnerImpl::new(Some(Arc::clone(self)))) as _);
        *self
            .job_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(self));
    }

    /// Schedules an auto-turn after a detached job reports its result.
    pub(super) fn schedule_auto_turn(self: &Arc<Self>, session: norte::SessionId) {
        if !self.config().tools.auto_turn_on_job_end {
            return;
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service.auto_turn_on_job_end(session).await;
        });
    }

    /// The next request id for an auto-turn.
    fn next_auto_request_id(&self) -> u64 {
        self.auto_request_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Starts a turn on its own because a job ended with a result to read.
    ///
    /// The `SessionId` is the same one the tool calls got so the surface and
    /// the service, then map it back to the key and check whether it is worth
    /// starting: the flag is on, the session is idle, and no other auto-turn
    /// is running for it.
    ///
    /// `start_turn` with `input: None` only, and then drain the events —
    /// the client sees the turn happen through them.
    pub(super) async fn auto_turn_on_job_end(self: &Arc<Self>, session: norte::SessionId) {
        if !self.config().tools.auto_turn_on_job_end {
            return;
        }
        let key = {
            let sessions = self.lock_sessions();
            sessions
                .iter()
                .find(|(_, record)| record.id == session)
                .map(|(key, _)| key.clone())
        };
        let Some(key) = key else {
            return;
        };
        {
            let sessions = self.lock_sessions();
            let is_idle = sessions
                .get(&key)
                .is_some_and(|record| record.phase == Phase::Idle);
            if !is_idle {
                return;
            }
        }
        {
            let mut auto_turns = self
                .auto_turns
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !auto_turns.insert(key.clone()) {
                // An auto-turn for this session is already in progress.
                return;
            }
        }
        let request_id = self.next_auto_request_id();
        match self
            .start_turn(
                request_id,
                Some(key.clone()),
                None,
                Vec::new(),
                None,
                None,
                false,
            )
            .await
        {
            Ok(mut stream) => while stream.next().await.is_some() {},
            Err(error) => {
                tracing::warn!(session = %key, %error, "auto-turn could not start");
            }
        }
        self.auto_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key);
    }
}

/// Runs a subagent's turn on a child session.
///
/// The child is created with the same rules as any other session, except
/// its tool list is frozen without `subagent`: it starts blank and cannot
/// spawn another one, so the depth is one and the recursion is impossible.
struct SubagentRunnerImpl {
    service: Option<Arc<Service>>,
}

impl SubagentRunnerImpl {
    fn new(service: Option<Arc<Service>>) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl SubagentRunner for SubagentRunnerImpl {
    #[allow(clippy::too_many_lines)]
    async fn run(
        &self,
        prompt: &str,
        parent: norte::SessionId,
        label: &str,
        cancel: &norte::Cancellation,
    ) -> SubagentOutcome {
        let _ = label;
        let Some(service) = self.service.clone() else {
            return SubagentOutcome {
                text: "subagent: no runner installed".to_owned(),
                is_error: true,
            };
        };
        // The parent's key lets the service resolve the configured child
        // model and tool policy. The session id is mapped back to that key.
        let parent_key = {
            let sessions = service.lock_sessions();
            sessions
                .iter()
                .find(|(_, record)| record.id == parent)
                .map(|(key, _)| key.clone())
        };
        let Some(parent_key) = parent_key else {
            return SubagentOutcome {
                text: "subagent: parent session is unknown".to_owned(),
                is_error: true,
            };
        };
        let (child_spec, child_disabled, child_budget, child_max_iterations) =
            match service.subagent_settings(&parent_key).await {
                Ok(settings) => settings,
                Err(error) => {
                    return SubagentOutcome {
                        text: format!("subagent: could not resolve child settings: {error}"),
                        is_error: true,
                    };
                }
            };
        let engine_budget = Arc::clone(&child_budget);
        let child_key = match service
            .create_session_child(child_spec, child_disabled, child_budget)
            .await
        {
            Ok(key) => key,
            Err(error) => {
                return SubagentOutcome {
                    text: format!("subagent: could not create child session: {error}"),
                    is_error: true,
                };
            }
        };
        let (nid, spec, _) = match service.require_record(&child_key) {
            Ok(found) => found,
            Err(error) => {
                return SubagentOutcome {
                    text: format!("subagent: child session is unknown: {error}"),
                    is_error: true,
                };
            }
        };
        let Some(spec) = spec else {
            return SubagentOutcome {
                text: "subagent: no model is configured".to_owned(),
                is_error: true,
            };
        };
        let engine = match service.engine_with_budget(
            &spec,
            Some(&child_key),
            Some(engine_budget),
            None,
            child_max_iterations,
        ) {
            Ok(engine) => engine,
            Err(error) => {
                return SubagentOutcome {
                    text: format!("subagent: could not build engine: {error}"),
                    is_error: true,
                };
            }
        };
        // One turn on the child. Cancellation is the job's own token: a
        // `/jobs stop` cancels this `cancel`, and the turn builder takes it.
        let token = tokio_util::sync::CancellationToken::new();
        if cancel.is_cancelled() {
            token.cancel();
        }
        let child = engine
            .turn(nid)
            .input(norte::Message::user(prompt))
            .cancellation(token)
            .start();
        let outcome = child.finish().await;
        match outcome {
            Ok(outcome) => match outcome.message {
                Some(message) => {
                    let text = message
                        .parts
                        .iter()
                        .filter_map(|part| match part {
                            norte::Part::Text(text) => Some(text.text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let text = if text.trim().is_empty() {
                        "(the child said nothing)".to_owned()
                    } else {
                        text
                    };
                    SubagentOutcome {
                        text,
                        is_error: false,
                    }
                }
                None => SubagentOutcome {
                    text: "(the child committed no message)".to_owned(),
                    is_error: false,
                },
            },
            Err(error) => SubagentOutcome {
                text: error.to_string(),
                is_error: true,
            },
        }
    }
}

#[cfg(test)]
mod tests;
