//! The plugin registry: what a plugin may add to a Norte Code client.
//!
//! A plugin is a box that is asked for things. It answers with **data** — a
//! command to register, lines for a panel, a fragment of
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
//! | The footer | a field on the status row |
//! | The screen | a line of the wordmark, and the tagline under it |
//!
//! # What it cannot
//!
//! Write to the terminal or block a frame — those remain the host's job.
//! Everything else is possible: a plugin can ask the host about the session
//! through the context the host hands it ([`PluginContext::host`]), keep
//! whatever state it wants in its own fields, and offer providers and models
//! of its own to the catalogue ([`Plugin::catalogue`]). The trait itself
//! carries no state; a plugin that needs to remember something stores it in a
//! field of its own type.
//!
//! # Extending it
//!
//! Adding a place a plugin may draw into is adding a method here and a drawing
//! function in the host — one place each, and the tests for the host's side can
//! use the same `Plugin` trait a real plugin does. Nothing here knows what a
//! plugin is for; that is the point.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::commands::{CommandSpec, Invocation};
use norte_code_wire::SessionKey;
use norte_code_wire::catalog::{Model, Provider};

/// The version of the plugin interface a plugin was built against.
///
/// A plugin is a shared library loaded into a process that is already running,
/// and the two sides have to agree on the shape of what they pass to each
/// other. They cannot agree at compile time — the plugin is built on its own,
/// against its own copy of this crate — so the version is a number both sides
/// carry and the host refuses a plugin whose number is not its own.
///
/// Bump it when the layout of [`PluginContext`], [`Plugin`] or [`PluginHost`]
/// changes in a way an old plugin would not survive.
///
/// The check is an exact match on purpose: a plugin built against another
/// version is a plugin whose method table this host cannot call safely, so it
/// is refused with the two numbers named rather than loaded on a guess. A
/// plugin built against an older release must be rebuilt against this one.
pub const PLUGIN_ABI_VERSION: u32 = 1;

/// The symbol a plugin exports to say which version it was built against.
///
/// The name is part of the interface: the host looks it up by this exact
/// spelling, and a plugin that does not export it is refused rather than
/// loaded on a guess.
pub const PLUGIN_ABI_SYMBOL: &[u8] = b"norte_plugin_abi_version\0";

/// What a plugin produces, in the places the host draws.
///
/// Every method has a default that adds nothing, so a plugin implements the one
/// or two it cares about and says nothing about the rest. That is what keeps a
/// plugin that adds a command from having to know what a panel is.
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

    /// Receives the host context before the host asks anything of the plugin.
    ///
    /// The host calls this once, before `commands`, `run`, `status`
    /// or `banner`. A plugin stores whatever it wants from the context in its
    /// own fields — the trait does not keep a copy. The host is passed as a
    /// [`PluginHost`] so this crate never names a concrete service type; the
    /// host that built the context implements it.
    ///
    /// The default implementation ignores the context. A plugin that wants the
    /// host overrides this and keeps it.
    fn set_context(&mut self, _ctx: PluginContext) {}

    /// The providers and models this plugin offers the catalogue.
    ///
    /// A plugin that serves a model the catalogue does not know offers it
    /// here, and it is in the catalogue for this run: the files a person owns
    /// are not written and do not have to be. The shapes are the ones the two
    /// files carry, so a plugin that has the JSON hands it over as it is.
    fn catalogue(&self) -> Catalogue {
        Catalogue::default()
    }
}

/// What a plugin offers the catalogue.
///
/// The same two shapes `providers.json` and `models.json` carry, so a plugin
/// that has them as JSON deserializes them and hands them over: the format a
/// person writes is the format a plugin speaks.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Catalogue {
    /// The providers, by id.
    #[serde(default)]
    pub providers: BTreeMap<String, Provider>,
    /// The models, in the order the plugin listed them.
    #[serde(default)]
    pub models: Vec<Model>,
}

/// How much of a model's context a session is using.
///
/// The context is what the last request carried: the whole conversation is
/// sent again every time, so the input tokens of the last request are the
/// context, and the window is what the catalogue says the model holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextUsage {
    /// The tokens the last request carried.
    pub used: u64,
    /// The model's context window, when the catalogue knows it.
    pub window: Option<u64>,
}

