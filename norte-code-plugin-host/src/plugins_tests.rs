use std::collections::BTreeMap;

use async_trait::async_trait;
use norte::{AnyFunction, FnExecutor, ToolMetadata, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::*;
use crate::{
    CommandArgument, CommandName, CommandOutput, CommandRegistrationError, CommandRegistry,
    CommandSpec,
};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct EmptyArgs {}

#[derive(Clone, Copy)]
struct Hello;

impl ToolMetadata for Hello {
    const NAME: &'static str = "hello";
    const DESCRIPTION: &'static str = "greet somebody";
}

#[async_trait]
impl FnExecutor<EmptyArgs, ToolOutput> for Hello {
    async fn call(&self, _args: EmptyArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::text("hello"))
    }
}

fn hello_tool() -> Arc<dyn AnyFunction> {
    Arc::new(norte::FunctionDeclaration::new(
        Hello::NAME,
        Hello::DESCRIPTION,
        Arc::new(Hello),
    ))
}

/// A plugin that answers everything, so the registry's plumbing is what the
/// tests are about.
struct Everything;

impl Plugin for Everything {
    fn name(&self) -> &'static str {
        "everything"
    }

    fn tools(&self) -> Vec<Arc<dyn AnyFunction>> {
        vec![hello_tool()]
    }

    fn status(&self) -> Option<String> {
        Some("hello".to_owned())
    }

    fn banner(&self) -> Option<String> {
        Some("  hello  ".to_owned())
    }
}

/// A plugin that adds one tool and nothing else.
struct Only;

impl Plugin for Only {
    fn name(&self) -> &'static str {
        "only"
    }

    fn tools(&self) -> Vec<Arc<dyn AnyFunction>> {
        vec![hello_tool()]
    }
}

struct CommandPlugin;

impl Plugin for CommandPlugin {
    fn name(&self) -> &'static str {
        "deploy"
    }

    fn register_commands(&self, registry: &mut CommandRegistry) {
        registry
            .register(
                self.name(),
                CommandSpec {
                    name: CommandName::new("deploy").expect("valid command"),
                    description: "Deploy the current workspace".to_owned(),
                    arguments: vec![CommandArgument {
                        name: "environment".to_owned(),
                        description: "Target environment".to_owned(),
                        required: true,
                        choices: vec!["staging".to_owned(), "production".to_owned()],
                    }],
                },
            )
            .expect("unique command");
    }

    fn execute_command(
        &self,
        command: &CommandName,
        arguments: &[String],
    ) -> Result<CommandOutput, CommandExecutionError> {
        if command.as_str() != "deploy" {
            return Err(CommandExecutionError::UnknownCommand);
        }
        Ok(CommandOutput {
            message: format!("deployed to {}", arguments[0]),
        })
    }
}

#[test]
fn plugins_register_commands_and_execute_their_own_actions() {
    let mut plugins = Plugins::new();
    plugins.add(Box::new(CommandPlugin));
    let registry = plugins.command_registry();
    let command = CommandName::new("deploy").expect("valid command");
    let result = plugins
        .execute_command(&registry, &command, &["staging".to_owned()])
        .expect("plugin command");
    assert_eq!(result.message, "deployed to staging");
}

struct SameNamedCommand {
    command: &'static str,
    output: &'static str,
}

impl Plugin for SameNamedCommand {
    fn name(&self) -> &'static str {
        "same-name"
    }

    fn register_commands(&self, registry: &mut CommandRegistry) {
        registry
            .register(
                self.name(),
                CommandSpec {
                    name: CommandName::new(self.command).expect("valid command"),
                    description: "Test command".to_owned(),
                    arguments: Vec::new(),
                },
            )
            .expect("unique command");
    }

    fn execute_command(
        &self,
        command: &CommandName,
        _arguments: &[String],
    ) -> Result<CommandOutput, CommandExecutionError> {
        if command.as_str() != self.command {
            return Err(CommandExecutionError::UnknownCommand);
        }
        Ok(CommandOutput {
            message: self.output.to_owned(),
        })
    }
}

