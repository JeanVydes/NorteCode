//! Plugin declarations, commands and widgets.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::{CommandExecutionError, CommandName, CommandOutput, CommandRegistry};
use norte::AnyFunction;
use norte_code_wire::ContextUsage;
use norte_code_wire::catalog::{Model, Provider};
use norte_code_wire::plugin_widgets::{
    PluginSelection, WidgetId, WidgetKey, WidgetRegistration, WidgetView,
};

/// The version of the plugin interface a plugin was built against.
///
/// A plugin is a shared library loaded into a process that is already running,
/// and the two sides have to agree on the shape of what they pass to each
/// other. They cannot agree at compile time — the plugin is built on its own,
/// against its own copy of this crate — so the version is a number both sides
/// carry and the host refuses a plugin whose number is not its own.
///
/// Changes to the [`Plugin`] vtable or the layout of its argument and return
/// types require every plugin to be rebuilt. The host rejects a different version before constructing the
/// trait object, because calling an old vtable can crash the process.
///
/// The check is an exact match on purpose: a plugin built against another
/// version is a plugin whose method table this host cannot call safely, so it
/// is refused with the two numbers named rather than loaded on a guess.
pub const PLUGIN_ABI_VERSION: u32 = 4;

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

    /// The tools this plugin adds to the tool registry.
    ///
    /// The host registers each one with `ToolRegistry`, so a plugin tool has the
    /// same schema, approval flow and audit record as a built-in tool. The
    /// plugin builds the declarations; the host owns the registry.
    fn tools(&self) -> Vec<Arc<dyn AnyFunction>> {
        Vec::new()
    }

    /// Lists the widgets this plugin can supply to clients.
    fn widgets(&self) -> Vec<WidgetDefinition> {
        Vec::new()
    }

    /// Returns a widget's current view for the caller's selection.
    ///
    /// Return cached data and schedule I/O separately. Notify the host via
    /// [`PluginContext::request_widget_update`] when that widget changes.
    fn widget(&self, _id: &WidgetId, _invocation: &PluginInvocation) -> Option<WidgetView> {
        None
    }

    /// Receives the handle used to notify clients about widget updates.
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

    /// Registers slash commands this plugin can execute.
    fn register_commands(&self, _registry: &mut CommandRegistry) {}

    /// Runs one command previously registered by this plugin.
    ///
    /// # Errors
    ///
    /// Return [`CommandExecutionError::Rejected`] when the supplied values do
    /// not meet the plugin command's constraints, or
    /// [`CommandExecutionError::Failed`] when the action cannot complete.
    fn execute_command(
        &self,
        _command: &CommandName,
        _arguments: &[String],
        _invocation: Option<&PluginInvocation>,
    ) -> Result<CommandOutput, CommandExecutionError> {
        Err(CommandExecutionError::UnknownCommand)
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

/// Request-scoped session information provided by the host to a plugin.
pub struct PluginInvocation {
    /// The requesting client's session, provider and model.
    pub selection: PluginSelection,
    /// Server-known title of the invoking session, if any.
    pub title: Option<String>,
    /// Server-known context usage of the invoking session.
    pub context_usage: ContextUsage,
}

/// Metadata for a widget declared by one plugin.
pub struct WidgetDefinition {
    /// Stable identifier within the plugin.
    pub id: WidgetId,
    /// A title a client can show in its widget picker.
    pub title: String,
}

/// The update handle bound to one loaded plugin.
#[derive(Clone)]
pub struct PluginContext {
    invalidate_widget: Arc<dyn Fn(&WidgetId) + Send + Sync>,
}

impl PluginContext {
    /// Constructs a context bound to the declaring plugin.
    pub fn new(invalidate_widget: impl Fn(&WidgetId) + Send + Sync + 'static) -> Self {
        Self {
            invalidate_widget: Arc::new(invalidate_widget),
        }
    }

    /// Requests new content for one of this plugin's registered widgets.
    pub fn request_widget_update(&self, id: &WidgetId) {
        (self.invalidate_widget)(id);
    }
}

impl fmt::Debug for PluginContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginContext").finish_non_exhaustive()
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

    /// Every tool every plugin adds, in load order.
    ///
    /// The host registers these with `ToolRegistry`, which rejects duplicate
    /// names. Two plugins offering the same tool name is a conflict the host
    /// reports, not one it hides.
    #[must_use]
    pub fn tools(&self) -> Vec<Arc<dyn AnyFunction>> {
        self.loaded
            .iter()
            .flat_map(|plugin| plugin.tools())
            .collect()
    }

    /// Collects declarations in plugin load order.
    #[must_use]
    pub fn widget_registrations(&self) -> Vec<WidgetRegistration> {
        let mut known = std::collections::BTreeSet::new();
        self.loaded
            .iter()
            .flat_map(|plugin| {
                plugin
                    .widgets()
                    .into_iter()
                    .map(|definition| WidgetRegistration {
                        key: WidgetKey {
                            plugin: plugin.name().to_owned(),
                            widget: definition.id,
                        },
                        title: definition.title,
                    })
            })
            .filter(|entry| {
                let id = entry.key.widget.as_str();
                !id.is_empty() && id.len() <= 128 && known.insert(entry.key.clone())
            })
            .collect()
    }

    /// Evaluates one registered widget for the requesting client.
    #[must_use]
    pub fn widget(&self, key: &WidgetKey, invocation: &PluginInvocation) -> Option<WidgetView> {
        self.loaded
            .iter()
            .find(|plugin| plugin.name() == key.plugin)?
            .widget(&key.widget, invocation)
    }

    /// Every tool paired with the name of the plugin that declared it.
    #[must_use]
    pub fn tools_named(&self) -> Vec<(String, Arc<dyn AnyFunction>)> {
        self.loaded
            .iter()
            .flat_map(|plugin| {
                let name = plugin.name().to_owned();
                plugin
                    .tools()
                    .into_iter()
                    .map(move |tool| (name.clone(), tool))
            })
            .collect()
    }

    /// Builds the command registry from every loaded plugin.
    #[must_use]
    pub fn command_registry(&self) -> CommandRegistry {
        let mut registry = CommandRegistry::default();
        for (index, plugin) in self.loaded.iter().enumerate() {
            registry.set_registration_owner(index, plugin.name().to_owned());
            plugin.register_commands(&mut registry);
            registry.clear_registration_owner();
        }
        registry
    }

    /// Executes a registered command through the plugin that owns it.
    ///
    /// # Errors
    ///
    /// Returns an error when the command is not registered, its arguments do
    /// not match its declaration, or the owning plugin rejects or fails it.
    pub fn execute_command(
        &self,
        registry: &CommandRegistry,
        command: &CommandName,
        arguments: &[String],
        invocation: Option<&PluginInvocation>,
    ) -> Result<CommandOutput, CommandExecutionError> {
        let registered = registry
            .get(command)
            .ok_or(CommandExecutionError::UnknownCommand)?;
        let plugin = registered
            .owner_index
            .and_then(|index| self.loaded.get(index))
            .or_else(|| {
                self.loaded
                    .iter()
                    .find(|plugin| plugin.name() == registered.plugin)
            })
            .ok_or(CommandExecutionError::UnknownCommand)?;
        let argument_count = arguments.len();
        if argument_count > registered.spec.arguments.len()
            || registered
                .spec
                .arguments
                .iter()
                .take(argument_count)
                .zip(arguments)
                .any(|(spec, value)| !spec.choices.is_empty() && !spec.choices.contains(value))
            || registered
                .spec
                .arguments
                .iter()
                .skip(argument_count)
                .any(|spec| spec.required)
        {
            return Err(CommandExecutionError::Rejected(
                "arguments do not match the registered command".to_owned(),
            ));
        }
        plugin.execute_command(command, arguments, invocation)
    }

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