impl ContextUsage {
    /// The share of the window in use, from 0 to 1, when the window is known.
    ///
    /// The share is a thousandth of the window computed in whole numbers: a
    /// token count is 64 bits wide and a float holds 52, and the difference is
    /// a rounding nobody can see in a number that is a percentage. A session
    /// that holds more than the window holds reports a full window, because
    /// that is what a full window looks like.
    #[must_use]
    pub fn share(&self) -> Option<f64> {
        let window = self.window.filter(|window| *window > 0)?;
        let thousandths = (u128::from(self.used) * 1_000) / u128::from(window);
        let thousandths = u32::try_from(thousandths).unwrap_or(1_000);
        Some(f64::from(thousandths) / 1_000.0)
    }
}

/// What a plugin may ask the host about the session it is running in.
///
/// The host implements it and the plugin calls it, and that is the whole
/// reason it is a trait and not a cast.
///
/// A plugin is a shared library with its **own copy** of this crate and of
/// everything it depends on, so the two sides do not share a single type: a
/// `TypeId` is only the same on both sides when the type comes from the same
/// compilation, and it does not. Casting the host's service to a concrete type
/// therefore fails every time, and the plugin is left holding nothing — which
/// is what "the service is not available" was, every time it was said.
///
/// A trait object crosses that line, because the call goes through the vtable
/// the *host* built. The plugin never names the host's types and never needs
/// to compile against them, which is also what lets a plugin depend on this
/// crate alone.
pub trait PluginHost: Send + Sync {
    /// The session the client is in, if there is one.
    ///
    /// It is asked for rather than handed over because the session moves: a
    /// compaction opens another one, and a plugin that was told the old key
    /// would keep reporting the old model.
    fn session(&self) -> Option<SessionKey>;

    /// The model that session runs on.
    fn session_model(&self) -> Option<String>;

    /// The provider serving it.
    fn session_provider(&self) -> Option<String>;

    /// The title of the session, when it has one.
    fn session_title(&self) -> Option<String>;

    /// How much of the model's context the session is using.
    fn session_context(&self) -> ContextUsage;
}

/// The host context handed to a plugin before the host asks anything of it.
///
/// The host is optional: the host passes what it has at the moment, and a
/// plugin that needs something absent simply does without it. The plugin may
/// keep the context — or any part of it — in its own fields and use it later,
/// when the host asks it to run a command or redraw.
#[derive(Clone)]
pub struct PluginContext {
    /// The host, which answers what the plugin may ask about the session.
    ///
    /// The host that built the context implements it and hands the plugin a
    /// handle to it. A plugin that wants the model, the provider, the title or
    /// the context asks the host when it needs them.
    pub host: Option<Arc<dyn PluginHost>>,
}

impl fmt::Debug for PluginContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The host is a trait object and says nothing about itself, so what is
        // written is what a reader can act on: there is a host to ask, or
        // there is not.
        f.debug_struct("PluginContext")
            .field("host", &self.host.as_ref().map(|_| "yes"))
            .finish()
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

    /// Give every loaded plugin the same host context.
    ///
    /// Called after the service and session exist, so plugins can reach them
    /// when they run a command or redraw.
    pub fn set_context_on_all(&mut self, ctx: &PluginContext) {
        for plugin in &mut self.loaded {
            plugin.set_context(ctx.clone());
        }
    }

    /// What every loaded plugin offers the catalogue, merged.
    ///
    /// A provider two plugins offer is one provider and a model two plugins
    /// offer is one model: the first one wins and the second is left out,
    /// because two entries with the same id would leave the catalogue saying
    /// one thing and meaning another.
    #[must_use]
    pub fn catalogue(&self) -> Catalogue {
        let mut catalogue = Catalogue::default();
        let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for plugin in &self.loaded {
            let offered = plugin.catalogue();
            for (id, provider) in offered.providers {
                catalogue.providers.entry(id).or_insert(provider);
            }
            for model in offered.models {
                let key = model.key();
                if !keys.insert(key) {
                    continue;
                }
                catalogue.models.push(model);
            }
        }
        catalogue
    }
}

impl fmt::Debug for Plugins {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plugins")
            .field("names", &self.names())
            .finish()
    }
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
