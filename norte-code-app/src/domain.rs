use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    #[serde(rename = "transport.ws")]
    WebSocket,
    #[serde(rename = "transport.sse")]
    ServerSentEvents,
    Resume,
    History,
    Uploads,
    #[serde(rename = "output.parts")]
    OutputParts,
    #[serde(rename = "X_NORTECODE_JOBS_BASIC")]
    JobsBasic,
    #[serde(rename = "X_NORTECODE_FORK_TEXT")]
    ForkText,
    #[serde(rename = "X_NORTECODE_COMPACTION_BASIC")]
    CompactionBasic,
    #[serde(rename = "X_NORTECODE_DELEGATION_BASIC")]
    DelegationBasic,
    Delegation,
    #[default]
    #[serde(other)]
    Other,
}

impl Capability {
    /// Reads a capability the server advertises.
    #[must_use]
    pub fn from_wire(capability: &str) -> Self {
        match capability {
            "transport.ws" => Self::WebSocket,
            "transport.sse" => Self::ServerSentEvents,
            "resume" => Self::Resume,
            "history" => Self::History,
            "uploads" => Self::Uploads,
            "output.parts" => Self::OutputParts,
            "X_NORTECODE_JOBS_BASIC" => Self::JobsBasic,
            "X_NORTECODE_FORK_TEXT" => Self::ForkText,
            "X_NORTECODE_COMPACTION_BASIC" => Self::CompactionBasic,
            "X_NORTECODE_DELEGATION_BASIC" => Self::DelegationBasic,
            "delegation" => Self::Delegation,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::WebSocket => "WebSocket transport",
            Self::ServerSentEvents => "Server-sent events",
            Self::Resume => "Session resume",
            Self::History => "History",
            Self::Uploads => "Uploads",
            Self::OutputParts => "Multimedia output",
            Self::JobsBasic => "Basic jobs",
            Self::ForkText => "Text session forks",
            Self::CompactionBasic => "Basic compaction",
            Self::DelegationBasic => "Basic child-session inspection",
            Self::Delegation => "Delegation",
            Self::Other => "Other advertised capability",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Overview,
    Sessions,
    Chat,
    Providers,
    Plugins,
    HostCapabilities,
    Costs,
    Telemetry,
    Settings,
    Developer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn css_class(self) -> &'static str {
        match self {
            Self::System => "theme-system",
            Self::Light => "theme-light",
            Self::Dark => "theme-dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

impl ConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected | Self::Failed => "Offline",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
        }
    }

    /// The sidebar dot: a small state mark instead of a word.
    pub fn dot_class(self) -> &'static str {
        match self {
            Self::Disconnected => "server-dot server-dot-offline",
            Self::Connecting => "server-dot server-dot-pending",
            Self::Connected => "server-dot server-dot-online",
            Self::Failed => "server-dot server-dot-error",
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            Self::Disconnected => "connection connection-offline",
            Self::Connecting => "connection connection-pending",
            Self::Connected => "connection connection-online",
            Self::Failed => "connection connection-error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerProfile {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub bearer_token: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConversationItem {
    pub id: String,
    pub turn_id: Option<String>,
    pub role: HistoryRole,
    pub kind: HistoryKind,
    pub content: String,
    pub created_at: i64,
    pub delivery: MessageDelivery,
    pub tool: Option<ToolActivity>,
    /// What the turn that produced this assistant answer cost and how fast
    /// it ran. Rendered as a compact footer line under the answer.
    pub usage: Option<TurnUsage>,
}

/// The numbers a completed turn reported, shown under the assistant answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: Option<f64>,
    pub duration_ms: u64,
    pub first_token_ms: Option<u64>,
    pub tokens_per_second: Option<f64>,
    pub cache_hit: bool,
    /// The provider that answered, when the host reported it.
    pub provider: Option<String>,
    /// The model that answered, when the host reported it.
    pub model: Option<String>,
}

/// Folds one request's usage into the answer it belongs to.
///
/// The numbers go on the newest assistant row of that turn; when the turn
/// produced no text (a tool-only request) a row is created to hold them, so
/// the cost is never silently dropped.
pub fn fold_usage(
    items: &mut Vec<ConversationItem>,
    turn_id: Option<String>,
    usage: &TurnUsage,
    created_at: i64,
) {
    let target = items.iter_mut().rev().find(|item| {
        item.kind == HistoryKind::Assistant
            && item.turn_id.is_some()
            && (turn_id.is_none() || item.turn_id == turn_id)
    });
    if let Some(item) = target {
        match &mut item.usage {
            Some(existing) => existing.merge(usage),
            slot => *slot = Some(usage.clone()),
        }
        return;
    }
    items.push(ConversationItem {
        id: format!("usage:{}", turn_id.as_deref().unwrap_or("turn")),
        turn_id,
        role: HistoryRole::Assistant,
        kind: HistoryKind::Assistant,
        content: String::new(),
        created_at,
        delivery: MessageDelivery::Sent,
        tool: None,
        usage: Some(usage.clone()),
    });
}

impl TurnUsage {
    /// `openrouter / stealth/crouton-alpha`, when the host named them.
    pub fn model_label(&self) -> Option<String> {
        match (&self.provider, &self.model) {
            (Some(provider), Some(model)) => Some(format!("{provider} / {model}")),
            (Some(provider), None) => Some(provider.clone()),
            (None, Some(model)) => Some(model.clone()),
            _ => None,
        }
    }

