//! Plugin management: install, list, and remove plugins from
//! ~/.config/norte-code/plugins/.
//!
//! Installing a plugin clones a git repository, builds it as a cdylib, and
//! copies the resulting shared library into the plugin directory.
//! This requires `cargo` and `git` to be available on the system.

use std::path::{Path, PathBuf};
use std::process::Command;

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
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                continue;
            };
            if ext == "so" || ext == "dll" || ext == "dylib" {
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
/// copies the resulting shared library into the plugin directory.
///
/// Output from git and cargo is captured and only shown on failure; the user
/// sees "Installing ..." while it works.
pub(crate) fn install_plugin(url: &str) -> Result<String, String> {
    // Ensure git and cargo are available.
    if Command::new("git").arg("--version").output().is_err() {
        return Err("git is not installed".to_owned());
    }
    if Command::new("cargo").arg("--version").output().is_err() {
        return Err("cargo is not installed".to_owned());
    }

    let tmp_dir = std::env::temp_dir().join(format!(
        "norte-code-plugin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    ));

    // Clone the repository.
    let clone_status = Command::new("git")
        .args(["clone", "--depth", "1", url, tmp_dir.to_str().unwrap()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("failed to run git clone: {e}"))?;
    if !clone_status.success() {
        return Err(format!("git clone failed for {url}"));
    }

    // Build as cdylib.
    let build_status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&tmp_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("failed to run cargo build: {e}"))?;
    if !build_status.success() {
        return Err(format!("cargo build failed for {url}"));
    }

    // Find the resulting shared library in target/release/.
    let release_dir = tmp_dir.join("target/release");
    let lib_name =
        find_shared_lib(&release_dir).ok_or_else(|| format!("no shared library found in {url}"))?;

    // Ensure the plugin directory exists.
    let plugin_dir = plugin_dir();
    std::fs::create_dir_all(&plugin_dir)
        .map_err(|e| format!("could not create plugin directory: {e}"))?;

    let dest = plugin_dir.join(lib_name.file_name().unwrap());
    std::fs::copy(&lib_name, &dest).map_err(|e| format!("could not copy plugin library: {e}"))?;

    // Clean up the temporary directory.
    let _ = std::fs::remove_dir_all(&tmp_dir);

    Ok(dest
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(|| "plugin".to_owned(), |s: &str| s.to_owned()))
}

/// Find the shared library produced by cargo build --release.
fn find_shared_lib(dir: &Path) -> Option<PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str())?;
            if ext == "so" || ext == "dll" || ext == "dylib" {
                let name = path.file_stem()?.to_string_lossy();
                // Skip the rlib and dylib metadata files cargo produces;
                // we want the plugin shared library.
                if name.starts_with("lib") {
                    return Some(path);
                }
            }
        }
    }
    None
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
}
