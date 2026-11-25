//! The in-process service: it owns the Norte engines, maps the client protocol
//! onto turns, and streams the engine's events back as [`ServerEvent`]s.
//!
//! An engine is cheap and shares the store and blob store, so each turn builds
//! one for the model the session selected. That is what makes a mid-session
//! model switch a change of model rather than a change of session.

mod approvals;
mod background;
mod budgets;
mod cancellation;
mod compaction;
mod config_state;
mod construction;
mod data_clear;
mod delegation;
mod delegation_settings;
mod delegator;
mod detach;
mod dispatch;
mod effects;
mod environment;
mod job_control;
mod jobs;
mod locks;
mod manage;
mod mapping;
mod mcp;
mod mcp_control;
mod media_wire;
mod models;
pub use models::ReasoningError;
mod plans;
pub use plans::PlanError;
pub mod pins;
mod plugin_message_interceptor;
mod plugin_message_provenance;
mod plugin_messages;
mod prompt;
mod questions;
mod sandbox;
mod session_children;
mod session_export;
mod session_lifecycle;
mod session_record;
mod session_rewind;
mod session_state;
mod session_stats;
mod session_titles;
mod skill_catalog;
mod skills;
mod steering;
mod subagent_runner;
mod subagent_views;
mod subagents;
mod tool_rules;
mod tooling;
mod turn_context;
mod turn_events;
mod turn_helpers;
mod turn_history;
mod turn_media;
mod turn_run;
mod turn_start;
mod turn_telemetry;
mod types;
mod workspace;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::runtime::Store;
use crate::runtime::TurnInterceptor;
use futures_util::StreamExt;
use norte::BlobStore;
use norte_code_protocol::costs::{BudgetBook, CostRecord};
use norte_code_protocol::skills::SkillRegistry;
use norte_code_protocol::sqlite::{SessionDirectory, StoredSession};
use norte_code_protocol::tools::{QuestionBroker, TodoState};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use norte_code_protocol::catalog::Catalog;
use norte_code_protocol::config::Config;
use norte_code_wire::catalog::{Models, Providers};
use norte_code_wire::{JobTurnPolicy, Phase, ServerEvent, ServerEventKind, SessionKey};

use crate::ServiceError;
use crate::provider::{ModelFactory, ModelSpec};
use crate::telemetry::Telemetry;

use self::session_record::SessionRecord;
use self::steering::{Steered, Steering};
use self::subagents::SubagentRunner;
use approvals::ApprovalRules;
use delegator::SharedChildren;
use detach::DetachRegistry;
use effects::EffectJournal;
use jobs::{JobRegistry, job_info};
use mapping::single;
use mcp::McpServers;
use models::resolve_reasoning;
use plans::PlanRepository;
use plugin_messages::PluginMessages;
use tooling::ToolActivity;

pub use approvals::{RuleScope, ToolStatus};
#[cfg(test)]
use delegation::job_policy_allows_turn;
pub use models::{effective_reasoning, model_list};
#[cfg(test)]
use norte_code_wire::Reasoning;
pub use prompt::PromptSize;
use types::{CANCEL_CLOSE_GRACE, CancelRegistration, STOP_REASON_CANCELLED, Stats};
pub use types::{
    EventStream, ImportedHistoryItem, ImportedHistoryRole, McpServerStatus, ReloadedConfig,
    SteerOutcome,
};

