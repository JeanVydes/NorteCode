//! The plugins this client ships, and where a host adds its own.
//!
//! A plugin here is Rust compiled into the binary. That is deliberate: a
//! dynamically loaded library would run in this process with this process's
//! memory and would need an ABI both sides keep, and the thing a plugin is
//! allowed to do — produce text the host draws — is something a function can
//! express exactly.
//!
//! # Adding one
//!
//! Write a type that implements [`Plugin`], and add it to [`ALL`]. That is the
//! whole of it:
//!
//! ```ignore
//! struct Clock;
//!
//! impl Plugin for Clock {
//!     fn name(&self) -> &str { "clock" }
//!
//!     // The time, on the status row, where a number is meant to be.
//!     fn status(&self) -> Option<String> {
//!         Some(chrono::Local::now().format("%H:%M").to_string())
//!     }
//! }
//! ```
//!
//! Nothing else changes: the command registry reads [`Plugin::commands`], the
//! frame draws [`Plugin::sidebar`] and [`Plugin::status`], and the startup
//! screen draws [`Plugin::banner`]. A host that adds a plugin adds no code
//! outside this file.
//!
//! # What a plugin cannot do
//!
//! Reach the terminal, block a frame, or wait on anything. It is asked for
//! text and it returns text. That is not a restriction on what a plugin is
//! *for* — the text can say anything, and a panel may hold a report of any
//! size — it is what keeps somebody else's code from being able to break the
//! interface it draws into.

use norte_code_protocol::commands::{ArgSpec, CommandSpec, Invocation};
use norte_code_protocol::plugins::{Plugin, Plugins, SidebarRow};

/// A plugin that shows the shape of one.
///
/// It answers `/example` with a panel, puts two rows beside the conversation
/// and a word on the status row. Every place a plugin can draw is used, which
/// is what makes it a template rather than a decoration.
struct Example;

impl Plugin for Example {
    fn name(&self) -> &'static str {
        "example"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![CommandSpec::new("/example", "a plugin's own command").arg(ArgSpec::rest("text"))]
    }

    fn run(&self, command: &Invocation) -> Option<Vec<String>> {
        if command.command != "/example" {
            return None;
        }
        let text = command.arg_or_empty("text");
        let mut lines = vec!["this panel came from a plugin".to_owned()];
        lines.push(String::new());
        lines.push(if text.is_empty() {
            "it takes the rest of the line as its argument: /example anything".to_owned()
        } else {
            format!("you typed: {text}")
        });
        lines.push(String::new());
        lines.push("esc closes it, arrows walk it, like every panel here".to_owned());
        Some(lines)
    }

    fn sidebar(&self) -> Vec<SidebarRow> {
        vec![SidebarRow::heading("plugin"), SidebarRow::text("example")]
    }

    fn status(&self) -> Option<String> {
        Some("plugin".to_owned())
    }
}

/// Every plugin this client ships.
///
/// The example is here so the plumbing is exercised on every run: a plugin that
/// is only in a test is a plugin nobody notices has stopped working.
const ALL: &[&dyn Plugin] = &[&Example];

/// The plugin with a name, when this client has it.
///
/// The name is the one [`Plugin::name`] gives, compared as written: a plugin
/// that answers to two names is two entries in [`ALL`], and a host that wanted
/// one would say so there.
#[must_use]
pub(crate) fn by_name(name: &str) -> Option<Box<dyn Plugin>> {
    match name {
        "example" => Some(Box::new(Example)),
        _ => None,
    }
}

/// Every plugin, ready to register.
///
/// It is what a host that wants them all calls, and what the loader does not:
/// the loader adds the ones that were asked for.
#[must_use]
#[allow(dead_code)]
pub(crate) fn all() -> Plugins {
    let mut plugins = Plugins::new();
    for plugin in ALL {
        if let Some(plugin) = by_name(plugin.name()) {
            plugins.add(plugin);
        }
    }
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_plugin_can_be_found_by_the_name_it_gives() {
        // The loader finds a plugin by the name it asks for, and a plugin whose
        // name does not resolve is one nobody can turn on. This is the test that
        // notices when the two lists drift.
        for plugin in ALL {
            assert!(
                by_name(plugin.name()).is_some(),
                "`{}` is in ALL and cannot be found by name",
                plugin.name()
            );
        }
    }

    #[test]
    fn a_name_nobody_has_resolves_to_nothing() {
        assert!(by_name("no-such-plugin-anywhere").is_none());
    }

    #[test]
    fn the_example_really_draws_in_every_place() {
        // A template that stopped using a place would stop being one: this is
        // what fails when a plugin API grows a place nobody wired.
        let plugin = Example;
        assert!(!plugin.commands().is_empty(), "it has a command");
        assert!(!plugin.sidebar().is_empty(), "it draws beside");
        assert!(plugin.status().is_some(), "and on the status row");
    }

    #[test]
    fn the_example_answers_its_own_command_and_no_other() {
        let mut registry = norte_code_protocol::commands::Registry::new();
        registry.register(CommandSpec::new("/example", "example").arg(ArgSpec::rest("text")));
        registry.register(CommandSpec::new("/mine", "mine"));
        let mine = registry.parse("/mine").expect("a command").expect("parsed");
        assert!(
            Example.run(&mine).is_none(),
            "a command it did not declare is not its answer to give"
        );
        let its_own = registry
            .parse("/example hello")
            .expect("a command")
            .expect("parsed");
        let lines = Example.run(&its_own).expect("an answer");
        assert!(
            lines.iter().any(|line| line.contains("hello")),
            "and its own carries the argument: {lines:?}"
        );
    }

    #[test]
    fn every_plugin_loaded_together_has_a_name() {
        let plugins = all();
        assert_eq!(plugins.len(), ALL.len());
        for name in plugins.names() {
            assert!(!name.is_empty(), "a plugin with no name cannot be reported");
        }
    }
}