#[test]
fn commands_route_to_the_declaring_plugin_even_when_names_repeat() {
    let mut plugins = Plugins::new();
    plugins.add(Box::new(SameNamedCommand {
        command: "first-action",
        output: "first",
    }));
    plugins.add(Box::new(SameNamedCommand {
        command: "second-action",
        output: "second",
    }));
    let registry = plugins.command_registry();
    let command = CommandName::new("second-action").expect("valid command");
    let output = plugins
        .execute_command(&registry, &command, &[])
        .expect("second plugin command");
    assert_eq!(output.message, "second");
}

#[test]
fn registry_rejects_builtin_collisions_and_invalid_argument_order() {
    let mut registry = CommandRegistry::default();
    for name in ["compact", "new", "open", "reasoning"] {
        let reserved = registry.register(
            "plugin",
            CommandSpec {
                name: CommandName::new(name).expect("valid command name"),
                description: "collision".to_owned(),
                arguments: Vec::new(),
            },
        );
        assert!(matches!(
            reserved,
            Err(CommandRegistrationError::Reserved(_))
        ));
    }

    let invalid = registry.register(
        "plugin",
        CommandSpec {
            name: CommandName::new("deploy").expect("valid command name"),
            description: "Deploy".to_owned(),
            arguments: vec![
                CommandArgument {
                    name: "environment".to_owned(),
                    description: "Optional environment".to_owned(),
                    required: false,
                    choices: Vec::new(),
                },
                CommandArgument {
                    name: "version".to_owned(),
                    description: "Required version".to_owned(),
                    required: true,
                    choices: Vec::new(),
                },
            ],
        },
    );
    assert!(matches!(
        invalid,
        Err(CommandRegistrationError::InvalidDefinition { .. })
    ));
}

#[test]
fn registry_rejects_ambiguous_argument_metadata() {
    let mut registry = CommandRegistry::default();
    let error = registry.register(
        "plugin",
        CommandSpec {
            name: CommandName::new("deploy").expect("valid command name"),
            description: "Deploy".to_owned(),
            arguments: vec![
                CommandArgument {
                    name: "environment".to_owned(),
                    description: "Target environment".to_owned(),
                    required: true,
                    choices: vec!["staging".to_owned(), "staging".to_owned()],
                },
                CommandArgument {
                    name: "environment".to_owned(),
                    description: "Repeated argument".to_owned(),
                    required: false,
                    choices: Vec::new(),
                },
            ],
        },
    );
    assert!(matches!(
        error,
        Err(CommandRegistrationError::InvalidDefinition { .. })
    ));
}

#[test]
fn a_plugin_that_implements_one_thing_says_nothing_about_the_rest() {
    let plugins = {
        let mut plugins = Plugins::new();
        plugins.add(Box::new(Only));
        plugins
    };
    assert_eq!(plugins.tools().len(), 1);
    assert_eq!(plugins.status().len(), 0);
    assert_eq!(plugins.banner().len(), 0);
}

#[test]
fn tools_from_every_plugin_reach_the_host() {
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Everything));
    plugins.add(Box::new(Only));
    let tools = plugins.tools();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name(), "hello");
    assert_eq!(tools[0].description(), "greet somebody");
}

#[test]
fn tools_are_paired_with_the_plugin_that_declared_them() {
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Everything));
    plugins.add(Box::new(Only));
    let named = plugins.tools_named();
    assert_eq!(named.len(), 2);
    assert_eq!(named[0].0, "everything");
    assert_eq!(named[0].1.name(), "hello");
    assert_eq!(named[1].0, "only");
}

#[test]
fn nothing_loaded_is_not_an_error() {
    let plugins = Plugins::new();
    assert!(plugins.is_empty());
    assert_eq!(plugins.len(), 0);
    assert_eq!(plugins.tools().len(), 0);
    assert_eq!(plugins.names().len(), 0);
}

