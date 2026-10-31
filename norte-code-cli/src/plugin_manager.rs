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

/// List installed plugins. Each .so file in the plugin directory is a plugin;
/// its file name (without extension) is the plugin name.
pub(crate) fn list_plugins() -> Vec<String> {
    let plugin_dir = plugin_dir();
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_library(&path) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    names.push(stem.to_owned());
                }
            }
        }
    }
    names.sort();
    names
}

/// Remove an installed plugin by name.
pub(crate) fn remove_plugin(name: &str) -> Result<(), String> {
    let plugin_dir = plugin_dir();
    for ext in ["so", "dll", "dylib"] {
        let path = plugin_dir.join(format!("{name}.{ext}"));
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("could not remove {name}: {e}"))?;
            return Ok(());
        }
    }
    Err(format!("plugin {name} not found"))
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
    installed
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
    std::fs::copy(&library, &dest)
        .map_err(|e| format!("could not copy the plugin library: {e}"))?;

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
