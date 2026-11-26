//! Dynamic plugin loading for the Norte Code server.
//!
//! This is the only crate allowed to cross the dynamic-library boundary. The
//! RAEP server remains `forbid(unsafe_code)` and receives an ordinary
//! [`Plugins`] registry from this crate.

#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod commands;
pub mod plugins;

pub use commands::{
    CommandArgument, CommandExecutionError, CommandName, CommandOutput, CommandRegistrationError,
    CommandRegistry, CommandSpec, RegisteredCommand,
};
pub use norte_code_plugin_api::{HOST_EFFECTS_KEY, HostEffects, UserMessageDraft};
pub use plugins::{PLUGIN_ABI_SYMBOL, PLUGIN_ABI_VERSION, Plugin, PluginContext, Plugins};

static LIBRARIES: Mutex<Vec<(PathBuf, Arc<libloading::Library>)>> = Mutex::new(Vec::new());

/// The plugin files that loaded in this process, by file name.
///
/// The host reports what is on disk and what is running separately: a library
/// that failed to load is on disk and not here, and a plugin that answers a
/// different name than its file still counts as loaded under that file.
static LOADED_FILES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The file names of the plugin libraries that loaded in this process.
#[must_use]
pub fn loaded_files() -> Vec<String> {
    LOADED_FILES
        .lock()
        .map(|files| files.clone())
        .unwrap_or_default()
}

/// Whether a plugin file loaded in this process.
#[must_use]
pub fn is_loaded(file_name: &str) -> bool {
    LOADED_FILES
        .lock()
        .is_ok_and(|files| files.iter().any(|known| known == file_name))
}

/// The plugin files that failed to load in this process, with the reason.
static FAILED_FILES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// The reason a plugin file failed to load, when it did.
#[must_use]
pub fn failure_reason(file_name: &str) -> Option<String> {
    FAILED_FILES.lock().ok().and_then(|files| {
        files
            .iter()
            .find(|(known, _)| known == file_name)
            .map(|(_, reason)| reason.clone())
    })
}

/// Records a plugin file that failed to load.
fn remember_failure(path: &Path, reason: &str) {
    let Some(file) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    if let Ok(mut files) = FAILED_FILES.lock() {
        files.retain(|(known, _)| known != file);
        files.push((file.to_owned(), reason.to_owned()));
    }
}

/// Records a plugin file that loaded, once.
fn remember_loaded(path: &Path) {
    let Some(file) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    if let Ok(mut files) = LOADED_FILES.lock()
        && !files.iter().any(|known| known == file)
    {
        files.push(file.to_owned());
    }
}

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
            Ok(plugin) => {
                remember_loaded(&path);
                plugins.add(plugin);
            }
            Err(error) => {
                remember_failure(&path, &error);
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
    validate_abi(theirs)
}

fn validate_abi(plugin_version: u32) -> Result<(), String> {
    if plugin_version == PLUGIN_ABI_VERSION {
        Ok(())
    } else {
        Err(format!(
            "was built against plugin interface {plugin_version}, host speaks {PLUGIN_ABI_VERSION}"
        ))
    }
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

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
