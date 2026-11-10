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
/// file records the source URL and selected workspace package by library file
/// stem. A plugin installed before it existed has no entry and must be
/// reinstalled once with `/plugins add`.
fn sources_path() -> PathBuf {
    plugin_dir().join("sources.json")
}

/// Where an installed plugin came from.
#[derive(Clone, Debug)]
struct PluginSource {
    url: String,
    package: Option<String>,
}

/// The source URL and workspace package of each installed plugin.
fn load_sources() -> BTreeMap<String, PluginSource> {
    let Ok(text) = std::fs::read_to_string(sources_path()) else {
        return BTreeMap::new();
    };
    parse_sources(&text).unwrap_or_default()
}

fn parse_sources(text: &str) -> Option<BTreeMap<String, PluginSource>> {
    let stored: serde_json::Value = serde_json::from_str(text).ok()?;
    let stored = stored.as_object()?;
    stored
        .iter()
        .map(|(stem, source)| {
            let source = if let Some(url) = source.as_str() {
                PluginSource {
                    url: url.to_owned(),
                    package: None,
                }
            } else {
                PluginSource {
                    url: source.get("url")?.as_str()?.to_owned(),
                    package: source
                        .get("package")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                }
            };
            Some((stem.clone(), source))
        })
        .collect()
}

/// Writes the source map, best effort: a plugin that installed still works
/// even if the sidecar cannot be written.
fn save_sources(sources: &BTreeMap<String, PluginSource>) {
    let path = sources_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let sources: BTreeMap<_, _> = sources
        .iter()
        .map(|(stem, source)| {
            let mut value = serde_json::Map::new();
            value.insert("url".to_owned(), serde_json::json!(source.url));
            if let Some(package) = &source.package {
                value.insert("package".to_owned(), serde_json::json!(package));
            }
            (stem.clone(), serde_json::Value::Object(value))
        })
        .collect();
    if let Ok(text) = serde_json::to_string_pretty(&sources) {
        let _ = std::fs::write(path, text);
    }
}

/// Records where a freshly installed library came from.
fn remember_source(stem: &str, url: &str, package: &str) {
    let mut sources = load_sources();
    sources.insert(
        stem.to_owned(),
        PluginSource {
            url: url.to_owned(),
            package: Some(package.to_owned()),
        },
    );
    save_sources(&sources);
}

