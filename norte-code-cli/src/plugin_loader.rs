//! Loading plugins from ~/.config/norte-code/plugins/ (or the directory named
//! by `NORTE_CODE_PLUGINS`).
//!
//! Each plugin is a dynamic library (.so on Linux, .dll on Windows) that
//! exports a `create_plugin()` function returning `Box<dyn Plugin>`. The host
//! loads them with libloading, calls `create_plugin()`, and hands the
//! resulting plugin to the Plugins registry.
//!
//! A plugin that fails to load is reported and skipped: the client starts
//! either way.

use std::path::PathBuf;

use norte_code_protocol::plugins::{Plugin, PluginContext, Plugins};

/// Loads every plugin found in the plugin directory and hands the context to
/// each one.
///
/// The plugin directory is `NORTE_CODE_PLUGINS` if set, otherwise
/// `~/.config/norte-code/plugins/`.
pub(crate) fn load_plugins(plugins: &mut Plugins, ctx: Option<&PluginContext>) {
    let plugin_dir = plugin_dir();
    if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            // Only load shared libraries: .so on Linux, .dll on Windows,
            // .dylib on macOS.
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                continue;
            };
            if ext != "so" && ext != "dll" && ext != "dylib" {
                continue;
            }

            match load_one_plugin(&path, ctx) {
                Ok(plugin) => plugins.add(plugin),
                Err(error) => eprintln!(
                    "warning: could not load plugin {}: {error}",
                    path.display()
                ),
            }
        }
    }
}

/// Loads a single plugin library.
///
/// # Safety
///
/// This function calls an externally provided `create_plugin` function and
/// constructs a `Box<dyn Plugin>` from its raw pointer. The plugin library
/// must export a `create_plugin` symbol with the correct signature, and the
/// plugin must be ABI-compatible with the host.
#[allow(unsafe_code)]
fn load_one_plugin(path: &std::path::Path, ctx: Option<&PluginContext>) -> Result<Box<dyn Plugin>, String> {
    let lib = unsafe { libloading::Library::new(path) }
        .map_err(|e| format!("could not load library: {e}"))?;

    let create: libloading::Symbol<unsafe extern "C" fn() -> *mut dyn Plugin> = unsafe {
        lib.get(b"create_plugin\0")
            .map_err(|e| format!("does not export create_plugin: {e}"))?
    };

    let mut plugin = unsafe { Box::from_raw(create()) };
    if let Some(ctx) = ctx {
        plugin.set_context(ctx.clone());
    }

    // Keep the library loaded for the lifetime of the process so the plugin's
    // vtable stays valid. We never drop it.
    std::mem::forget(lib);

    Ok(plugin)
}

/// The directory where plugin libraries live.
fn plugin_dir() -> PathBuf {
    if let Ok(value) = std::env::var("NORTE_CODE_PLUGINS") {
        return PathBuf::from(value);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return PathBuf::new();
    }
    PathBuf::from(home).join(".config/norte-code/plugins")
}