/// The Norte Code service.
pub struct Service {
    /// How many inputs may wait for the turn in flight (RFC 12.1:
    /// `limits.max_queued_inputs`, default 8).
    ///
    /// One more is refused with `SessionBusy` (`ERROR SESSION_BUSY` on the
    /// wire) instead of growing `pending_steers` without bound.
    max_queued_inputs: AtomicUsize,
    config: RwLock<Config>,
    agent_profiles: RwLock<BTreeMap<String, Config>>,
    catalog: RwLock<Arc<Catalog>>,
    plugin_catalogue: RwLock<(Providers, Models)>,
    /// Tools contributed by server-loaded plugins, registered per engine.
    plugin_tools: RwLock<Vec<Arc<dyn norte::AnyFunction>>>,
    /// Which plugin declared each plugin tool, by tool name.
    plugin_tool_origins: RwLock<BTreeMap<String, String>>,
    /// Plugin message drafts accepted from tool results and waiting to be applied.
    plugin_messages: Arc<PluginMessages>,
    default_spec: RwLock<Option<ModelSpec>>,
    store: Arc<dyn Store>,
    blobs: Arc<dyn BlobStore>,
    factory: Arc<dyn ModelFactory>,
    /// A fixed model for title requests in tests. Production builds the title
    /// model from the session's own spec.
    title_model: Option<Arc<dyn norte::Model>>,
    system_prompt: String,
    subagent_system_prompt: String,
    project_path: Option<PathBuf>,
    project_chars: usize,
    workspace_root: PathBuf,
    skills: RwLock<Arc<SkillRegistry>>,
    todo: Arc<TodoState>,
    questions: Arc<QuestionBroker>,
    /// The children the service has spawned, by parent (RFC 8.2).
    children: SharedChildren,
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
    /// Delegation events outbound to clients (RFC 8.2.10): the RAEP host
    /// subscribes and emits the `CHILD_SESSION_*` frames on the parent's
    /// stream. Events carry no request id (0); the host only reads `kind`
    /// and `session`.
    delegation_events: Arc<tokio::sync::broadcast::Sender<ServerEvent>>,
    delegation_rx: Arc<Mutex<Option<tokio::sync::broadcast::Receiver<ServerEvent>>>>,
    /// The service reference the shell calls hold, so a finished job can
    /// start a turn of its own when the flag is on. Set the same way the
    /// subagent runner is.
    job_host: Arc<RwLock<Option<Arc<Service>>>>,
    /// Sessions with an auto-turn already running, so a second job end for the
    /// same session does not start a duplicate one at the same time.
    auto_turns: Mutex<BTreeSet<SessionKey>>,
    /// The turn currently running for each session, when one has started.
    active_turns: Mutex<BTreeMap<SessionKey, u64>>,
    /// Jobs whose terminal result reserved a new turn.
    job_auto_turns: Mutex<HashMap<String, Option<u64>>>,
    job_auto_turn_changed: tokio::sync::Notify,
    /// Per-session job turn policies. Unset sessions use the configured default.
    job_turn_policies: Mutex<HashMap<norte::SessionId, JobTurnPolicy>>,
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

/// A child session spawned by a parent session.
#[derive(Clone, Debug)]
pub struct ChildSessionSummary {
    /// The child session's RAEP identifier.
    pub session: SessionKey,
    /// The title assigned to the child, if any.
    pub title: Option<String>,
    /// The child's current runner state.
    pub state: ChildSessionState,
}

/// The current state of a child session runner.
///
/// It is the protocol's own closed set, so the runner and the event stream
/// cannot disagree about what a child is doing.
pub type ChildSessionState = norte_code_wire::ChildState;

/// Whether a name is in a session's withheld list.
///
/// The comparison is exact: a name is what the model calls a tool, and a prefix
/// that matched: `read` for `read_file`: would take away a tool nobody named.
fn withheld(disabled: &[String], name: &str) -> bool {
    disabled.iter().any(|entry| entry == name)
}

impl Service {
    pub(super) fn profile_for_session(&self, key: &SessionKey) -> Config {
        let profile_id = self
            .lock_sessions()
            .get(key)
            .and_then(|record| record.agent_profile.clone());
        profile_id
            .and_then(|id| {
                self.agent_profiles
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&id)
                    .cloned()
            })
            .unwrap_or_else(|| self.config())
    }

    /// Returns the gate that serializes turn starts for one session.
    pub(super) fn turn_lock(&self, key: &SessionKey) -> Arc<AsyncMutex<()>> {
        self.turn_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// Measures the static prompt material: the system prompt and the tool
    /// declarations.
    ///
    /// # Errors
    ///
    /// Returns the tool-declaration error when the registry cannot build them.
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

    /// Replaces the configuration.
    ///
    /// The caller rebuilds what was derived from it; this only swaps the value.
    pub(super) fn replace_config(&self, next: Config) {
        *self
            .config
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    }

    /// The config lock, for reads that must not copy the whole value.
    pub(super) fn lock_config(&self) -> std::sync::RwLockReadGuard<'_, Config> {
        self.config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
#[path = "plugin_messages_tests.rs"]
mod plugin_messages_tests;
#[cfg(test)]
mod tests;
