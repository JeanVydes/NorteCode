use super::*;
use crate::commands::ArgSpec;

/// A plugin that answers everything, so the registry's plumbing is what the
/// tests are about.
struct Everything;

impl Plugin for Everything {
    fn name(&self) -> &'static str {
        "everything"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![CommandSpec::new("/hello", "say hello").arg(ArgSpec::rest("who"))]
    }

    fn run(&self, command: &Invocation) -> Option<Vec<String>> {
        (command.command == "/hello")
            .then(|| vec![format!("hello {}", command.arg_or_empty("who"))])
    }

    fn status(&self) -> Option<String> {
        Some("hello".to_owned())
    }

    fn banner(&self) -> Option<String> {
        Some("  hello  ".to_owned())
    }
}

/// A plugin that adds one command and nothing else.
struct Only;

impl Plugin for Only {
    fn name(&self) -> &'static str {
        "only"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![CommandSpec::new("/only", "just this")]
    }
}

fn invocation(text: &str) -> Invocation {
    let mut registry = crate::commands::Registry::new();
    registry.register(CommandSpec::new("/hello", "say hello").arg(ArgSpec::rest("who")));
    registry.parse(text).expect("a command").expect("parsed")
}

#[test]
fn a_plugin_that_implements_one_thing_says_nothing_about_the_rest() {
    // The whole reason the trait has defaults: a plugin that adds a command
    // should not have to know what a panel is to compile.
    let plugins = {
        let mut plugins = Plugins::new();
        plugins.add(Box::new(Only));
        plugins
    };
    assert_eq!(plugins.commands().len(), 1);
    assert_eq!(plugins.status().len(), 0);
    assert_eq!(plugins.banner().len(), 0);
}

#[test]
fn a_command_reaches_the_plugin_that_declared_it() {
    // The host parses; the plugin runs. What comes back is lines of text,
    // which is what the host draws and what a test reads.
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Everything));
    let lines = plugins.run(&invocation("/hello world")).expect("an answer");
    assert_eq!(lines, ["hello world"]);
}

#[test]
fn a_command_nobody_declared_answers_nothing() {
    // `None` is what lets the host fall through to its own commands: a
    // plugin that answered everything would swallow them.
    let mut registry = crate::commands::Registry::new();
    registry.register(CommandSpec::new("/mine", "mine"));
    let command = registry.parse("/mine").expect("a command").expect("parsed");
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Everything));
    assert!(plugins.run(&command).is_none());
}

#[test]
fn the_first_plugin_to_answer_one_command_is_the_one_that_runs() {
    // Two plugins claiming a name is two answers to one question, and the
    // second would be answering something nobody asked it.
    struct Second;
    impl Plugin for Second {
        fn name(&self) -> &'static str {
            "second"
        }
        fn run(&self, _command: &Invocation) -> Option<Vec<String>> {
            Some(vec!["the second one".to_owned()])
        }
    }
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Everything));
    plugins.add(Box::new(Second));
    let lines = plugins.run(&invocation("/hello")).expect("an answer");
    assert_eq!(lines, ["hello "], "the one loaded first answers");
}

#[test]
fn nothing_loaded_is_not_an_error() {
    let plugins = Plugins::new();
    assert!(plugins.is_empty());
    assert_eq!(plugins.len(), 0);
    assert_eq!(plugins.commands().len(), 0);
    assert_eq!(plugins.names().len(), 0);
}

#[test]
fn two_plugins_may_share_a_name_and_both_are_kept() {
    // A host that hid one would be hiding the one somebody is looking for.
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

    fn run(&self, _command: &Invocation) -> Option<Vec<String>> {
        let host = self.host.as_ref()?;
        Some(vec![
            host.session_model().unwrap_or_default(),
            host.session_provider().unwrap_or_default(),
            host.session_context().used.to_string(),
        ])
    }
}

#[test]
fn a_plugin_asks_the_host_and_the_host_answers() {
    // The whole reason the host is a trait and not a cast: the plugin
    // reaches the session through a call, and the call lands on the code
    // the host wrote.
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Asking { host: None }));
    plugins.set_context_on_all(&PluginContext {
        host: Some(Arc::new(Answering)),
    });
    let lines = plugins
        .run(&Invocation {
            command: "/asking".to_owned(),
            args: BTreeMap::new(),
        })
        .expect("an answer");
    assert_eq!(lines, ["nvidia/nemotron", "nvidia", "4096"]);
}

#[test]
fn a_context_without_a_host_is_not_an_error() {
    // The host passes what it has. A plugin that needs a host and has none
    // says nothing, and the client falls through to its own commands.
    let context = PluginContext { host: None };
    assert!(context.host.is_none());
}

#[test]
fn the_context_share_is_the_used_part_of_the_window() {
    // What a plugin draws next to the conversation: how much of the model's
    // context the session holds.
    let usage = ContextUsage {
        used: 32_000,
        window: Some(128_000),
    };
    assert_eq!(usage.share(), Some(0.25));
}

#[test]
fn a_session_without_a_window_has_no_share() {
    // A model the catalogue does not describe has no window, and a share
    // of nothing is not a number anybody can act on.
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
    // The catalogue is the sole authority on a model, and a plugin that
    // serves one has to be able to say so without the person writing a
    // file for it.
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Offering));
    let catalogue = plugins.catalogue();
    assert_eq!(catalogue.providers.len(), 1);
    assert_eq!(catalogue.models.len(), 1);
    assert_eq!(catalogue.models[0].key(), "demo/demo-model");
}

#[test]
fn two_plugins_offering_the_same_provider_leave_one() {
    // Two entries with the same id would leave the catalogue saying one
    // thing and meaning another.
    let mut plugins = Plugins::new();
    plugins.add(Box::new(Offering));
    plugins.add(Box::new(Offering));
    let catalogue = plugins.catalogue();
    assert_eq!(catalogue.providers.len(), 1);
    assert_eq!(catalogue.models.len(), 1, "the model is one as well");
}