/// List installed plugins. Each .so file in the plugin directory is a plugin;
/// its file name (without extension) is the plugin name.
#[cfg(test)]
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
    None
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
///
/// `on_progress` is called with the step number, the total step count and a
/// short label, so a caller can show where the install stands.
pub(crate) fn install_plugin(
    url: &str,
    package: Option<&str>,
    on_progress: &dyn Fn(usize, usize, &str),
) -> Result<String, String> {
    let tmp_dir = std::env::temp_dir().join(format!(
        "norte-code-plugin-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let installed = build_plugin_inner(url, package, &tmp_dir, on_progress);
    // The temporary directory goes either way: a failed build leaves a
    // directory with a whole `target/` in it, and nobody asked for that.
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let (stem, package) = installed?;
    remember_source(&stem, url, &package);
    Ok(stem)
}

/// Installs every plugin package in a workspace, one after another.
///
/// Each package is built and installed on its own, and its outcome is
/// reported, so one failure does not stop the rest. Returns the installed
/// library stems, or an error when the workspace holds no plugin package.
///
/// # Errors
///
/// Returns an error when the workspace has no plugin package, or when every
/// package failed to install.
pub(crate) fn install_all_plugins(
    url: &str,
    on_progress: &dyn Fn(usize, usize, &str),
) -> Result<Vec<String>, String> {
    let tmp_dir = std::env::temp_dir().join(format!(
        "norte-code-plugin-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let installed = build_all_plugins(url, &tmp_dir, on_progress);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let results = installed?;
    let mut stems = Vec::new();
    let mut first_error = None;
    for (package, result) in results {
        match result {
            Ok(stem) => {
                remember_source(&stem, url, &package);
                stems.push(stem);
            }
            Err(error) => {
                first_error.get_or_insert(format!("{package}: {error}"));
            }
        }
    }
    if stems.is_empty() {
        return Err(
            first_error.unwrap_or_else(|| "no plugin package could be installed".to_owned())
        );
    }
    Ok(stems)
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
    for (stem, source) in sources {
        if target.as_deref().is_some_and(|target| target != stem) {
            continue;
        }
        let result = install_plugin(&source.url, source.package.as_deref(), &|_, _, _| {});
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

/// One workspace package with one plugin cdylib target.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PluginPackage {
    id: String,
    name: String,
    targets: Vec<String>,
}

/// Clone, inspect, build and install every plugin package in a workspace.
///
/// The repository is cloned and inspected once; each package is then built
/// and installed on its own, so one failure does not stop the rest.
/// `on_progress` receives the package index, the package count and the
/// package name, once per package.
///
/// # Errors
///
/// Returns an error when the workspace has no plugin package.
#[allow(clippy::type_complexity)]
fn build_all_plugins(
    url: &str,
    tmp_dir: &Path,
    on_progress: &dyn Fn(usize, usize, &str),
) -> Result<Vec<(String, Result<String, String>)>, String> {
    if Command::new("git").arg("--version").output().is_err() {
        return Err("git is not installed".to_owned());
    }
    if Command::new("cargo").arg("--version").output().is_err() {
        return Err("cargo is not installed".to_owned());
    }
    let clone = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(tmp_dir)
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
    let metadata = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(tmp_dir)
        .output()
        .map_err(|error| format!("could not inspect the workspace with cargo: {error}"))?;
    if !metadata.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            tail(&metadata.stderr, BUILD_ERROR_LINES)
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout)
        .map_err(|error| format!("cargo metadata returned invalid JSON: {error}"))?;
    let packages = plugin_packages(&metadata)?;
    if packages.is_empty() {
        return Err(
            "the repository has no cdylib plugin package; a plugin must expose a cdylib target"
                .to_owned(),
        );
    }
    let total = packages.len();
    let mut results = Vec::new();
    for (index, package) in packages.iter().enumerate() {
        on_progress(index + 1, total, &package.name);
        let result =
            build_and_install_package(tmp_dir, package).map(|stem| (stem, package.name.clone()));
        results.push((package.name.clone(), result.map(|(stem, _)| stem)));
    }
    Ok(results)
}

/// Builds one already-inspected package and installs its library.
fn build_and_install_package(tmp_dir: &Path, package: &PluginPackage) -> Result<String, String> {
    let build = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--message-format=json",
            "--package",
            &package.name,
        ])
        .current_dir(tmp_dir)
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if !build.status.success() {
        return Err(format!(
            "cargo build failed: {}",
            tail(&build.stderr, BUILD_ERROR_LINES)
        ));
    }
    let library = cdylib(&build.stdout, &package.id).ok_or_else(|| {
        format!(
            "package '{}' built no cdylib plugin; check its workspace target",
            package.name
        )
    })?;
    install_library(&library)
}

/// Copies a built library into the plugin directory, atomically.
///
/// Write beside the destination and rename over it. Copying in place
/// truncates the file the process may still have mapped — the update path
/// resolves a plugin by name, which loads it to read its name — and a
/// shared library rewritten under its own mapping crashes on unload. A
/// rename swaps the directory entry and leaves the old mapping intact.
fn install_library(library: &Path) -> Result<String, String> {
    let plugin_dir = plugin_dir();
    std::fs::create_dir_all(&plugin_dir)
        .map_err(|e| format!("could not create {}: {e}", plugin_dir.display()))?;
    let file_name = library
        .file_name()
        .ok_or_else(|| "the built library has no name".to_owned())?;
    let dest = plugin_dir.join(file_name);
    let staging = plugin_dir.join(format!(".{}.new", file_name.to_string_lossy()));
    std::fs::copy(library, &staging)
        .map_err(|e| format!("could not copy the plugin library: {e}"))?;
    std::fs::rename(&staging, &dest)
        .map_err(|e| format!("could not install the plugin library: {e}"))?;
    Ok(dest
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(|| "plugin".to_owned(), std::borrow::ToOwned::to_owned))
}

/// Clone, inspect, build and install one workspace plugin.
///
/// `on_progress` receives the step number, the total step count and a short
/// label, so a caller can show where the install stands.
///
/// # Errors
///
/// Returns an error when git or cargo is missing, the clone or build fails,
/// or the package cannot be selected or installed.
fn build_plugin_inner(
    url: &str,
    selected_package: Option<&str>,
    tmp_dir: &Path,
    on_progress: &dyn Fn(usize, usize, &str),
) -> Result<(String, String), String> {
    if Command::new("git").arg("--version").output().is_err() {
        return Err("git is not installed".to_owned());
    }
    if Command::new("cargo").arg("--version").output().is_err() {
        return Err("cargo is not installed".to_owned());
    }

    // Clone the repository.
    on_progress(1, 4, "cloning");
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

    let metadata = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(tmp_dir)
        .output()
        .map_err(|error| format!("could not inspect the workspace with cargo: {error}"))?;
    if !metadata.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            tail(&metadata.stderr, BUILD_ERROR_LINES)
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout)
        .map_err(|error| format!("cargo metadata returned invalid JSON: {error}"))?;
    on_progress(2, 4, "inspecting the workspace");
    let packages = plugin_packages(&metadata)?;
    let package = select_plugin_package(&packages, selected_package)?;
    on_progress(3, 4, &format!("building {}", package.name));
    let stem = build_and_install_package(tmp_dir, &package)?;
    on_progress(4, 4, "installing");
    Ok((stem, package.name))
}

/// Reads the workspace members that declare a `cdylib` target.
fn plugin_packages(metadata: &serde_json::Value) -> Result<Vec<PluginPackage>, String> {
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "cargo metadata did not describe workspace packages".to_owned())?;
    let mut plugins = Vec::new();
    for package in packages {
        let Some(id) = package.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(name) = package.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let targets = package
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|target| {
                target
                    .get("crate_types")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|crate_type| crate_type.as_str() == Some("cdylib"))
            })
            .filter_map(|target| target.get("name").and_then(serde_json::Value::as_str))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !targets.is_empty() {
            plugins.push(PluginPackage {
                id: id.to_owned(),
                name: name.to_owned(),
                targets,
            });
        }
    }
    plugins.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(plugins)
}

