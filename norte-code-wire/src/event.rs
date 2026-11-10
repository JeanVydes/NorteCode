//! The canonical event tags the RAEP adapter writes.
//!
//! Every Norte Code payload carries its name in an `event` field, and the
//! catalogue and service-data families carry a second `kind`. The host names
//! them with a serde-tagged enum; clients used to compare those names as
//! strings, so a renamed or misspelled tag silently matched nothing and an
//! empty list looked like an empty catalogue. This is the closed set both
//! sides compile against: a new tag is a variant here, and a client that
//! forgets it fails to compile instead of rendering nothing.
//!
//! Decoding ignores the rest of the payload, so the same enum serves as a
//! discriminator: read `event`/`kind` from a frame, match, and then decode the
//! body into the typed payload for that family.

use serde::{Deserialize, Serialize};

/// What kind of catalogue an `agent_catalog` payload carries.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogKind {
    /// The model catalogue.
    Models,
    /// The installed Agent Skills.
    Skills,
    /// The editable providers catalogue.
    Providers,
    /// The session's plans.
    Plans,
}

/// What kind of data a `service_data` payload carries.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceDataKind {
    /// Request telemetry rows.
    Telemetry,
    /// The tools and the rules deciding whether each may run.
    ToolList,
    /// The configured MCP servers and their tools.
    McpList,
}

/// The name of a canonical Norte Code payload.
///
/// The variants mirror the host's own tags one for one; the ones that carry a
/// second field keep it, so `agent_catalog` is never matched without its
/// `kind`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CanonicalEvent {
    /// Norte Code service-specific data.
    ServiceData {
        /// Which family of data.
        kind: ServiceDataKind,
    },
    /// The catalogue the agent can offer.
    AgentCatalog {
        /// Which catalogue.
        kind: CatalogKind,
    },
    /// The plugins installed on the host.
    PluginList,
    /// A plugin lifecycle operation completed.
    PluginChanged,
    /// A session was created.
    SessionCreated,
    /// The current session list.
    SessionList,
    /// A session was deleted.
    SessionDeleted,
    /// A session was renamed.
    SessionRenamed,
    /// A session was forked.
    SessionForked,
    /// A turn began.
    TurnStarted,
    /// A model token delta.
    TokenDelta,
    /// A committed turn.
    Committed,
    /// A replayed history message.
    HistoryMessage,
    /// The current session todos.
    SessionTodos,
    /// A tool call began.
    ToolCallStarted,
    /// A detached call's output.
    JobProgressDelta,
    /// A tool call ended.
    ToolCallFinished,
    /// The jobs the session knows about.
    JobList,
    /// Jobs were stopped.
    JobsStopped,
    /// A detached tool call became a job.
    JobStarted,
    /// A detached job wrote a chunk of output.
    DetachedJobProgress,
    /// A background job reached a terminal state.
    JobFinished,
    /// A question requiring client input.
    QuestionAsked,
    /// An approval or question was resolved.
    ApprovalResolved,
    /// A turn changed lifecycle state.
    TurnStateChanged,
    /// One model request finished.
    RequestCompleted,
    /// Generated media was collected for the output store.
    MediaPartCompleted,
    /// A context compaction lifecycle update.
    ContextUpdate,
    /// A budget warning.
    BudgetWarning,
    /// A budget was exhausted.
    BudgetExceeded,
    /// The session budget changed.
    BudgetChanged,
    /// Session usage totals.
    SessionStats,
    /// Provider-reported cost was updated.
    CostUpdated,
    /// The provider reported no cost.
    CostNotReported,
    /// Input was accepted, or steering arrived.
    UserInputAccepted,
    /// The agent that serves the session changed.
    AgentChanged,
    /// A provider retry was scheduled or started.
    ProviderRetry,
    /// A request failed.
    Error,
    /// A session export is ready to download.
    SessionExportReady,
    /// A portable import finished.
    ImportResult,
    /// A child session started.
    ChildSessionStarted,
    /// A child session changed state.
    ChildSessionUpdated,
    /// A child's delegator changed state.
    DelegatorStateChanged,
    /// A child waits for its delegator's answer.
    ChildAwaitingInput,
    /// One page of history.
    HistoryPage,
    /// History was rewound to a turn.
    HistoryRewound,
    /// A non-text output part started.
    OutputPartStarted,
    /// A non-text output part finished.
    OutputPartFinished,
}

impl CanonicalEvent {
    /// Reads the tag from a payload, or `None` when the frame is not one of
    /// these events.
    #[must_use]
    pub fn decode(payload: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(payload.clone()).ok()
    }

    /// Whether this payload is the event `self` names.
    #[must_use]
    pub fn matches(&self, payload: &serde_json::Value) -> bool {
        Self::decode(payload).is_some_and(|event| &event == self)
    }

    /// The model catalogue.
    #[must_use]
    pub const fn agent_models() -> Self {
        Self::AgentCatalog {
            kind: CatalogKind::Models,
        }
    }

    /// The installed Agent Skills.
    #[must_use]
    pub const fn agent_skills() -> Self {
        Self::AgentCatalog {
            kind: CatalogKind::Skills,
        }
    }

    /// The editable providers catalogue.
    #[must_use]
    pub const fn catalog_providers() -> Self {
        Self::AgentCatalog {
            kind: CatalogKind::Providers,
        }
    }

    /// The session's plans.
    #[must_use]
    pub const fn plan_list() -> Self {
        Self::AgentCatalog {
            kind: CatalogKind::Plans,
        }
    }

    /// The tools and their rules.
    #[must_use]
    pub const fn tool_list() -> Self {
        Self::ServiceData {
            kind: ServiceDataKind::ToolList,
        }
    }

    /// The MCP servers and their tools.
    #[must_use]
    pub const fn mcp_list() -> Self {
        Self::ServiceData {
            kind: ServiceDataKind::McpList,
        }
    }
}

#[cfg(test)]
#[path = "event_tests.rs"]
mod tests;