    pub fn merge(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.cost_usd = match (self.cost_usd, other.cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        };
        self.duration_ms = self.duration_ms.saturating_add(other.duration_ms);
        if self.first_token_ms.is_none() {
            self.first_token_ms = other.first_token_ms;
        }
        if let Some(tps) = other.tokens_per_second {
            self.tokens_per_second = Some(tps);
        }
        self.cache_hit |= other.cache_hit;
    }

    /// `12.3s · 48 tok/s · TTFT 320ms`, the parts the provider reported.
    pub fn performance_label(&self) -> String {
        let mut parts = Vec::new();
        if self.duration_ms > 0 {
            parts.push(format_duration(self.duration_ms));
        }
        if let Some(tps) = self.tokens_per_second {
            parts.push(format!("{tps:.0} tok/s"));
        }
        if let Some(ttft) = self.first_token_ms {
            parts.push(format!("TTFT {ttft} ms"));
        }
        parts.join(" · ")
    }

    /// `1.2k in · 340 out`, the token counts, cache included when present.
    pub fn token_label(&self) -> String {
        let mut text = format!(
            "{} in · {} out",
            human_count(self.input_tokens),
            human_count(self.output_tokens)
        );
        if self.cache_read_tokens > 0 {
            let _ = std::fmt::Write::write_fmt(
                &mut text,
                format_args!(" · {} cached", human_count(self.cache_read_tokens)),
            );
        }
        text
    }
}

/// A token count a person can read: `947`, `12.3k`, `1.2m`, `3.4b`.
pub fn human_count(value: u64) -> String {
    const K: u64 = 1_000;
    const M: u64 = 1_000_000;
    const B: u64 = 1_000_000_000;
    match value {
        v if v >= B => format!("{}.{}b", v / B, (v % B) / (B / 10)),
        v if v >= M => format!("{}.{}m", v / M, (v % M) / (M / 10)),
        v if v >= K => format!("{}.{}k", v / K, (v % K) / (K / 10)),
        v => v.to_string(),
    }
}

fn format_duration(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1_000)
    } else if ms >= 1_000 {
        format!("{}.{}s", ms / 1_000, (ms % 1_000) / 100)
    } else {
        format!("{ms} ms")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageDelivery {
    #[default]
    Sent,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolActivity {
    pub call_id: String,
    pub name: String,
    pub path: Option<String>,
    pub arguments: serde_json::Value,
    pub output: String,
    pub status: ToolActivityStatus,
    pub duration_ms: Option<u64>,
    pub result_summary: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolActivityStatus {
    #[default]
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTodo {
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingQuestion {
    pub id: String,
    pub turn_id: u64,
    pub prompt: String,
    pub options: Vec<String>,
    pub multiple: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

pub fn todos_from_history(items: &[ConversationItem]) -> Option<Vec<SessionTodo>> {
    let mut latest = None;
    for item in items
        .iter()
        .filter(|item| item.kind == HistoryKind::ToolCall)
    {
        let Some((name, arguments)) = item.content.trim_start().split_once(' ') else {
            continue;
        };
        if name != "todo" {
            continue;
        }
        let Ok(arguments) = serde_json::from_str::<serde_json::Value>(arguments) else {
            continue;
        };
        if let Some(todos) = parse_session_todos(&arguments) {
            latest = Some(todos);
        }
    }
    latest
}

pub(crate) fn parse_session_todos(value: &serde_json::Value) -> Option<Vec<SessionTodo>> {
    value
        .get("todos")?
        .as_array()?
        .iter()
        .map(|todo| {
            let content = todo.get("content")?.as_str()?.to_owned();
            let status = match todo.get("status")?.as_str()? {
                "pending" => TodoStatus::Pending,
                "in_progress" => TodoStatus::InProgress,
                "completed" => TodoStatus::Completed,
                _ => return None,
            };
            Some(SessionTodo { content, status })
        })
        .collect()
}

#[cfg(test)]
#[path = "domain_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRole {
    User,
    Assistant,
    Tool,
    SystemEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    User,
    Assistant,
    Tool,
    SystemEvent,
    Reasoning,
    ToolCall,
    ToolResult,
    Refusal,
}

impl HistoryKind {
    /// Reads the wire's spelling of a history kind.
    #[must_use]
    pub fn from_wire(kind: &str) -> Self {
        match kind {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "reasoning" => Self::Reasoning,
            "tool_call" => Self::ToolCall,
            "tool_result" => Self::ToolResult,
            "refusal" => Self::Refusal,
            "tool" => Self::Tool,
            _ => Self::SystemEvent,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatState {
    #[default]
    Idle,
    LoadingHistory,
    Sending,
    /// The model is generating: nothing has arrived yet.
    Thinking,
    /// Answer text is streaming in.
    Streaming,
    /// A tool is running for this turn.
    RunningTool,
    /// The session is compacting its history.
    Compacting,
    /// The model asked a question and the turn is parked until it is
    /// answered. It is not idle, the turn is still open, and not working
    /// either: nothing will happen until a person replies.
    AwaitingAnswer,
    /// Another client holds the session. Nothing is retried: taking it back
    /// would take it from whoever is using it.
    Displaced,
    /// The stream dropped and is being reopened. The conversation is intact;
    /// new events resume when the host is reachable again.
    Reconnecting,
}

impl ChatState {
    pub fn is_busy(self) -> bool {
        matches!(
            self,
            Self::LoadingHistory
                | Self::Sending
                | Self::Thinking
                | Self::Streaming
                | Self::RunningTool
        )
    }

    /// Whether a turn is running, so there is something to stop.
    ///
    /// Compacting and reconnecting hold the page, but neither has a turn the
    /// host could cancel: offering Stop there offers a button that fails.
    pub fn has_running_turn(self) -> bool {
        matches!(
            self,
            Self::Sending | Self::Thinking | Self::Streaming | Self::RunningTool
        )
    }

    /// The one line the composer shows while a turn runs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::LoadingHistory => "Loading conversation…",
            Self::Sending => "Sending…",
            Self::Thinking => "Thinking…",
            Self::Streaming => "Responding…",
            Self::RunningTool => "Running tool…",
            Self::Compacting => "Compacting context…",
            Self::AwaitingAnswer => "Waiting for your answer…",
            Self::Reconnecting => "Reconnecting…",
            Self::Displaced => "Open in another tab",
        }
    }
}

/// How a message sent while a turn is running should be handled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueueMode {
    /// Ask before interrupting: the server rejects the message.
    #[default]
    Reject,
    /// Hold the message until the running turn finishes.
    Queue,
    /// Stop the running turn and send this instead.
    Interrupt,
}

impl QueueMode {
    /// Every mode the composer offers, in the order it offers them.
    pub const ALL: [Self; 3] = [Self::Queue, Self::Interrupt, Self::Reject];

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Queue => "queue",
            Self::Interrupt => "interrupt",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Reject => "Wait for the turn to finish",
            Self::Queue => "Send after this turn",
            Self::Interrupt => "Stop the turn and send",
        }
    }
}