/// Resolves the selected package, refusing to guess in a multi-plugin workspace.
fn select_plugin_package(
    packages: &[PluginPackage],
    selected: Option<&str>,
) -> Result<PluginPackage, String> {
    let Some(selected) = selected else {
        return match packages {
            [package] if package.targets.len() == 1 => Ok(package.clone()),
            [package] => Err(format!(
                "package '{}' has multiple cdylib targets ({}); split it into one plugin per package",
                package.name,
                package.targets.join(", ")
            )),
            [] => Err(
                "the repository has no cdylib plugin package; a plugin must expose a cdylib target"
                    .to_owned(),
            ),
            _ => Err(format!(
                "workspace has multiple plugin packages ({}); select one with --package <name> or install them all with --all",
                plugin_package_names(packages)
            )),
        };
    };
    let matching: Vec<_> = packages
        .iter()
        .filter(|package| package.name == selected)
        .collect();
    match matching.as_slice() {
        [package] if package.targets.len() == 1 => Ok((**package).clone()),
        [package] => Err(format!(
            "package '{}' has multiple cdylib targets ({}); split it into one plugin per package",
            package.name,
            package.targets.join(", ")
        )),
        [] => Err(format!(
            "no plugin package named '{selected}' was found; workspace plugins: {}",
            plugin_package_names(packages)
        )),
        _ => Err(format!(
            "package name '{selected}' is ambiguous in the workspace"
        )),
    }
}

fn plugin_package_names(packages: &[PluginPackage]) -> String {
    if packages.is_empty() {
        "(none found)".to_owned()
    } else {
        packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Whether a path is a shared library this platform loads.
fn is_library(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext == "so" || ext == "dll" || ext == "dylib")
}

/// The suffix that marks a library as disabled.
///
/// Disabling a plugin is renaming its file: the loader only opens `.so`/`.dll`
/// /`.dylib`, so a `.disabled` suffix is enough for the host to pass it by,
/// and the bytes stay where they are.
const DISABLED_SUFFIX: &str = ".disabled";

/// Whether a path is a disabled plugin library.
fn is_disabled_library(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(DISABLED_SUFFIX))
        .is_some_and(|stem| is_library(Path::new(stem)))
}

/// Every installed plugin file, enabled and disabled, in path order.
pub(crate) fn installed_entries() -> Vec<PathBuf> {
    let plugin_dir = plugin_dir();
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_library(&path) || is_disabled_library(&path) {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

pub(crate) fn plugin_info() -> Vec<norte_code_wire::PluginInfo> {
    installed_entries()
        .into_iter()
        .filter_map(|path| {
            let file = path.file_name()?.to_str()?.to_owned();
            let disabled = file.ends_with(".disabled");
            let name = path
                .file_stem()?
                .to_str()?
                .trim_end_matches(".disabled")
                .to_owned();
            Some(norte_code_wire::PluginInfo {
                name,
                enabled: !disabled,
                loaded: false,
            })
        })
        .collect()
}

/// Enables or disables a plugin by renaming its library.
///
/// The name is either the one the plugin answers with or the one its file has,
/// enabled or disabled, so a person types what the list shows.
///
/// # Errors
///
/// Returns an error when the plugin is not installed or the rename fails.
pub(crate) fn set_plugin_enabled(name: &str, enabled: bool) -> Result<(), String> {
    let path = installed_entries()
        .into_iter()
        .find(|path| {
            let stem = path.file_stem().and_then(|s| s.to_str());
            stem == Some(name)
        })
        .ok_or_else(|| format!("plugin {name} not found"))?;
    let currently_enabled = is_library(&path);
    if currently_enabled == enabled {
        return Ok(());
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("plugin {name} has no file name"))?
        .to_owned();
    let renamed = if enabled {
        let stem = file_name
            .strip_suffix(DISABLED_SUFFIX)
            .ok_or_else(|| format!("plugin {name} is not disabled"))?;
        path.with_file_name(stem)
    } else {
        path.with_file_name(format!("{file_name}{DISABLED_SUFFIX}"))
    };
    std::fs::rename(&path, &renamed).map_err(|e| {
        format!(
            "could not {} {name}: {e}",
            if enabled { "enable" } else { "disable" }
        )
    })
}

/// The shared library cargo built, read out of its JSON messages.
///
/// Cargo says what it produced and where; the directory also holds the
/// libraries of every dependency, and one of those is not a plugin.
fn cdylib(messages: &[u8], package_id: &str) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(messages);
    for line in text.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        if message
            .get("package_id")
            .and_then(serde_json::Value::as_str)
            != Some(package_id)
        {
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
#[path = "plugin_manager_tests.rs"]
mod tests;
