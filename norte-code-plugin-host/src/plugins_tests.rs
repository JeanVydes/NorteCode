use std::collections::BTreeMap;

use async_trait::async_trait;
use norte::{AnyFunction, FnExecutor, ToolMetadata, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::*;

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
