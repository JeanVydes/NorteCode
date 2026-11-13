use norte_code_wire::catalog::{Model, Provider};
use norte_code_wire::{QueueMode, SessionKey};

use crate::tools::DiffMode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolRuleAction {
    Allow,
    Deny,
    Forget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum McpState {
    Active,
    Inactive,
}

pub(crate) enum PluginOperation {
    Install {
        url: String,
        package: Option<String>,
    },
    Update {
        name: Option<String>,
    },
    Remove {
        name: String,
    },
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
}

pub(crate) enum CatalogOperation {
    UpsertProvider { id: String, provider: Box<Provider> },
    RemoveProvider(String),
    UpsertModel(Box<Model>),
    RemoveModel { provider: String, model: String },
}

#[derive(Clone, Copy)]
pub(crate) enum Preference {
    Theme,
    Thinking,
    Timestamps,
    ToolOutput,
    LiveRefresh,
    DeveloperMode,
    DiffStyle,
    BudgetAction,
}

pub(crate) enum ViewAction {
    RefreshHost,
    RefreshSkills,
    RefreshMcp,
    SetMcp {
        name: String,
        state: McpState,
    },
    SetToolRule {
        name: String,
        action: ToolRuleAction,
    },
    RefreshPlugins,
    Plugin(PluginOperation),
    RefreshTelemetry,
    ToggleTelemetryScope,
    RefreshCosts,
    RefreshCatalog,
    Catalog(Box<CatalogOperation>),
    CompactSession,
    ExportSession,
    TogglePreference(Preference),
    TestConnection,
}

pub(crate) enum ComponentAction {
    OpenSession(SessionKey),
    OpenChildSession(SessionKey),
    CreateSession,
    RenameSession {
        session: SessionKey,
        title: String,
    },
    DeleteSession(SessionKey),
    ForkSession(SessionKey),
    SendMessage {
        text: String,
        chips: Vec<crate::chips::Chip>,
    },
    AttachFile(String),
    ImportSession(String),
    SetQueueMode(QueueMode),
    SetModel {
        provider: String,
        model: String,
    },
    SetBudget {
        limit: f64,
        action: norte_code_wire::BudgetAction,
    },
    ApproveBudget,
    DeclineBudget,
    CancelTurn,
    SetReasoning(crate::ui_model::ReasoningLevel),
    Rewind {
        turn_id: String,
    },
    PinTurn {
        turn_id: String,
        pinned: bool,
    },
    StopJob {
        job_id: String,
    },
    SetDiffStyle(DiffMode),
    AnswerQuestion {
        session: SessionKey,
        question: String,
        answer: String,
    },
    View(ViewAction),
}
