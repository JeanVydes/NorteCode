//! Where configuration lives and how it is edited in place.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::loading::{invalid, read_value};
use super::{ConfigError, ServiceConfig};

/// The user service path: `$XDG_CONFIG_HOME/norte-code/service.json`, or the
/// platform equivalent.
#[must_use]
pub fn user_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("norte-code").join("service.json"))
}

/// Default server configuration shipped with this build.
const DEFAULT_SERVICE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../default/service.json"
));

/// Default client configuration shipped with this build.
const DEFAULT_CLI_JSON: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../default/cli.json"));

/// Creates the user service configuration when it does not exist.
///
/// Existing files are never overwritten. `None` means that no user config
/// directory could be determined and the caller must keep using in-memory
/// defaults.
///
/// # Errors
///
/// Returns [`ConfigError`] when the default cannot be written.
pub fn ensure_user_service_config() -> Result<Option<PathBuf>, ConfigError> {
    ensure_default_config_file(user_config_path(), DEFAULT_SERVICE_JSON)
}

/// The client configuration path: `$XDG_CONFIG_HOME/norte-code/cli.json`, or the
/// platform equivalent.
#[must_use]
pub fn user_cli_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("norte-code").join("cli.json"))
}

/// Creates the user CLI configuration when it does not exist.
///
/// Existing files are never overwritten.
///
/// # Errors
///
/// Returns [`ConfigError`] when the default cannot be written.
pub fn ensure_user_cli_config() -> Result<Option<PathBuf>, ConfigError> {
    ensure_default_config_file(user_cli_path(), DEFAULT_CLI_JSON)
}

fn ensure_default_config_file(
    path: Option<PathBuf>,
    contents: &str,
) -> Result<Option<PathBuf>, ConfigError> {
    let Some(path) = path else {
        return Ok(None);
    };
    if path.exists() {
        return Ok(Some(path));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: path.clone(),
            source,
        })?;
    }
    atomic_write(&path, contents.as_bytes())?;
    Ok(Some(path))
}

/// The per-project overlay path under `root`.
#[must_use]
pub fn project_config_path(root: &Path) -> PathBuf {
    root.join(".norte-code").join("service.json")
}

/// The per-project client overlay path under `root`.
#[must_use]
pub fn project_cli_path(root: &Path) -> PathBuf {
    root.join(".norte-code").join("cli.json")
}

/// The file a change to the user's configuration is written to.
///
/// The path the process was started with wins, so `--config FILE` is the file
/// that is edited rather than a second one the user never looks at again; the
/// platform user path is the fallback. `None` means there is nowhere to write,
/// which the caller reports instead of guessing.
#[must_use]
pub fn resolve_user_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
    explicit.map(Path::to_path_buf).or_else(user_config_path)
}

/// Sets `default_provider` and `default_model` in a config file, preserving the
/// rest of it.
///
/// The two are written together and kept apart, because a model name is a string
/// that may carry slashes and a provider is its own key: the pair is what
/// `run` and the pickers resolve, and neither is ever split to find the other.
///
/// # Errors
///
/// Returns [`ConfigError`] when either name is empty, or when the file cannot be
/// read or written.
pub fn update_user_default(path: &Path, provider: &str, model: &str) -> Result<(), ConfigError> {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() {
        return Err(invalid("default_provider", "the provider name is empty"));
    }
    if model.is_empty() {
        return Err(invalid("default_model", "the model name is empty"));
    }
    let mut service = if path.exists() {
        ServiceConfig::load(Some(path), None)?
    } else {
        ServiceConfig::default()
    };
    let default_agent = service.default_agent.clone();
    let agent = service
        .agents
        .iter_mut()
        .find(|agent| agent.name == default_agent)
        .ok_or_else(|| invalid("default_agent", "the default agent does not exist"))?;
    agent.default_provider = Some(provider.to_owned());
    agent.default_model = Some(model.to_owned());
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let mut body = serde_json::to_string_pretty(&service).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    body.push('\n');
    atomic_write(path, body.as_bytes())
}

/// Writes a config file atomically, preserving an existing symlink and mode.
fn atomic_write(path: &Path, body: &[u8]) -> Result<(), ConfigError> {
    atomic_write_with(path, body, false)
}

