//! Loading plugins from ~/.config/norte-code/plugins/ (or the directory named
//! by `NORTE_CODE_PLUGINS`).
//!
//! Each plugin is a dynamic library (.so on Linux, .dll on Windows) that
//! exports a `create_plugin()` function returning the plugin, and a
//! `norte_plugin_abi_version()` function saying which version of the plugin
//! interface it was built against. The host loads them with libloading, calls
//! `create_plugin()`, and hands the resulting plugin to the Plugins registry.
//!
//! A plugin that fails to load is reported and skipped: the client starts
//! either way.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use norte_code_protocol::plugins::{
    PLUGIN_ABI_SYMBOL, PLUGIN_ABI_VERSION, Plugin, PluginContext, Plugins,
};

/// The libraries already loaded, kept for the life of the process.
///
/// A plugin's vtable points into its library, so the library has to outlive
/// the plugin: it is never unloaded. Loading the file once and keeping it
/// here is what lets two registries hold two instances of the same plugin
/// without opening the file twice — and without leaking a handle per call.
static LIBRARIES: Mutex<Vec<(PathBuf, Arc<libloading::Library>)>> = Mutex::new(Vec::new());

/// Loads every plugin found in the plugin directory and hands the context to
/// each one.
///
/// The plugin directory is `NORTE_CODE_PLUGINS` if set, otherwise
/// `~/.config/norte-code/plugins/`.
pub(crate) fn load_plugins(plugins: &mut Plugins, ctx: Option<&PluginContext>) {
    let plugin_dir = plugin_dir();
    let Ok(entries) = std::fs::read_dir(&plugin_dir) else {
        return;
    };
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
            Err(error) => {
                eprintln!("warning: could not load plugin {}: {error}", path.display());
            }
        }
    }
}

/// Loads a single plugin library.
///
/// # Safety
///
/// This function calls an externally provided `create_plugin` function and
/// constructs a `Box<dyn Plugin>` from the raw pointer it returns. The plugin
/// library must export a `create_plugin` symbol with the correct signature,
/// and the plugin must be ABI-compatible with the host — which is what the
/// version check below is there to establish.
#[allow(unsafe_code)]
fn load_one_plugin(path: &Path, ctx: Option<&PluginContext>) -> Result<Box<dyn Plugin>, String> {
    let library = library(path)?;
    check_abi(&library)?;
    let create: libloading::Symbol<unsafe extern "C" fn() -> *mut dyn Plugin> = unsafe {
        library
            .get(b"create_plugin\0")
            .map_err(|e| format!("does not export create_plugin: {e}"))?
    };

    let mut plugin = unsafe { Box::from_raw(create()) };
    if let Some(ctx) = ctx {
        plugin.set_context(ctx.clone());
    }

    Ok(plugin)
}

/// The name a plugin library answers with, or why it cannot be loaded.
///
/// It loads the plugin for real: a library that cannot be loaded is one the
/// client will skip, and a list that says a plugin is installed when the
/// client cannot use it is a list nobody can act on.
pub(crate) fn describe(path: &Path) -> Result<String, String> {
    Ok(load_one_plugin(path, None)?.name().to_owned())
}

/// The library at `path`, loaded once.
///
/// The library is kept for the life of the process because a plugin's vtable
/// points into it: unloading it would leave the host calling into a library
/// that is gone.
fn library(path: &Path) -> Result<Arc<libloading::Library>, String> {
    let mut libraries = LIBRARIES
        .lock()
        .map_err(|_| "the plugin registry is poisoned")?;
    if let Some((_, library)) = libraries.iter().find(|(known, _)| known == path) {
        return Ok(Arc::clone(library));
    }
    let library = Arc::new(
        unsafe { libloading::Library::new(path) }
            .map_err(|e| format!("could not load library: {e}"))?,
    );
    libraries.push((path.to_path_buf(), Arc::clone(&library)));
    Ok(library)
}

/// Checks that the plugin speaks this host's plugin interface.
///
/// The two sides pass each other structures whose layout has to match, and
/// they cannot agree on that at compile time: the plugin is built on its own,
/// against its own copy of the protocol crate. The version is how the two
/// say so out loud, and a plugin that does not say it is refused rather than
/// loaded on a guess.
fn check_abi(library: &libloading::Library) -> Result<(), String> {
    let version: libloading::Symbol<unsafe extern "C" fn() -> u32> = unsafe {
        library.get(PLUGIN_ABI_SYMBOL).map_err(|e| {
            format!(
                "does not export norte_plugin_abi_version, so it cannot be \
                 checked against this host: {e}"
            )
        })?
    };
    let theirs = unsafe { version() };
    if theirs != PLUGIN_ABI_VERSION {
        return Err(format!(
            "was built against plugin interface {theirs}, and this host speaks \
             {PLUGIN_ABI_VERSION}"
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_that_is_not_loaded_is_not_in_the_registry() {
        // The registry is a cache of what was loaded, and a path that was
        // never loaded is not in it: the next load of it opens the file.
        let libraries = LIBRARIES.lock().expect("the registry");
        assert!(
            !libraries.iter().any(|(path, _)| path.ends_with("never.so")),
            "a path that was never loaded is not in the registry"
        );
    }
}
