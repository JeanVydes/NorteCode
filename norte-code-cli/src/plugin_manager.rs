//! Plugin management: install, list, and remove plugins from
//! ~/.config/norte-code/plugins/.
//!
//! Installing a plugin clones a git repository, builds it as a cdylib, and
//! copies the resulting shared library into the plugin directory.
//! This requires `cargo` and `git` to be available on the system.
//!
//! The clone and the build are run with their output captured rather than
//! printed: the terminal belongs to the client, and a build log is a page of
//! somebody else's warnings that the person who asked to install a plugin did
//! not come to read. What the build said when it *failed* is kept, because
//! that is the one thing worth reading.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// How many lines of a failed build are kept for the error.
///
/// Enough to say what went wrong and not enough to bury the message: the
/// lines that matter are the last ones, and everything before them is a
/// dependency's warning about a dependency.
const BUILD_ERROR_LINES: usize = 6;

/// The directory where plugin libraries live.
pub(crate) fn plugin_dir() -> PathBuf {
    if let Ok(value) = std::env::var("NORTE_CODE_PLUGINS") {
        return PathBuf::from(value);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return PathBuf::new();
    }
    PathBuf::from(home).join(".config/norte-code/plugins")
}

/// The sidecar that remembers where each installed plugin came from.
///
/// The plugin directory holds built libraries and nothing that says which
/// repository produced one, so an update would have nothing to re-clone. This
/// file records the source URL by library file stem, written on install. A
/// plugin installed before it existed has no entry and must be reinstalled
/// once with `/plugins add`.
fn sources_path() -> PathBuf {
    plugin_dir().join("sources.json")
}

