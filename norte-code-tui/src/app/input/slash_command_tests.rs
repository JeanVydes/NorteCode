use norte_code_client::PluginSummary;
use norte_code_wire::{
    Modality, ModelInfo, PluginCommandArgumentInfo, PluginCommandInfo, Reasoning, ReasoningInfo,
    SessionSummary,
};

use super::{SlashCommand, SlashCommandError, parse, suggestions};

fn completions(
    input: &str,
    reasoning: Option<&ReasoningInfo>,
    plugins: &[PluginSummary],
) -> Vec<String> {
    suggestions(input, reasoning, plugins, &[], &[])
        .into_iter()
        .map(|suggestion| suggestion.completion)
        .collect()
}

fn model(provider: &str, id: &str, name: &str) -> ModelInfo {
    ModelInfo {
        provider: provider.to_owned(),
        model: id.to_owned(),
        name: name.to_owned(),
        description: None,
        max_context: None,
        max_output: None,
        tools: false,
        input: vec![Modality::Text],
        output: vec![Modality::Text],
        reasoning: ReasoningInfo::default(),
        deprecated: false,
    }
}

#[test]
fn double_slash_is_not_a_command_or_completion() {
    assert_eq!(
        suggestions("//comment", None, &[], &[], &[]),
        Vec::<crate::ui::CommandSuggestion>::new()
    );
    assert!(parse("//comment", None, &[], &[], &[]).is_err());
}

#[test]
fn compact_accepts_no_arguments() {
    assert_eq!(
        parse("/compact", None, &[], &[], &[]),
        Ok(SlashCommand::Compact)
    );
    assert_eq!(
        parse("/compact now", None, &[], &[], &[]),
        Err(SlashCommandError::InvalidArguments)
    );
}

#[test]
fn effort_completion_and_validation_follow_model_capabilities() {
    let capabilities = ReasoningInfo {
        supported: true,
        can_disable: false,
        efforts: vec!["low".to_owned(), "xhigh".to_owned()],
        supports_budget: false,
        max_budget: None,
    };
    assert_eq!(
        completions("/reasoning ", Some(&capabilities), &[]),
        vec!["/reasoning auto", "/reasoning low", "/reasoning xhigh"]
    );
    assert_eq!(
        parse("/reasoning xhigh", Some(&capabilities), &[], &[], &[]),
        Ok(SlashCommand::SetReasoning(Reasoning::Effort {
            effort: "xhigh".to_owned()
        }))
    );
    assert_eq!(
        parse("/reasoning high", Some(&capabilities), &[], &[], &[]),
        Err(SlashCommandError::UnsupportedEffort)
    );
    assert_eq!(
        parse("/reasoning off", Some(&capabilities), &[], &[], &[]),
        Err(SlashCommandError::CannotDisable)
    );
}

#[test]
fn budget_completion_and_validation_follow_budget_capabilities() {
    let capabilities = ReasoningInfo {
        supported: true,
        can_disable: false,
        efforts: Vec::new(),
        supports_budget: true,
        max_budget: Some(4096),
    };
    assert_eq!(
        completions("/reasoning budget ", Some(&capabilities), &[]),
        vec!["/reasoning budget 1024", "/reasoning budget 4096"]
    );
    assert_eq!(
        parse("/reasoning budget 2048", Some(&capabilities), &[], &[], &[]),
        Ok(SlashCommand::SetReasoning(Reasoning::Budget {
            tokens: 2048
        }))
    );
    assert_eq!(
        parse("/reasoning budget 8192", Some(&capabilities), &[], &[], &[]),
        Err(SlashCommandError::BudgetTooLarge)
    );
}

fn deployment_plugin() -> PluginSummary {
    PluginSummary {
        name: "deploy-plugin".to_owned(),
        loaded: true,
        commands: vec![PluginCommandInfo {
            name: "deploy".to_owned(),
            description: "Deploy the workspace".to_owned(),
            arguments: vec![PluginCommandArgumentInfo {
                name: "environment".to_owned(),
                description: "Deployment target".to_owned(),
                required: true,
                choices: vec!["staging".to_owned(), "production".to_owned()],
            }],
        }],
        ..PluginSummary::default()
    }
}

#[test]
fn plugin_commands_and_choice_arguments_autocomplete_and_validate() {
    let plugins = vec![deployment_plugin()];
    assert_eq!(completions("/dep", None, &plugins), vec!["/deploy "]);
    assert_eq!(
        completions("/deploy pro", None, &plugins),
        vec!["/deploy production"]
    );
    assert_eq!(
        parse("/deploy staging", None, &plugins, &[], &[]),
        Ok(SlashCommand::Plugin {
            command: "deploy".to_owned(),
            arguments: vec!["staging".to_owned()],
        })
    );
    assert_eq!(
        parse("/deploy nowhere", None, &plugins, &[], &[]),
        Err(SlashCommandError::InvalidArguments)
    );
}

#[test]
fn new_completes_provider_then_only_that_providers_models() {
    let models = vec![
        model("anthropic", "sonnet", "Claude Sonnet"),
        model("anthropic", "opus", "Claude Opus"),
        model("openai", "gpt-5", "GPT 5"),
    ];
    let providers = suggestions("/new ", None, &[], &[], &models);
    assert_eq!(
        providers
            .iter()
            .map(|item| item.completion.as_str())
            .collect::<Vec<_>>(),
        ["/new anthropic", "/new openai"]
    );
    let anthropic_models = suggestions("/new anthropic ", None, &[], &[], &models);
    assert_eq!(
        anthropic_models
            .iter()
            .map(|item| item.completion.as_str())
            .collect::<Vec<_>>(),
        ["/new anthropic sonnet", "/new anthropic opus"]
    );
    assert_eq!(
        parse("/new anthropic sonnet", None, &[], &[], &models),
        Ok(SlashCommand::New(Some(crate::client::NewSessionModel {
            provider: "anthropic".to_owned(),
            model: "sonnet".to_owned(),
        })))
    );
    assert!(matches!(
        parse("/new openai sonnet", None, &[], &[], &models),
        Err(SlashCommandError::UnknownModel)
    ));
}

#[test]
fn new_without_arguments_starts_a_default_model_draft() {
    assert_eq!(
        parse("/new", None, &[], &[], &[]),
        Ok(SlashCommand::New(None))
    );
}

#[test]
fn open_suggestions_are_newest_first_and_resolve_only_known_sessions() {
    let sessions = vec![
        SessionSummary {
            session_id: "older-id-1234".to_owned(),
            title: Some("Older".to_owned()),
            updated_at: Some(10),
            ..SessionSummary::default()
        },
        SessionSummary {
            session_id: "newer-id-5678".to_owned(),
            title: Some("Newest".to_owned()),
            updated_at: Some(20),
            ..SessionSummary::default()
        },
    ];
    let items = suggestions("/open ", None, &[], &sessions, &[]);
    assert_eq!(items[0].label, "Newest · newer-id");
    assert_eq!(items[0].completion, "/open newer-id-5678");
    assert_eq!(
        parse("/open newer-id-5678", None, &[], &sessions, &[]).expect("known session"),
        SlashCommand::Open(Box::new(sessions[1].clone()))
    );
    assert_eq!(
        parse("/open missing", None, &[], &sessions, &[]),
        Err(SlashCommandError::UnknownSession)
    );
}
