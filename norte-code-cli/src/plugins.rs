//! Plugin loading for Norte Code.
//!
//! Plugins live in ~/.config/norte-code/plugins/ as dynamic libraries
//! (.so on Linux, .dll on Windows). Each library must export a
//! `create_plugin()` function returning `Box<dyn Plugin>`.

use norte_code_protocol::plugins::{PluginContext, Plugins};

/// Loads every plugin found in ~/.config/norte-code/plugins/ (or the
/// directory named by `NORTE_CODE_PLUGINS`) and hands the context to each.
pub(crate) fn all(ctx: Option<&PluginContext>) -> Plugins {
    let mut plugins = Plugins::new();
    crate::plugin_loader::load_plugins(&mut plugins, ctx);
    plugins
}
