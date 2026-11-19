use norte_code_wire::SessionKey;

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

/// A setting the reader can flip from the menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Preference {
    Theme,
    Thinking,
    Timestamps,
    ToolOutput,
    LiveRefresh,
    DeveloperMode,
}

/// What the command menu asks the application to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandMenuAction {
    NewSession,
    ExportSession,
    ImportSession,
    CompactSession,
    RenameSession,
    DeleteSession,
    ForkSession,
    SetQueueMode(crate::ui_model::SendMode),
    SetReasoning(crate::ui_model::ReasoningLevel),
    RewindToTurn,
    PinCurrentTurn,
    SetBudgetLimit,
    SetBudgetAction(norte_code_wire::BudgetAction),
    RefreshView,
    SetDiffStyle(DiffMode),
    ToggleShellOutput,
    TogglePreference(Preference),
    Help,
    Quit,
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
    RefreshCosts,
    RefreshJobs,
    CompactSession,
    ExportSession,
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
    AnswerQuestion {
        session: SessionKey,
        question: String,
        answer: String,
    },
    View(ViewAction),
}

#[cfg(test)]
#[path = "action_tests.rs"]
mod tests;