/// Writes a file atomically, preserving an existing symlink and mode.
///
/// A fixed temporary name would race with another Norte Code process, and a
/// truncating write would lose the file if the process died halfway. Write a
/// unique sibling with `create_new`, then rename it over the target.
///
/// `force_private` makes the result mode 0600 even when the target already
/// existed with broader permissions. The secrets file uses it; a config file
/// keeps the mode a person gave it.
#[allow(clippy::too_many_lines)]
pub(crate) fn atomic_write_with(
    path: &Path,
    body: &[u8],
    force_private: bool,
) -> Result<(), ConfigError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[cfg(not(unix))]
    let _ = force_private;
    let target = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::canonicalize(path).map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source,
            })?
        }
        Ok(_) => path.to_path_buf(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(source) => {
            return Err(ConfigError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let existing = match fs::metadata(&target) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(ConfigError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if existing
        .as_ref()
        .is_some_and(|metadata| !metadata.is_file())
    {
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        });
    }
    let permissions = existing.map(|metadata| metadata.permissions());
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for _ in 0..16 {
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("service.json");
        let tmp = parent.join(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = match options.open(&tmp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(ConfigError::Write {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let result = (|| -> std::io::Result<()> {
            let mut file = file;
            file.write_all(body)?;
            file.sync_all()?;
            if force_private {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o600))?;
                }
            } else if let Some(permissions) = &permissions {
                file.set_permissions(permissions.clone())?;
            }
            drop(file);
            fs::rename(&tmp, &target)
        })();
        if let Err(source) = result {
            let _ = fs::remove_file(&tmp);
            return Err(ConfigError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
        return Ok(());
    }
    Err(ConfigError::Write {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a unique temporary file",
        ),
    })
}

/// Removes a tool from the config's approval lists.
///
/// It is what "ask again" means, and it has to touch the file for the same
/// reason [`update_user_approval`] does: a rule forgotten in memory and kept on
/// disk comes back at the next restart, which is a rule nobody can get rid of.
///
/// A tool that is in neither list is not an error: forgetting a rule that was
/// never written is what a person asks for when they press the key twice, and
/// refusing would be the panel arguing with the press.
///
/// # Errors
///
/// Returns a [`ConfigError`] when the tool is unnamed, when the file is not a
/// JSON object, or when it cannot be read or written.
pub fn forget_user_approval(path: &Path, tool: &str) -> Result<(), ConfigError> {
    let tool = tool.trim();
    if tool.is_empty() {
        return Err(invalid("approvals", "the tool name is empty"));
    }
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_value(path)?;
    let Some(root) = root.as_object_mut() else {
        return Err(invalid(
            "approvals",
            "the config root must be a JSON object",
        ));
    };
    let Some(approvals) = root.get_mut("approvals").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let mut changed = false;
    for list in ["allow", "deny"] {
        if let Some(entries) = approvals.get_mut(list).and_then(Value::as_array_mut) {
            let before = entries.len();
            entries.retain(|entry| entry.as_str() != Some(tool));
            changed |= entries.len() != before;
        }
    }
    if !changed {
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let mut body = serde_json::to_string_pretty(&root).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    body.push('\n');
    atomic_write(path, body.as_bytes())
}

/// The `mcpServers` map of a config file, as a mutable JSON object.
///
/// It is created when the file does not have it, and refused when it has one of
/// the wrong shape: a `mcpServers` that is not an object is a file a person
/// edited by hand and got wrong, and writing over it would lose what they meant.
fn mcp_servers_mut(root: &mut Value) -> Result<&mut serde_json::Map<String, Value>, ConfigError> {
    let root = root
        .as_object_mut()
        .ok_or_else(|| invalid("mcpServers", "the config root must be a JSON object"))?;
    let entry = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    entry
        .as_object_mut()
        .ok_or_else(|| invalid("mcpServers", "must be a JSON object"))
}

/// Reads the file, applies `edit`, and writes it back, preserving the rest.
fn edit_config(
    path: &Path,
    edit: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    let mut root = if path.exists() {
        read_value(path)?
    } else {
        Value::Object(serde_json::Map::new())
    };
    edit(&mut root)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let mut body = serde_json::to_string_pretty(&root).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    body.push('\n');
    atomic_write(path, body.as_bytes())
}

/// Adds one MCP server to the config file at `mcpServers.<name>`.
///
/// The entry is written as a program to run — `npx`, a path, anything the
/// platform can execute — with its arguments and an `enabled` flag. An existing
/// entry of the same name is replaced, because adding a server that is already
/// named is what a person does to change it.
///
/// # Errors
///
/// Returns [`ConfigError`] when the name or the command is empty, or when the
/// file cannot be read or written.
pub fn add_user_mcp_server(
    path: &Path,
    name: &str,
    command: &str,
    args: &[String],
) -> Result<(), ConfigError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("mcpServers", "the server name is empty"));
    }
    if command.trim().is_empty() {
        return Err(invalid("mcpServers", "the server command is empty"));
    }
    edit_config(path, |root| {
        let servers = mcp_servers_mut(root)?;
        servers.insert(
            name.to_owned(),
            serde_json::json!({
                "command": command.trim(),
                "args": args,
                "enabled": true,
            }),
        );
        Ok(())
    })
}

/// Turns one MCP server on or off in the config file.
///
/// It sets `mcpServers.<name>.enabled`, which is the same flag the config
/// carries and the one that keeps an entry without starting it.
///
/// # Errors
///
/// Returns [`ConfigError`] when the name is empty, no server has it, or the file
/// cannot be read or written.
pub fn set_user_mcp_enabled(path: &Path, name: &str, enabled: bool) -> Result<(), ConfigError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("mcpServers", "the server name is empty"));
    }
    edit_config(path, |root| {
        let servers = mcp_servers_mut(root)?;
        let Some(entry) = servers.get_mut(name) else {
            return Err(invalid(
                "mcpServers",
                format!("no server named `{name}` in the config"),
            ));
        };
        let entry = entry
            .as_object_mut()
            .ok_or_else(|| invalid("mcpServers", format!("`{name}` is not a JSON object")))?;
        entry.insert("enabled".to_owned(), Value::Bool(enabled));
        Ok(())
    })
}

