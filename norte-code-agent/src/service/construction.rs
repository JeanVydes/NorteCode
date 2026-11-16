//! Building a service and the engines it runs turns with.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use norte::http::{HttpClient, HttpMedia};
use norte::{BlobStore, Layer, LayerStack, MediaAdapter, Model};
use norte_code_protocol::catalog::Catalog;
use norte_code_protocol::config::{CompactionTrigger, Config};
use norte_code_protocol::costs::BudgetBook;
use norte_code_protocol::sqlite::{InMemoryDirectory, SessionDirectory};
use norte_code_protocol::tools::{QuestionBroker, TodoState};
use norte_code_wire::catalog::{Models, Providers};
use norte_code_wire::{ServerEvent, SessionKey};

use crate::provider::{ModelFactory, ModelSpec};
use crate::runtime::{
    Harness, HarnessBuilder, HarnessConfig, InMemoryBlobStore, Store, TurnInterceptor,
};
use crate::telemetry::{Telemetry, TelemetryLayer, TelemetryObserver};
use crate::{DEFAULT_SUBAGENT_SYSTEM_PROMPT, ServiceError};

use super::Service;
use super::approvals::{ApprovalRules, ToolRuleInterceptor};
use super::compaction::CompactionGate;
use super::compaction::CompactionLayer;
use super::delegator;
use super::detach::DetachRegistry;
use super::effects::EffectJournal;
use super::jobs::JobRegistry;
use super::mapping::BudgetLayer;
use super::mcp::McpServers;
use super::plans::PlanRepository;
use super::prompt::{compose_system_prompt, load_project_instructions};
use super::questions::DirectoryQuestions;
use super::screenshots::{ScreenshotInterceptor, Screenshots};
use super::skills::{collect_secrets, discover_skills};
use super::steering::{Steering, SteeringInterceptor};
use super::tooling::{ToolActivity, edit_source};

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
    /// A client that offers a startup choice: begin a new session, continue the
    /// last one, pick one from a list: needs the session to exist *before* the
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
    pub fn sessions(&self) -> Vec<norte_code_wire::SessionInfo> {
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
        // The delegation channel is built before the struct so the receiver
        // can be stored beside the sender: the host takes it once at startup
        // and spawns the drain that turns these events into RAEP frames.
        let (delegation_sender, delegation_receiver) =
            tokio::sync::broadcast::channel::<ServerEvent>(256);
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
            children: Arc::new(delegator::ChildRegistry::new()),
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
            delegation_events: Arc::new(delegation_sender),
            delegation_rx: Arc::new(Mutex::new(Some(delegation_receiver))),
            job_host: Arc::new(RwLock::new(None)),
            auto_turns: Mutex::new(BTreeSet::new()),
            active_turns: Mutex::new(BTreeMap::new()),
            job_auto_turns: Mutex::new(HashMap::new()),
            job_auto_turn_changed: tokio::sync::Notify::new(),
            job_turn_policies: Mutex::new(HashMap::new()),
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
    pub(super) fn engine(
        &self,
        spec: &ModelSpec,
        key: Option<&SessionKey>,
    ) -> Result<Harness, ServiceError> {
        self.engine_with_budget(spec, key, None, None, None)
    }

    /// Builds an engine whose requests pass through the budget and telemetry
    /// layers, plus an optional eager-compaction layer.
    pub(super) fn engine_with_budget(
        &self,
        spec: &ModelSpec,
        key: Option<&SessionKey>,
        budget: Option<Arc<Mutex<BudgetBook>>>,
        compaction: Option<Arc<CompactionGate>>,
        max_iterations: Option<usize>,
    ) -> Result<Harness, ServiceError> {
        let model: Arc<dyn Model> = self.factory.build(spec, &spec.capabilities)?;
        // The adapter is what turns a `Blob` back into bytes on the way out. A
        // message may carry media the client stored: a pasted image: and the
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
                    .or_else(|| {
                        key.and_then(|session| self.profile_for_session(session).max_iterations)
                    })
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
                // Read from the spec the engine was built with: never
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
    pub(super) fn compaction_gate(&self, spec: &ModelSpec) -> Option<Arc<CompactionGate>> {
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
}
