use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SessionStatus {
    Active,
    Suspended,
    Closed,
    #[default]
    #[serde(other)]
    Unknown,
}

impl SessionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Available",
            Self::Suspended => "Suspended",
            Self::Closed => "Closed",
            Self::Unknown => "Unknown",
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            Self::Active => "status status-active",
            Self::Suspended | Self::Closed => "status status-muted",
            Self::Unknown => "status status-unknown",
        }
    }
}

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
    Jobs,
    Subagents,
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
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Failed => "Connection failed",
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationItem {
    pub id: String,
    pub turn_id: Option<String>,
    pub role: HistoryRole,
    pub kind: HistoryKind,
    pub content: String,
    pub created_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRole {
    User,
    Assistant,
    Tool,
    SystemEvent,
}

impl HistoryRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Assistant => "Assistant",
            Self::Tool => "Tool",
            Self::SystemEvent => "System",
        }
    }
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatState {
    #[default]
    Idle,
    Attaching,
    Sending,
    WaitingForReply,
    Failed,
}

impl ChatState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Attaching => "Attaching to session",
            Self::Sending => "Sending message",
            Self::WaitingForReply => "Waiting for response",
            Self::Failed => "Chat request failed",
        }
    }
}
