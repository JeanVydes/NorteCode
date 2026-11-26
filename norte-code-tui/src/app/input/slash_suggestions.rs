//! Slash-command completions for built-ins, sessions, models and plugins.

use norte_code_client::PluginSummary;
use norte_code_wire::ReasoningInfo;

use super::slash_command::{COMMANDS, ReasoningEffort};

const BUDGET_PRESETS: [u32; 4] = [1024, 4096, 8192, 16384];

pub(super) fn suggestions(
    input: &str,
    reasoning: Option<&ReasoningInfo>,
    plugins: &[PluginSummary],
    sessions: &[norte_code_wire::SessionSummary],
    models: &[norte_code_wire::ModelInfo],
) -> Vec<super::super::ui::CommandSuggestion> {
    if input.starts_with("//") || !input.starts_with('/') {
        return Vec::new();
    }
    let candidates = if input == "/open" || input.starts_with("/open ") {
        open_suggestions(input, sessions)
    } else if input == "/new" || input.starts_with("/new ") {
        new_suggestions(input, models)
    } else if input == "/queue" || input.starts_with("/queue ") {
        queue_suggestions(input)
    } else if !input.contains(' ') {
        let mut candidates = COMMANDS
            .iter()
            .map(|command| suggestion(command.usage.to_owned(), command.usage.to_owned()))
            .collect::<Vec<_>>();
        candidates.extend(
            plugins
                .iter()
                .filter(|plugin| plugin.loaded)
                .flat_map(|plugin| &plugin.commands)
                .map(|command| {
                    let suffix = if command.arguments.is_empty() {
                        ""
                    } else {
                        " "
                    };
                    let value = format!("/{}{suffix}", command.name);
                    suggestion(value.clone(), value)
                }),
        );
        candidates
    } else if input.starts_with("/reasoning ") {
        reasoning_suggestions(input, reasoning)
            .into_iter()
            .map(|value| suggestion(value.clone(), value))
            .collect()
    } else {
        plugin_argument_suggestions(input, plugins)
            .into_iter()
            .map(|value| suggestion(value.clone(), value))
            .collect()
    };
    candidates
        .into_iter()
        .filter(|candidate| candidate.completion.starts_with(input))
        .collect()
}

fn queue_suggestions(input: &str) -> Vec<super::super::ui::CommandSuggestion> {
    let value = input
        .strip_prefix("/queue")
        .unwrap_or_default()
        .trim_start();
    crate::ui_model::SendMode::ALL
        .into_iter()
        .map(|mode| {
            let completion = format!("/queue {}", mode.label());
            suggestion(completion, mode.label().to_owned())
        })
        .filter(|candidate| {
            candidate
                .completion
                .strip_prefix("/queue ")
                .is_some_and(|mode| mode.starts_with(value))
        })
        .collect()
}

fn suggestion(completion: String, label: String) -> super::super::ui::CommandSuggestion {
    super::super::ui::CommandSuggestion { completion, label }
}

fn open_suggestions(
    input: &str,
    sessions: &[norte_code_wire::SessionSummary],
) -> Vec<super::super::ui::CommandSuggestion> {
    let query = input.strip_prefix("/open").unwrap_or_default().trim();
    let mut sessions = sessions.iter().collect::<Vec<_>>();
    sessions.sort_by_key(|session| {
        std::cmp::Reverse(
            session
                .updated_at
                .or(session.last_activity_at)
                .or(session.created_at)
                .unwrap_or_default(),
        )
    });
    sessions
        .into_iter()
        .filter(|session| query.is_empty() || session.session_id.starts_with(query))
        .map(|session| {
            let title = session
                .title
                .as_deref()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("Untitled session");
            suggestion(
                format!("/open {}", session.session_id),
                format!("{title} · {}", short_session_id(&session.session_id)),
            )
        })
        .collect()
}

fn short_session_id(session: &str) -> &str {
    session.get(..8).unwrap_or(session)
}

