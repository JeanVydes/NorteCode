//! Parsing and completing commands entered at the start of the composer.

use norte_code_client::PluginSummary;
use norte_code_wire::{Reasoning, ReasoningInfo};

pub(super) const COMMANDS: [CommandDefinition; 4] = [
    CommandDefinition {
        name: CommandName::Compact,
        usage: "/compact",
    },
    CommandDefinition {
        name: CommandName::New,
        usage: "/new",
    },
    CommandDefinition {
        name: CommandName::Open,
        usage: "/open",
    },
    CommandDefinition {
        name: CommandName::Reasoning,
        usage: "/reasoning",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommandName {
    Compact,
    New,
    Open,
    Reasoning,
}

impl CommandName {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Compact => "/compact",
            Self::New => "/new",
            Self::Open => "/open",
            Self::Reasoning => "/reasoning",
        }
    }

    fn parse(label: &str) -> Option<Self> {
        COMMANDS
            .iter()
            .find(|command| command.name.label() == label)
            .map(|command| command.name)
    }
}

pub(super) struct CommandDefinition {
    pub(super) name: CommandName,
    pub(super) usage: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffort {
    pub(super) const ALL: [Self; 6] = [
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|effort| effort.label() == value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum SlashCommand {
    Compact,
    New(Option<crate::client::NewSessionModel>),
    Open(Box<norte_code_wire::SessionSummary>),
    SetReasoning(Reasoning),
    Plugin {
        command: String,
        arguments: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlashCommandError {
    UnknownCommand,
    InvalidArguments,
    UnsupportedEffort,
    CannotDisable,
    UnsupportedBudget,
    InvalidBudget,
    BudgetTooLarge,
    UnknownSession,
    UnknownProvider,
    UnknownModel,
}

impl std::fmt::Display for SlashCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnknownCommand => "unknown command",
            Self::InvalidArguments => "invalid command arguments",
            Self::UnsupportedEffort => {
                "that reasoning effort is not supported by the selected model"
            }
            Self::CannotDisable => "the selected model does not allow reasoning to be disabled",
            Self::UnsupportedBudget => "the selected model does not accept a reasoning budget",
            Self::InvalidBudget => "budget must be a positive whole number of tokens",
            Self::BudgetTooLarge => "budget exceeds the selected model's maximum",
            Self::UnknownSession => "no matching session",
            Self::UnknownProvider => "unknown provider",
            Self::UnknownModel => "model does not belong to that provider",
        })
    }
}

pub(super) fn parse(
    input: &str,
    reasoning: Option<&ReasoningInfo>,
    plugins: &[PluginSummary],
    sessions: &[norte_code_wire::SessionSummary],
    models: &[norte_code_wire::ModelInfo],
) -> Result<SlashCommand, SlashCommandError> {
    let mut parts = input.split_ascii_whitespace();
    let label = parts.next().ok_or(SlashCommandError::UnknownCommand)?;
    let args = parts.collect::<Vec<_>>();
    match CommandName::parse(label) {
        Some(CommandName::Compact) if args.is_empty() => Ok(SlashCommand::Compact),
        Some(CommandName::Compact) => Err(SlashCommandError::InvalidArguments),
        Some(CommandName::New) => parse_new(&args, models),
        Some(CommandName::Open) => parse_open(&args, sessions),
        Some(CommandName::Reasoning) => parse_reasoning(&args, reasoning),
        None => parse_plugin(label, args, plugins),
    }
}

fn parse_new(
    arguments: &[&str],
    models: &[norte_code_wire::ModelInfo],
) -> Result<SlashCommand, SlashCommandError> {
    let selection = match arguments {
        [] => None,
        [provider, model] => {
            if !models
                .iter()
                .any(|candidate| !candidate.deprecated && candidate.provider == *provider)
            {
                return Err(SlashCommandError::UnknownProvider);
            }
            if !models.iter().any(|candidate| {
                !candidate.deprecated
                    && candidate.provider == *provider
                    && candidate.model == *model
            }) {
                return Err(SlashCommandError::UnknownModel);
            }
            Some(crate::client::NewSessionModel {
                provider: (*provider).to_owned(),
                model: (*model).to_owned(),
            })
        }
        _ => return Err(SlashCommandError::InvalidArguments),
    };
    Ok(SlashCommand::New(selection))
}

fn parse_open(
    arguments: &[&str],
    sessions: &[norte_code_wire::SessionSummary],
) -> Result<SlashCommand, SlashCommandError> {
    let [query] = arguments else {
        return Err(SlashCommandError::UnknownSession);
    };
    let session = sessions
        .iter()
        .find(|session| session.session_id == *query)
        .cloned()
        .ok_or(SlashCommandError::UnknownSession)?;
    Ok(SlashCommand::Open(Box::new(session)))
}

fn parse_plugin(
    label: &str,
    arguments: Vec<&str>,
    plugins: &[PluginSummary],
) -> Result<SlashCommand, SlashCommandError> {
    let name = label.strip_prefix('/').unwrap_or_default();
    let command = plugins
        .iter()
        .filter(|plugin| plugin.loaded)
        .flat_map(|plugin| &plugin.commands)
        .find(|command| command.name == name)
        .ok_or(SlashCommandError::UnknownCommand)?;
    if arguments.len() > command.arguments.len()
        || command
            .arguments
            .iter()
            .take(arguments.len())
            .zip(&arguments)
            .any(|(spec, value)| {
                !spec.choices.is_empty() && !spec.choices.iter().any(|choice| choice == value)
            })
        || command
            .arguments
            .iter()
            .skip(arguments.len())
            .any(|argument| argument.required)
    {
        return Err(SlashCommandError::InvalidArguments);
    }
    Ok(SlashCommand::Plugin {
        command: name.to_owned(),
        arguments: arguments.into_iter().map(str::to_owned).collect(),
    })
}

fn parse_reasoning(
    args: &[&str],
    reasoning: Option<&ReasoningInfo>,
) -> Result<SlashCommand, SlashCommandError> {
    if let ["auto"] = args {
        return Ok(SlashCommand::SetReasoning(Reasoning::Automatic));
    }
    let capabilities = reasoning.ok_or(SlashCommandError::UnsupportedEffort)?;
    match args {
        ["off"] if capabilities.can_disable => Ok(SlashCommand::SetReasoning(Reasoning::Disabled)),
        ["off"] => Err(SlashCommandError::CannotDisable),
        ["budget", _] if !capabilities.supports_budget => Err(SlashCommandError::UnsupportedBudget),
        ["budget", value] => {
            let tokens = value
                .parse::<u32>()
                .ok()
                .filter(|tokens| *tokens > 0)
                .ok_or(SlashCommandError::InvalidBudget)?;
            if capabilities.max_budget.is_some_and(|max| tokens > max) {
                return Err(SlashCommandError::BudgetTooLarge);
            }
            Ok(SlashCommand::SetReasoning(Reasoning::Budget { tokens }))
        }
        [value] => {
            let effort =
                ReasoningEffort::parse(value).ok_or(SlashCommandError::UnsupportedEffort)?;
            if !capabilities.supported
                || !capabilities
                    .efforts
                    .iter()
                    .any(|candidate| candidate == effort.label())
            {
                return Err(SlashCommandError::UnsupportedEffort);
            }
            Ok(SlashCommand::SetReasoning(Reasoning::Effort {
                effort: effort.label().to_owned(),
            }))
        }
        _ => Err(SlashCommandError::InvalidArguments),
    }
}

pub(super) use super::slash_suggestions::suggestions;

#[cfg(test)]
#[path = "slash_command_tests.rs"]
mod tests;
