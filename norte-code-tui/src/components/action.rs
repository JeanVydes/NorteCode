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
    RefreshJobs,
    CompactSession,
    ExportSession,
    TogglePreference(Preference),
    TestConnection,
}

/// A choice the header can open as a floating menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeaderModal {
    /// The queue mode of the session.
    Queue,
    /// The reasoning effort of the next request.
    Reasoning,
}

impl HeaderModal {
    /// The title of the floating menu.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Queue => "queue mode",
            Self::Reasoning => "reasoning effort",
        }
    }

    /// The options of the menu when no model is known, in the order shown.
    pub(crate) const fn options(self) -> &'static [&'static str] {
        match self {
            Self::Queue => &["steer", "queue", "interrupt", "reject"],
            Self::Reasoning => &["auto", "off", "low", "medium", "high"],
        }
    }

    /// The options the model accepts, in the order shown.
    pub(crate) fn options_for(
        self,
        capabilities: Option<&norte_code_wire::ReasoningInfo>,
    ) -> Vec<&'static str> {
        match self {
            Self::Queue => Self::Queue.options().to_vec(),
            Self::Reasoning => {
                let mut options = vec!["auto"];
                let Some(info) = capabilities else {
                    options.extend(["off", "low", "medium", "high"]);
                    return options;
                };
                if crate::ui_model::ReasoningLevel::Disabled
                    .incompatible_with(info)
                    .is_none()
                {
                    options.push("off");
                }
                for level in [
                    crate::ui_model::ReasoningLevel::Low,
                    crate::ui_model::ReasoningLevel::Medium,
                    crate::ui_model::ReasoningLevel::High,
                ] {
                    if level.incompatible_with(info).is_none() {
                        options.push(level.label());
                    }
                }
                options
            }
        }
    }

    /// The action the chosen option stands for, when it is one.
    pub(crate) fn action(self, index: usize, selected: &[&'static str]) -> Option<ComponentAction> {
        match self {
            Self::Queue => match selected.get(index) {
                Some(&"steer") => Some(ComponentAction::SetQueueMode(
                    crate::ui_model::SendMode::Steer,
                )),
                Some(&"queue") => Some(ComponentAction::SetQueueMode(
                    crate::ui_model::SendMode::Queue,
                )),
                Some(&"interrupt") => Some(ComponentAction::SetQueueMode(
                    crate::ui_model::SendMode::Interrupt,
                )),
                Some(&"reject") => Some(ComponentAction::SetQueueMode(
                    crate::ui_model::SendMode::Reject,
                )),
                _ => None,
            },
            Self::Reasoning => match selected.get(index).copied()? {
                "auto" => Some(ComponentAction::SetReasoning(
                    crate::ui_model::ReasoningLevel::Automatic,
                )),
                "off" => Some(ComponentAction::SetReasoning(
                    crate::ui_model::ReasoningLevel::Disabled,
                )),
                "low" => Some(ComponentAction::SetReasoning(
                    crate::ui_model::ReasoningLevel::Low,
                )),
                "medium" => Some(ComponentAction::SetReasoning(
                    crate::ui_model::ReasoningLevel::Medium,
                )),
                "high" => Some(ComponentAction::SetReasoning(
                    crate::ui_model::ReasoningLevel::High,
                )),
                _ => None,
            },
        }
    }
}

pub(crate) enum ComponentAction {
    OpenSession(SessionKey),
    /// Opens a floating menu for a header segment.
    OpenHeaderModal(HeaderModal),
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
    ScrollTranscript(isize),
    SetQueueMode(crate::ui_model::SendMode),
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

#[cfg(test)]
#[path = "action_tests.rs"]
mod tests;