fn new_suggestions(
    input: &str,
    models: &[norte_code_wire::ModelInfo],
) -> Vec<super::super::ui::CommandSuggestion> {
    let Some(arguments) = input.strip_prefix("/new") else {
        return Vec::new();
    };
    let has_separator = arguments.starts_with(' ');
    let values = arguments.split_ascii_whitespace().collect::<Vec<_>>();
    match values.as_slice() {
        [] if has_separator => provider_suggestions("", models),
        [provider_prefix] if has_separator && !arguments.ends_with(' ') => {
            let exact_provider = models
                .iter()
                .any(|model| !model.deprecated && model.provider == *provider_prefix);
            if exact_provider {
                model_suggestions(provider_prefix, "", models)
            } else {
                provider_suggestions(provider_prefix, models)
            }
        }
        [provider] if has_separator => model_suggestions(provider, "", models),
        [provider, model_prefix] => model_suggestions(provider, model_prefix, models),
        _ => Vec::new(),
    }
}

fn provider_suggestions(
    prefix: &str,
    models: &[norte_code_wire::ModelInfo],
) -> Vec<super::super::ui::CommandSuggestion> {
    let mut providers = models
        .iter()
        .filter(|model| !model.deprecated)
        .map(|model| model.provider.as_str())
        .filter(|provider| provider.starts_with(prefix))
        .collect::<Vec<_>>();
    providers.sort_unstable();
    providers.dedup();
    providers
        .into_iter()
        .map(|provider| suggestion(format!("/new {provider}"), provider.to_owned()))
        .collect()
}

fn model_suggestions(
    provider: &str,
    prefix: &str,
    models: &[norte_code_wire::ModelInfo],
) -> Vec<super::super::ui::CommandSuggestion> {
    models
        .iter()
        .filter(|model| {
            !model.deprecated && model.provider == provider && model.model.starts_with(prefix)
        })
        .map(|model| {
            suggestion(
                format!("/new {} {}", model.provider, model.model),
                format!("{} · {}", model.name, model.model),
            )
        })
        .collect()
}

fn plugin_argument_suggestions(input: &str, plugins: &[PluginSummary]) -> Vec<String> {
    let Some((label, raw_arguments)) = input[1..].split_once(' ') else {
        return Vec::new();
    };
    let command = plugins
        .iter()
        .filter(|plugin| plugin.loaded)
        .flat_map(|plugin| &plugin.commands)
        .find(|command| command.name == label);
    let Some(command) = command else {
        return Vec::new();
    };
    let trailing_space = raw_arguments.ends_with(' ');
    let mut values = raw_arguments.split_ascii_whitespace().collect::<Vec<_>>();
    let prefix = if trailing_space {
        ""
    } else {
        values.pop().unwrap_or_default()
    };
    let argument_index = values.len();
    let Some(argument) = command.arguments.get(argument_index) else {
        return Vec::new();
    };
    let prefix_args = values.join(" ");
    argument
        .choices
        .iter()
        .filter(|choice| choice.starts_with(prefix))
        .map(|choice| {
            let separator = if prefix_args.is_empty() { "" } else { " " };
            format!("/{label} {prefix_args}{separator}{choice}")
        })
        .collect()
}

fn reasoning_suggestions(input: &str, reasoning: Option<&ReasoningInfo>) -> Vec<String> {
    let capabilities = reasoning.cloned().unwrap_or_default();
    let value = input.strip_prefix("/reasoning ").unwrap_or_default();
    if value.starts_with("budget ") {
        return BUDGET_PRESETS
            .into_iter()
            .filter(|_| capabilities.supports_budget)
            .filter(|tokens| capabilities.max_budget.is_none_or(|max| *tokens <= max))
            .map(|tokens| format!("/reasoning budget {tokens}"))
            .collect();
    }
    let mut values = vec!["auto".to_owned()];
    if capabilities.can_disable {
        values.push("off".to_owned());
    }
    if capabilities.supported {
        values.extend(
            ReasoningEffort::ALL
                .into_iter()
                .filter(|effort| {
                    capabilities
                        .efforts
                        .iter()
                        .any(|value| value == effort.label())
                })
                .map(|effort| effort.label().to_owned()),
        );
    }
    if capabilities.supports_budget {
        values.push("budget".to_owned());
    }
    values
        .into_iter()
        .map(|value| format!("/reasoning {value}"))
        .collect()
}
