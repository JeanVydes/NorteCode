//! Display lines and tools contributed by plugins loaded at server start.
//!
//! A plugin answers `status()`, `banner()` and `tools()`. The host captures what
//! they offer once, before anything else asks the plugins, and keeps each line
//! beside the name of the plugin that offered it. `PluginInfo` carries the lines
//! and tool names as part of the plugin's own metadata. No second event is
//! needed to paint the sidebar or list a plugin's tools.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use norte_code_plugin_host::Plugins;

#[derive(Clone, Debug, Default)]
struct PluginOfferings {
    status: BTreeMap<String, String>,
    banner: BTreeMap<String, String>,
    tools_by_plugin: BTreeMap<String, Vec<String>>,
}

static OFFERINGS: OnceLock<PluginOfferings> = OnceLock::new();

/// The tools the loaded plugins offered, with the name of each declaring plugin.
#[derive(Clone, Debug, Default)]
pub(crate) struct PluginTools {
    pub(crate) functions: Vec<Arc<dyn norte::AnyFunction>>,
    pub(crate) origins: BTreeMap<String, String>,
}

/// Captures everything the loaded plugins offer at startup.
pub(crate) fn capture(plugins: &Plugins) -> PluginTools {
    let named = plugins.tools_named();
    let mut functions = Vec::with_capacity(named.len());
    let mut origins = BTreeMap::new();
    let mut tools_by_plugin: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (plugin, tool) in named {
        let name = tool.name().to_owned();
        origins.insert(name.clone(), plugin.clone());
        tools_by_plugin.entry(plugin).or_default().push(name);
        functions.push(tool);
    }
    let _ = OFFERINGS.set(PluginOfferings {
        status: plugins.status_named().into_iter().collect(),
        banner: plugins.banner_named().into_iter().collect(),
        tools_by_plugin,
    });
    PluginTools { functions, origins }
}

/// The status line a named plugin offered, when it offered one.
pub(crate) fn status_for(name: &str) -> Option<String> {
    OFFERINGS.get()?.status.get(name).cloned()
}

/// The banner line a named plugin offered, when it offered one.
pub(crate) fn banner_for(name: &str) -> Option<String> {
    OFFERINGS.get()?.banner.get(name).cloned()
}

/// The names of the tools a named plugin declared.
pub(crate) fn tools_for(name: &str) -> Vec<String> {
    OFFERINGS
        .get()
        .and_then(|offerings| offerings.tools_by_plugin.get(name))
        .cloned()
        .unwrap_or_default()
}