/// Removes one MCP server from the config file.
///
/// A name that is not there is not an error: removing one twice is what a person
/// does when they are not sure it worked, and refusing the second press is the
/// file arguing with them.
///
/// # Errors
///
/// Returns [`ConfigError`] when the name is empty, or when the file cannot be
/// read or written.
pub fn remove_user_mcp_server(path: &Path, name: &str) -> Result<(), ConfigError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("mcpServers", "the server name is empty"));
    }
    edit_config(path, |root| {
        let servers = mcp_servers_mut(root)?;
        servers.remove(name);
        Ok(())
    })
}

/// Adds one global approval rule to the user config, preserving the rest of
/// the file.
///
/// A tool moves between `approvals.allow` and `approvals.deny`; the opposite
/// list is cleared for that tool so the file never says both. Unknown keys and
/// the existing formatting-independent JSON content are kept.
///
/// # Errors
///
/// Returns [`ConfigError`] when the file cannot be read or written, is not a
/// JSON object, or has an `approvals` field of the wrong shape.
pub fn update_user_approval(path: &Path, tool: &str, allow: bool) -> Result<(), ConfigError> {
    let tool = tool.trim();
    if tool.is_empty() {
        return Err(invalid("approvals", "the tool name is empty"));
    }
    let mut root = if path.exists() {
        read_value(path)?
    } else {
        Value::Object(serde_json::Map::new())
    };
    let root = root
        .as_object_mut()
        .ok_or_else(|| invalid("approvals", "the config root must be a JSON object"))?;
    let approvals = root
        .entry("approvals")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let approvals = approvals
        .as_object_mut()
        .ok_or_else(|| invalid("approvals", "must be a JSON object"))?;
    let (keep, drop) = if allow {
        ("allow", "deny")
    } else {
        ("deny", "allow")
    };
    let list = approvals
        .entry(keep)
        .or_insert_with(|| Value::Array(Vec::new()));
    let list = list.as_array_mut().ok_or_else(|| {
        invalid(
            format!("approvals.{keep}"),
            "must be an array of tool names",
        )
    })?;
    if !list.iter().any(|entry| entry.as_str() == Some(tool)) {
        list.push(Value::String(tool.to_owned()));
    }
    if let Some(other) = approvals.get_mut(drop).and_then(Value::as_array_mut) {
        other.retain(|entry| entry.as_str() != Some(tool));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let mut body = serde_json::to_string_pretty(&root).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    body.push('\n');
    atomic_write(path, body.as_bytes())
}