#[test]
fn two_plugins_may_share_a_name_and_both_are_kept() {
    struct Same;
    impl Plugin for Same {
        fn name(&self) -> &'static str {
            "everything"
        }
    }
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Everything));
    plugins.add(Box::new(Same));
    assert_eq!(plugins.names(), ["everything", "everything"]);
    assert_eq!(plugins.len(), 2);
}

/// A host that answers for one session, so the plugin has something to ask.
struct Answering;

impl PluginHost for Answering {
    fn session(&self) -> Option<SessionKey> {
        Some(SessionKey::new("s"))
    }

    fn session_model(&self) -> Option<String> {
        Some("nvidia/nemotron".to_owned())
    }

    fn session_provider(&self) -> Option<String> {
        Some("nvidia".to_owned())
    }

    fn session_title(&self) -> Option<String> {
        Some("a session".to_owned())
    }

    fn session_context(&self) -> ContextUsage {
        ContextUsage {
            used: 4_096,
            window: Some(128_000),
        }
    }
}

/// A plugin that asks the host what it may not know.
struct Asking {
    host: Option<Arc<dyn PluginHost>>,
}

impl Plugin for Asking {
    fn name(&self) -> &'static str {
        "asking"
    }

    fn set_context(&mut self, ctx: PluginContext) {
        self.host = ctx.host;
    }

    fn status(&self) -> Option<String> {
        let host = self.host.as_ref()?;
        Some(format!(
            "{} {} {}",
            host.session_model().unwrap_or_default(),
            host.session_provider().unwrap_or_default(),
            host.session_context().used
        ))
    }
}

#[test]
fn a_plugin_asks_the_host_and_the_host_answers() {
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Asking { host: None }));
    plugins.set_context_on_all(&PluginContext {
        host: Some(Arc::new(Answering)),
    });
    let statuses = plugins.status();
    assert_eq!(statuses, ["nvidia/nemotron nvidia 4096"]);
}

#[test]
fn a_context_without_a_host_is_not_an_error() {
    let context = PluginContext { host: None };
    assert!(context.host.is_none());
}

#[test]
fn the_context_share_is_the_used_part_of_the_window() {
    let usage = ContextUsage {
        used: 32_000,
        window: Some(128_000),
    };
    assert_eq!(usage.share(), Some(0.25));
}

#[test]
fn a_session_without_a_window_has_no_share() {
    let usage = ContextUsage {
        used: 1_000,
        window: None,
    };
    assert_eq!(usage.share(), None);
}

/// A plugin that offers a provider and a model of its own.
struct Offering;

impl Plugin for Offering {
    fn name(&self) -> &'static str {
        "offering"
    }

    fn catalogue(&self) -> Catalogue {
        let provider = Provider {
            name: "demo".to_owned(),
            doc: None,
            environment_keys: vec!["DEMO_API_KEY".to_owned()],
            wires: Vec::new(),
        };
        let mut model = Model {
            provider_id: "demo".to_owned(),
            id: "demo-model".to_owned(),
            ..Model::default()
        };
        model.name = "Demo model".to_owned();
        Catalogue {
            providers: BTreeMap::from([("demo".to_owned(), provider)]),
            models: vec![model],
        }
    }
}

#[test]
fn a_plugin_offers_its_own_providers_and_models() {
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Offering));
    let catalogue = plugins.catalogue();
    assert_eq!(catalogue.providers.len(), 1);
    assert_eq!(catalogue.models.len(), 1);
    assert_eq!(catalogue.models[0].key(), "demo/demo-model");
}

#[test]
fn two_plugins_offering_the_same_provider_leave_one() {
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Offering));
    plugins.add(Box::new(Offering));
    let catalogue = plugins.catalogue();
    assert_eq!(catalogue.providers.len(), 1);
    assert_eq!(catalogue.models.len(), 1, "the model is one as well");
}
