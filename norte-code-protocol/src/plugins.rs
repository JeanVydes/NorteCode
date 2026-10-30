//! The plugin registry: what a plugin may add to a Norte Code client.
//!
//! A plugin is a box that is asked for things. It answers with **data** — a
//! command to register, lines for a panel, rows for a sidebar, a fragment of
//! the wordmark — and the host draws it. That is the whole design, and
//! everything else follows from it:
//!
//! - A plugin **cannot** write to the terminal. It has no way to reach it, so
//!   it cannot clear a line it does not own, leave the cursor somewhere, or
//!   fight the frame for a row. The bugs this client has spent its life fixing
//!   are not available to a plugin because the API does not hand them over.
//! - A plugin **cannot** block. Everything here is a function that returns; a
//!   plugin that wants to wait owns its own task and hands back what it found.
//! - A plugin **is testable without a terminal**, because what it produces is
//!   text and the text is what a test reads.
//!
//! # What a plugin can add
//!
//! | Where | What |
//! | --- | --- |
//! | The command registry | a command, with its arguments and its usage line |
//! | A panel | lines over the interface, opened by a command |
//! | A sidebar | rows beside the conversation, redrawn as they change |
//! | The footer | a field on the status row |
//! | The screen | a line of the wordmark, and the tagline under it |
//!
//! # What it cannot
//!
//! Write to the terminal, block a frame, reach the service, read a file, or
//! keep state the host did not hand it. A plugin that needs any of those is a
//! host feature, and the answer is to write it here rather than to widen this.
//!
//! # Extending it
//!
//! Adding a place a plugin may draw into is adding a method here and a drawing
//! function in the host — one place each, and the tests for the host's side can
//! use the same `Plugin` trait a real plugin does. Nothing here knows what a
//! plugin is for; that is the point.

use std::collections::BTreeMap;

use crate::commands::{CommandSpec, Invocation};

/// What a plugin produces, in the places the host draws.
///
/// Every method has a default that adds nothing, so a plugin implements the one
/// or two it cares about and says nothing about the rest. That is what keeps a
/// plugin that adds a command from having to know what a sidebar is.
pub trait Plugin: Send + Sync {
    /// The name the host reports it by, in `/plugins` and in an error.
    fn name(&self) -> &str;

    /// The commands this plugin adds to the registry.
    ///
    /// They are parsed, completed and listed by the host exactly like its own:
    /// a plugin's command is a command, and the difference is not visible to
    /// whoever types it.
    fn commands(&self) -> Vec<CommandSpec> {
        Vec::new()
    }

    /// What a command of this plugin does, as lines for a panel.
    ///
    /// `Some(lines)` opens a panel; `None` means the command was not this
    /// plugin's. The lines are plain text, and the host colours, wraps and
    /// scrolls them: a plugin says what to read and not how it looks.
    fn run(&self, _command: &Invocation) -> Option<Vec<String>> {
        None
    }

    /// The rows this plugin puts beside the conversation.
    ///
    /// Called when the host repaints, so a plugin that wants a number to move
    /// returns a different one; it is text and not a widget, so nothing here
    /// can hold the frame.
    fn sidebar(&self) -> Vec<SidebarRow> {
        Vec::new()
    }

    /// A field on the status row, under the conversation.
    ///
    /// `None` adds nothing. A row is one line and the host bounds it, so a
    /// plugin that returns a paragraph gets its beginning and no more.
    fn status(&self) -> Option<String> {
        None
    }

    /// One line of the screen's wordmark, drawn under the built-in one.
    ///
    /// A plugin that wants its own banner says it here rather than printing it:
    /// the screen has one place for art and this is it.
    fn banner(&self) -> Option<String> {
        None
    }
}

/// One row of the sidebar a plugin asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarRow {
    /// What the row says.
    pub text: String,
    /// Whether it is a heading, which the host draws in the foreground ink.
    pub heading: bool,
}

impl SidebarRow {
    /// A row of body text.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            heading: false,
        }
    }

    /// A row that names a section.
    #[must_use]
    pub fn heading(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            heading: true,
        }
    }
}

