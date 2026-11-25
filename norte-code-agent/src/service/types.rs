//! Public service result types and the service's live counters.

use std::pin::Pin;

use futures_core::Stream;
use norte_code_protocol::config::Config;
use norte_code_wire::ServerEvent;
use tokio_util::sync::CancellationToken;

/// A stream of events answering one request.
pub type EventStream = Pin<Box<dyn Stream<Item = ServerEvent> + Send>>;

/// Where a steering message went.
pub enum SteerOutcome {
    /// The message joined the turn already in flight; the events it produces
    /// surface on that turn's stream.
    Injected,
    /// No turn was running, so the message started one and the stream carries
    /// its events.
    Started(EventStream),
}

impl std::fmt::Debug for SteerOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Injected => formatter.write_str("Injected"),
            Self::Started(_) => formatter.write_str("Started(..)"),
        }
    }
}

/// A session as the service tracks it, beyond what Norte stores.
/// Live counters for the turn in flight.
#[derive(Debug, Clone, Default)]
pub(super) struct Stats {
    pub(super) turn: u64,
    pub(super) billed: usize,
    pub(super) requests: u64,
    pub(super) cost: f64,
    pub(super) unpriced: u64,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) cache_write_tokens: u64,
    /// The input tokens of the last request, which is the context.
    ///
    /// The whole conversation is sent again every time, so the last request's
    /// input tokens are what the context holds: not the sum, which is what
    /// was spent.
    pub(super) context_used: u64,
    /// The request telemetry that has not been matched to a committed message
    /// yet.
    ///
    /// The telemetry arrives when a provider answers and the message is
    /// committed after that; a settle that runs in between keeps the telemetry
    /// here instead of dropping it, so the request that follows is measured
    /// with its own numbers.
    pub(super) pending: std::collections::VecDeque<norte_code_protocol::sqlite::RequestRecord>,
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

/// How long a cancelled turn has to close itself before the service closes it.
pub(super) const CANCEL_CLOSE_GRACE: u64 = 3;

/// The reason a cancelled turn reports when the service closes it.
pub(super) const STOP_REASON_CANCELLED: &str = "cancelled";

/// A role accepted when restoring portable history.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportedHistoryRole {
    /// A user message.
    User,
    /// An assistant message.
    Assistant,
}

/// One plain-text message restored from a portable RAEP history.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ImportedHistoryItem {
    /// The original RAEP role. Portable imports preserve `user` and
    /// `assistant` rather than promoting untrusted tool or system content.
    pub role: ImportedHistoryRole,
    /// Text content from the history item.
    pub content: String,
    /// Original turn identifier, when present.
    pub turn_id: Option<String>,
    /// Original timestamp in Unix milliseconds, when present.
    pub timestamp: Option<i64>,
    /// Original provenance retained as node metadata.
    pub origin: Option<serde_json::Value>,
}
