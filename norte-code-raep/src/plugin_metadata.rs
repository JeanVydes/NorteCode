//! Tools and commands contributed by plugins loaded at server start.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use norte_code_plugin_host::Plugins;

#[derive(Clone, Debug, Default)]
struct PluginOfferings {
    widgets: Vec<norte_code_wire::plugin_widgets::WidgetRegistration>,
    tools_by_plugin: BTreeMap<String, Vec<String>>,
    commands_by_plugin: BTreeMap<String, Vec<norte_code_wire::PluginCommandInfo>>,
}

static OFFERINGS: OnceLock<PluginOfferings> = OnceLock::new();

/// The tools the loaded plugins offered, with the name of each declaring plugin.
#[derive(Clone, Debug, Default)]
pub(crate) struct PluginTools {
    pub(crate) functions: Vec<Arc<dyn norte::AnyFunction>>,
    pub(crate) origins: BTreeMap<String, String>,
}

/// Captures everything the loaded plugins offer at startup.
pub(crate) fn capture(
    plugins: &Plugins,
    widgets: Vec<norte_code_wire::plugin_widgets::WidgetRegistration>,
) -> PluginTools {
    let named = plugins.tools_named();
    let mut functions = Vec::with_capacity(named.len());
    let mut origins = BTreeMap::new();
    let mut tools_by_plugin: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut commands_by_plugin: BTreeMap<String, Vec<norte_code_wire::PluginCommandInfo>> =
        BTreeMap::new();
    for (plugin, tool) in named {
        let name = tool.name().to_owned();
        origins.insert(name.clone(), plugin.clone());
        tools_by_plugin.entry(plugin).or_default().push(name);
        functions.push(tool);
    }
    for command in plugins.command_registry().commands() {
        commands_by_plugin
            .entry(command.plugin.clone())
            .or_default()
            .push(norte_code_wire::PluginCommandInfo {
                name: command.spec.name.as_str().to_owned(),
                description: command.spec.description.clone(),
                arguments: command
                    .spec
                    .arguments
                    .iter()
                    .map(|argument| norte_code_wire::PluginCommandArgumentInfo {
                        name: argument.name.clone(),
                        description: argument.description.clone(),
                        required: argument.required,
                        choices: argument.choices.clone(),
                    })
                    .collect(),
            });
    }
    let _ = OFFERINGS.set(PluginOfferings {
        widgets,
        tools_by_plugin,
        commands_by_plugin,
    });
    PluginTools { functions, origins }
}

/// The names of the tools a named plugin declared.
pub(crate) fn tools_for(name: &str) -> Vec<String> {
    OFFERINGS
        .get()
        .and_then(|offerings| offerings.tools_by_plugin.get(name))
        .cloned()
        .unwrap_or_default()
}

/// The slash commands a named plugin registered.
pub(crate) fn commands_for(name: &str) -> Vec<norte_code_wire::PluginCommandInfo> {
    OFFERINGS
        .get()
        .and_then(|offerings| offerings.commands_by_plugin.get(name))
        .cloned()
        .unwrap_or_default()
}

/// Widget declarations advertised by a loaded plugin.
pub(crate) fn widgets_for(name: &str) -> Vec<norte_code_wire::plugin_widgets::WidgetRegistration> {
    OFFERINGS.get().map_or_else(Vec::new, |offerings| {
        offerings
            .widgets
            .iter()
            .filter(|entry| entry.key.plugin == name)
            .cloned()
            .collect()
    })
}
