//! Dynamic plugin loading for the Norte Code server.
//!
//! This is the only crate allowed to cross the dynamic-library boundary. The
//! RAEP server remains `forbid(unsafe_code)` and receives an ordinary
//! [`Plugins`] registry from this crate.

#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub mod commands;
pub mod plugins;

pub use plugins::{PLUGIN_ABI_SYMBOL, PLUGIN_ABI_VERSION, Plugin, PluginContext, Plugins};

static LIBRARIES: Mutex<Vec<(PathBuf, Arc<libloading::Library>)>> = Mutex::new(Vec::new());

/// Loads all enabled dynamic plugins from the configured plugin directory.
///
/// Invalid or incompatible libraries are reported and skipped so one broken
/// optional plugin cannot prevent the server from starting.
pub fn load_plugins(plugins: &mut Plugins, ctx: Option<&PluginContext>) {
    let plugin_dir = plugin_dir();
    let Ok(entries) = std::fs::read_dir(&plugin_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !matches!(ext, "so" | "dll" | "dylib") {
            continue;
        }
        match load_one(&path, ctx) {
            Ok(plugin) => plugins.add(plugin),
            Err(error) => {
                eprintln!("warning: could not load plugin {}: {error}", path.display());
            }
        }
    }
}

fn load_one(path: &Path, ctx: Option<&PluginContext>) -> Result<Box<dyn Plugin>, String> {
    let library = library(path)?;
    check_abi(&library)?;
    let create: libloading::Symbol<unsafe extern "C" fn() -> *mut dyn Plugin> =
        unsafe { library.get(b"create_plugin\0") }
            .map_err(|error| format!("does not export create_plugin: {error}"))?;
    let raw = unsafe { create() };
    if raw.is_null() {
        return Err("create_plugin returned a null pointer".to_owned());
    }
    let mut plugin = unsafe { Box::from_raw(raw) };
    if let Some(ctx) = ctx {
        plugin.set_context(ctx.clone());
    }
    Ok(plugin)
}

fn library(path: &Path) -> Result<Arc<libloading::Library>, String> {
    let mut libraries = LIBRARIES
        .lock()
        .map_err(|_| "the plugin registry is poisoned".to_owned())?;
    if let Some((_, library)) = libraries.iter().find(|(known, _)| known == path) {
        return Ok(Arc::clone(library));
    }
    let library = Arc::new(
        unsafe { libloading::Library::new(path) }
            .map_err(|error| format!("could not load library: {error}"))?,
    );
    libraries.push((path.to_path_buf(), Arc::clone(&library)));
    Ok(library)
}

fn check_abi(library: &libloading::Library) -> Result<(), String> {
    let version: libloading::Symbol<unsafe extern "C" fn() -> u32> =
        unsafe { library.get(PLUGIN_ABI_SYMBOL) }
            .map_err(|error| format!("does not export norte_plugin_abi_version: {error}"))?;
    let theirs = unsafe { version() };
    if theirs != PLUGIN_ABI_VERSION {
        return Err(format!(
            "was built against plugin interface {theirs}, host speaks {PLUGIN_ABI_VERSION}"
        ));
    }
    Ok(())
}

fn plugin_dir() -> PathBuf {
    if let Ok(value) = std::env::var("NORTE_CODE_PLUGINS")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }
    std::env::var_os("HOME").map_or_else(PathBuf::new, |home| {
        PathBuf::from(home).join(".config/norte-code/plugins")
    })
}