/// Every plugin this client loaded.
///
/// It is a plain collection with no interior mutability: plugins are loaded
/// once, at startup, and nothing adds one while a turn runs. A registry that
/// could change under a frame would need a lock in the drawing path, which is
/// how a plugin ends up able to stall the interface.
#[derive(Default)]
pub struct Plugins {
    loaded: Vec<Box<dyn Plugin>>,
}

impl Plugins {
    /// A registry with nothing in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a plugin.
    ///
    /// The name is kept as given, including a repeat: two plugins with the same
    /// name are two plugins, and a host that hid one would be hiding the one
    /// the person is looking for. What a repeat breaks is the reporting, and
    /// the report says both.
    pub fn add(&mut self, plugin: Box<dyn Plugin>) {
        self.loaded.push(plugin);
    }

    /// Whether any plugin was loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty()
    }

    /// How many are loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.loaded.len()
    }

    /// The name of every plugin, in the order they were added.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.loaded.iter().map(|plugin| plugin.name()).collect()
    }

    /// Every command every plugin adds.
    #[must_use]
    pub fn commands(&self) -> Vec<CommandSpec> {
        self.loaded
            .iter()
            .flat_map(|plugin| plugin.commands())
            .collect()
    }

    /// Asks every plugin to run a command, and takes the first answer.
    ///
    /// The first and not every one: two plugins claiming the same name is two
    /// answers to one command, and the second would be answering a question
    /// nobody asked it. The order is the order they were loaded in, which is
    /// the order they were declared in.
    #[must_use]
    pub fn run(&self, command: &Invocation) -> Option<Vec<String>> {
        self.loaded.iter().find_map(|plugin| plugin.run(command))
    }

    /// Every sidebar row, grouped by the plugin that asked for it.
    ///
    /// Grouped rather than flat so a person can tell which plugin a row came
    /// from: a list of rows with no attribution is one nobody can turn off.
    #[must_use]
    pub fn sidebar(&self) -> BTreeMap<&str, Vec<SidebarRow>> {
        let mut out: BTreeMap<&str, Vec<SidebarRow>> = BTreeMap::new();
        for plugin in &self.loaded {
            let rows = plugin.sidebar();
            if !rows.is_empty() {
                out.insert(plugin.name(), rows);
            }
        }
        out
    }

    /// The status fields, in the order the plugins were loaded.
    #[must_use]
    pub fn status(&self) -> Vec<String> {
        self.loaded
            .iter()
            .filter_map(|plugin| plugin.status())
            .collect()
    }

    /// The banner lines, in the order the plugins were loaded.
    #[must_use]
    pub fn banner(&self) -> Vec<String> {
        self.loaded
            .iter()
            .filter_map(|plugin| plugin.banner())
            .collect()
    }
}

impl fmt::Debug for Plugins {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plugins")
            .field("names", &self.names())
            .finish()
    }
}

use std::fmt;

#[cfg(test)]
mod tests {
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

        fn sidebar(&self) -> Vec<SidebarRow> {
            vec![SidebarRow::heading("hello"), SidebarRow::text("world")]
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
        // should not have to know what a sidebar is to compile.
        let plugins = {
            let mut plugins = Plugins::new();
            plugins.add(Box::new(Only));
            plugins
        };
        assert_eq!(plugins.commands().len(), 1);
        assert_eq!(plugins.sidebar().len(), 0);
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
    fn the_sidebar_says_which_plugin_a_row_came_from() {
        // A list of rows with no attribution is one nobody can turn off.
        let mut plugins = Plugins::new();
        plugins.add(Box::new(Only));
        plugins.add(Box::new(Everything));
        let sidebar = plugins.sidebar();
        assert_eq!(sidebar.len(), 1, "the plugin with no rows adds none");
        let rows = sidebar.get("everything").expect("its rows");
        assert_eq!(rows.len(), 2);
        assert!(rows[0].heading, "the first row names a section");
        assert!(!rows[1].heading);
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
}