/// The source URL of each installed plugin, by library file stem.
fn load_sources() -> BTreeMap<String, String> {
    std::fs::read_to_string(sources_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Writes the source map, best effort: a plugin that installed still works
/// even if the sidecar cannot be written.
fn save_sources(sources: &BTreeMap<String, String>) {
    let path = sources_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(sources) {
        let _ = std::fs::write(path, text);
    }
}

/// Records where a freshly installed library came from.
fn remember_source(stem: &str, url: &str) {
    let mut sources = load_sources();
    sources.insert(stem.to_owned(), url.to_owned());
    save_sources(&sources);
}

/// List installed plugins. Each .so file in the plugin directory is a plugin;
/// its file name (without extension) is the plugin name.
pub(crate) fn list_plugins() -> Vec<String> {
    installed_libraries()
        .into_iter()
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
        .collect()
}

/// The plugin libraries installed, in the order the directory holds them.
///
/// The paths and not the names: a name is what the file is called, and what a
/// command needs is the file — to load it, or to say what it is called.
pub(crate) fn installed_libraries() -> Vec<PathBuf> {
    let plugin_dir = plugin_dir();
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_library(&path) {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

/// Remove an installed plugin by name.
///
/// The name is either the one the plugin answers with or the one its file has:
/// the list shows both, and a person removing a plugin types the one they
/// read.
pub(crate) fn remove_plugin(name: &str) -> Result<(), String> {
    let path = library_named(name).ok_or_else(|| format!("plugin {name} not found"))?;
    std::fs::remove_file(&path).map_err(|e| format!("could not remove {name}: {e}"))?;
    if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
        let mut sources = load_sources();
        if sources.remove(stem).is_some() {
            save_sources(&sources);
        }
    }
    Ok(())
}

/// The library a plugin is known by.
///
/// The file name first — it is what the plugin is on disk — and then the name
/// the plugin answers with, which is the one a person read in the list.
pub(crate) fn library_named(name: &str) -> Option<PathBuf> {
    let libraries = installed_libraries();
    if let Some(path) = libraries
        .iter()
        .find(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(name))
    {
        return Some(path.clone());
    }
    libraries
        .into_iter()
        .find(|path| crate::plugin_loader::describe(path).is_ok_and(|plugin| plugin == name))
}

/// Install a plugin from a git URL.
///
/// Clones the repository into a temporary directory, builds it as a cdylib, and
/// copies the resulting shared library into the plugin directory. Returns the
/// name of the library that was installed.
///
/// The clone never asks for credentials: a build that stops to ask is a build
/// a person cannot get out of, and the answer to a private repository is to
/// use one that can be read.
pub(crate) fn install_plugin(url: &str) -> Result<String, String> {
    let tmp_dir = std::env::temp_dir().join(format!(
        "norte-code-plugin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    ));
    let installed = build_plugin(url, &tmp_dir);
    // The temporary directory goes either way: a failed build leaves a
    // directory with a whole `target/` in it, and nobody asked for that.
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let stem = installed?;
    remember_source(&stem, url);
    Ok(stem)
}

/// Rebuilds installed plugins from the repositories they came from.
///
/// With `only`, just the plugin whose name or library stem matches; without
/// it, every plugin with a recorded source. Each plugin is rebuilt on its own
/// and its outcome is reported, so one failure does not stop the rest.
///
/// # Errors
///
/// Returns an error when nothing can be updated: no recorded sources, or a
/// named plugin that is not installed.
#[allow(clippy::type_complexity)]
pub(crate) fn update_plugins(
    only: Option<&str>,
) -> Result<Vec<(String, Result<String, String>)>, String> {
    let sources = load_sources();
    if sources.is_empty() {
        return Err(
            "no plugin has a recorded source; reinstall one with /plugins add <url>".to_owned(),
        );
    }
    let target = match only {
        Some(name) => {
            let path = library_named(name).ok_or_else(|| format!("plugin {name} not found"))?;
            Some(
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_owned)
                    .ok_or_else(|| format!("plugin {name} not found"))?,
            )
        }
        None => None,
    };
    let mut results = Vec::new();
    for (stem, url) in sources {
        if target.as_deref().is_some_and(|target| target != stem) {
            continue;
        }
        let result = install_plugin(&url);
        // A crate that renamed its library leaves the old stem pointing at a
        // file that is gone; the install recorded the new one.
        if let Ok(new_stem) = &result
            && *new_stem != stem
        {
            let mut current = load_sources();
            if current.remove(&stem).is_some() {
                save_sources(&current);
            }
        }
        results.push((stem, result));
    }
    if results.is_empty() {
        return Err(
            "the plugin has no recorded source; reinstall it with /plugins add <url>".to_owned(),
        );
    }
    Ok(results)
}

/// Clone, build and install, into the plugin directory.
fn build_plugin(url: &str, tmp_dir: &Path) -> Result<String, String> {
    if Command::new("git").arg("--version").output().is_err() {
        return Err("git is not installed".to_owned());
    }
    if Command::new("cargo").arg("--version").output().is_err() {
        return Err("cargo is not installed".to_owned());
    }

    // Clone the repository.
    let clone = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(tmp_dir)
        // A clone that asks for a password is a clone that waits forever on a
        // terminal that is busy drawing something else.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/true")
        .env("SSH_ASKPASS", "/bin/true")
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !clone.status.success() {
        return Err(format!(
            "git clone failed: {}",
            tail(&clone.stderr, BUILD_ERROR_LINES)
        ));
    }

    // Build it as a cdylib. The JSON message format is what says which file
    // the library is: the directory holds the dependencies' libraries too, and
    // picking one of those would install something that is not a plugin.
    let build = Command::new("cargo")
        .args(["build", "--release", "--message-format=json"])
        .current_dir(tmp_dir)
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if !build.status.success() {
        return Err(format!(
            "cargo build failed: {}",
            tail(&build.stderr, BUILD_ERROR_LINES)
        ));
    }

    let library = cdylib(&build.stdout).ok_or_else(|| {
        "the build produced no shared library; a plugin has to be a cdylib".to_owned()
    })?;

    let plugin_dir = plugin_dir();
    std::fs::create_dir_all(&plugin_dir)
        .map_err(|e| format!("could not create {}: {e}", plugin_dir.display()))?;

    let file_name = library
        .file_name()
        .ok_or_else(|| "the built library has no name".to_owned())?;
    let dest = plugin_dir.join(file_name);
    // Write beside the destination and rename over it. Copying in place
    // truncates the file the process may still have mapped — the update path
    // resolves a plugin by name, which loads it to read its name — and a
    // shared library rewritten under its own mapping crashes on unload. A
    // rename swaps the directory entry and leaves the old mapping intact.
    let staging = plugin_dir.join(format!(".{}.new", file_name.to_string_lossy()));
    std::fs::copy(&library, &staging)
        .map_err(|e| format!("could not copy the plugin library: {e}"))?;
    std::fs::rename(&staging, &dest)
        .map_err(|e| format!("could not install the plugin library: {e}"))?;

    Ok(dest
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(|| "plugin".to_owned(), std::borrow::ToOwned::to_owned))
}

/// Whether a path is a shared library this platform loads.
fn is_library(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext == "so" || ext == "dll" || ext == "dylib")
}

/// The shared library cargo built, read out of its JSON messages.
///
/// Cargo says what it produced and where; the directory also holds the
/// libraries of every dependency, and one of those is not a plugin.
fn cdylib(messages: &[u8]) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(messages);
    for line in text.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let is_cdylib = message
            .get("target")
            .and_then(|target| target.get("kind"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("cdylib")));
        if !is_cdylib {
            continue;
        }
        let Some(filenames) = message
            .get("filenames")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for filename in filenames.iter().filter_map(serde_json::Value::as_str) {
            let path = PathBuf::from(filename);
            if is_library(&path) {
                return Some(path);
            }
        }
    }
    None
}

/// The last lines of what a command printed, for the error that says why it
/// failed.
///
/// The lines that matter are the last ones: everything before them is a
/// dependency's warning about a dependency.
fn tail(output: &[u8], lines: usize) -> String {
    let text = String::from_utf8_lossy(output);
    let mut kept: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    if kept.len() > lines {
        kept = kept.split_off(kept.len() - lines);
    }
    if kept.is_empty() {
        "no output".to_owned()
    } else {
        kept.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_dir_defaults_to_home_config() {
        let dir = plugin_dir();
        assert!(dir.to_string_lossy().contains(".config/norte-code/plugins"));
    }

    #[test]
    fn list_plugins_empty_when_dir_missing() {
        // Use a bogus HOME so the directory doesn't exist.
        unsafe { std::env::set_var("HOME", "/nonexistent") };
        let names = list_plugins();
        assert_eq!(names, Vec::<String>::new());
    }

    #[test]
    fn the_cdylib_is_read_out_of_the_cargo_messages() {
        // The directory holds every dependency's library too, and one of
        // those is not a plugin.
        let messages = br#"{"reason":"compiler-artifact","target":{"kind":["cdylib"]},"filenames":["/tmp/target/release/libplugin.so"]}
{"reason":"compiler-artifact","target":{"kind":["lib"]},"filenames":["/tmp/target/release/libplugin.rlib"]}
"#;
        assert_eq!(
            cdylib(messages),
            Some(PathBuf::from("/tmp/target/release/libplugin.so"))
        );
    }

    #[test]
    fn a_build_without_a_cdylib_has_no_library() {
        let messages = br#"{"reason":"compiler-artifact","target":{"kind":["lib"]},"filenames":["/tmp/target/release/libplugin.rlib"]}
"#;
        assert_eq!(cdylib(messages), None);
    }

    #[test]
    fn the_error_keeps_the_last_lines_and_not_the_whole_log() {
        let output = b"one\ntwo\nthree\nfour\nfive\n";
        assert_eq!(tail(output, 2), "four\nfive");
    }

    #[test]
    fn an_error_without_output_still_says_something() {
        assert_eq!(tail(b"", 4), "no output");
    }
}
