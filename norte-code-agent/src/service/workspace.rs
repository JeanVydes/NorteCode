//! Workspace confinement: re-root tool paths and refuse escapes.
//!
//! The wrap is a gate on arguments, not a sandbox: a `shell` call can still
//! reach outside the workspace. It stops the common mistake of a model reading
//! `../../etc/passwd`, resolves symbolic links before comparing them with the
//! root, and refuses FIFOs, sockets and devices so a tool cannot block forever.
//!
//! It is a wrapper around the declaration rather than a rule the engine
//! evaluates, because the tool's own arguments are typed here — the wrapper
//! knows which field is a path, which is the thing a rule over raw JSON has to
//! be told.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use norte::{AnyFunction, Result as NorteResult, ToolContext, ToolOutput};

/// One path argument a tool names, and what it is expected to be.
#[derive(Clone)]
pub(super) struct Confined {
    /// The field to re-root and check.
    pub(super) key: &'static str,
    /// Whether a missing path becomes the root itself.
    pub(super) default_to_root: bool,
    /// What the path is expected to name when it already exists.
    pub(super) kind: PathKind,
}

/// What a path argument is expected to name when it already exists.
#[derive(Clone, Copy)]
pub(super) enum PathKind {
    /// A regular file (or a path that does not exist yet, for a write).
    File,
    /// A directory (or, for a path that does not exist, a future one).
    Directory,
    /// Either a regular file or a directory, but never a FIFO, socket or device.
    FileOrDirectory,
}

/// The confines a tool's arguments meet, in one place.
#[derive(Clone)]
pub(super) struct Workspace {
    /// The root every path lands under.
    pub(super) root: PathBuf,
    /// Whether a path outside the workspace is refused.
    ///
    /// Configured by `tools.confinement.workspace_only`. Off, the path argument
    /// is handed through as the caller wrote it, so a host working across
    /// repositories can name any file. The special-file check stays either way:
    /// it protects the tool from blocking, not the filesystem from the model.
    pub(super) enforce_root: bool,
    /// The path arguments this tool carries.
    pub(super) fields: Vec<Confined>,
}

impl Workspace {
    /// Applies every confine to one arguments object.
    ///
    /// # Errors
    ///
    /// Fails with the reason the model reads when a path escapes or names
    /// something that is not the kind of file the tool needs.
    pub(super) fn apply(
        &self,
        arguments: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        let mut arguments = arguments.clone();
        for field in &self.fields {
            arguments = if self.enforce_root {
                transform_path(&arguments, field.key, &self.root, field.default_to_root)?
            } else {
                arguments
            };
            check_path_kind(&arguments, field.key, field.kind)?;
        }
        Ok(arguments)
    }
}

/// Wraps a tool so every call's path arguments land in the workspace.
///
/// The wrap is on the erased view, not on the typed one, because the confinement
/// is about the *raw* arguments: the path is rewritten before the tool's own
/// deserialization sees it, and a refusal is an error the model reads like any
/// other failed call.
pub(super) fn confined(inner: Arc<dyn AnyFunction>, workspace: Workspace) -> Arc<dyn AnyFunction> {
    Arc::new(ConfinedFunction { inner, workspace })
}

/// A tool whose path arguments are re-rooted and checked before it runs.
struct ConfinedFunction {
    inner: Arc<dyn AnyFunction>,
    workspace: Workspace,
}

impl std::fmt::Debug for ConfinedFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfinedFunction")
            .field("name", &self.inner.name())
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl AnyFunction for ConfinedFunction {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> &schemars::Schema {
        self.inner.parameters_schema()
    }

    fn kind(&self) -> norte::ToolKind {
        self.inner.kind()
    }

    fn declaration(&self) -> Option<serde_json::Value> {
        self.inner.declaration()
    }

    async fn execute(
        &self,
        context: &ToolContext,
        arguments: &serde_json::Value,
    ) -> NorteResult<ToolOutput> {
        let arguments = self
            .workspace
            .apply(arguments)
            .map_err(norte::Error::InvalidInput)?;
        self.inner.execute(context, &arguments).await
    }
}

/// Resolves `key` in `arguments` under `root`, rejecting an escape.
///
/// With `default_to_root`, a missing path becomes the root itself; otherwise a
/// missing key is left alone.
pub(super) fn transform_path(
    arguments: &serde_json::Value,
    key: &str,
    root: &Path,
    default_to_root: bool,
) -> std::result::Result<serde_json::Value, String> {
    let Some(object) = arguments.as_object() else {
        return Err("tool arguments must be a JSON object".to_owned());
    };
    let mut object = object.clone();
    let Some(raw) = object.get(key) else {
        if default_to_root {
            object.insert(
                key.to_owned(),
                serde_json::Value::String(root.display().to_string()),
            );
            return Ok(serde_json::Value::Object(object));
        }
        return Ok(serde_json::Value::Object(object));
    };
    let Some(raw) = raw.as_str() else {
        return Err(format!("`{key}` must be a string path"));
    };
    let path = Path::new(raw);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let resolved = resolve_inside(root, &joined)
        .map_err(|reason| format!("`{raw}` cannot be used: {reason}"))?;
    object.insert(
        key.to_owned(),
        serde_json::Value::String(resolved.display().to_string()),
    );
    Ok(serde_json::Value::Object(object))
}

/// Resolves symlinks in the part of `path` that exists, and checks the result
/// against the canonical workspace root.
///
/// A lexical check is not enough: a repository can carry `link -> /etc`, and
/// the OS follows it after the policy has approved the argument. Canonicalizing
/// the deepest existing ancestor catches that, while leaving a not-yet-created
/// file name for `write` to create.
fn resolve_inside(root: &Path, path: &Path) -> std::result::Result<PathBuf, String> {
    let canonical =
        resolve_existing_prefix(path).map_err(|error| format!("`{}`: {error}", path.display()))?;
    if !canonical.starts_with(root) {
        return Err(format!(
            "is outside the workspace root `{}`; use a path inside it",
            root.display()
        ));
    }
    Ok(canonical)
}

/// Canonicalizes the longest existing prefix of `path` and reattaches the
/// missing tail. A dangling symlink is refused rather than followed, because
/// its target could be outside the workspace and would be created there.
fn resolve_existing_prefix(path: &Path) -> std::io::Result<PathBuf> {
    let mut cursor = path;
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(cursor) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::symlink_metadata(cursor) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(std::io::Error::other(format!(
                            "{} is a symbolic link whose target does not exist",
                            cursor.display()
                        )));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                let Some(name) = cursor.file_name() else {
                    return Err(error);
                };
                missing.push(name.to_os_string());
                let Some(parent) = cursor.parent() else {
                    return Err(error);
                };
                cursor = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Refuses a path that exists and is not the kind the tool can handle.
///
/// The check follows symlinks (the policy has already established that the
/// target is inside the workspace). Its main job is a FIFO: `read` opens it
/// synchronously, and opening a FIFO with no writer blocks forever.
pub(super) fn check_path_kind(
    arguments: &serde_json::Value,
    key: &str,
    kind: PathKind,
) -> std::result::Result<(), String> {
    let Some(raw) = arguments.get(key).and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    let path = Path::new(raw);
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("`{raw}`: {error}")),
    };
    let file_type = metadata.file_type();
    let accepted = match kind {
        PathKind::File => file_type.is_file(),
        PathKind::Directory => file_type.is_dir(),
        PathKind::FileOrDirectory => file_type.is_file() || file_type.is_dir(),
    };
    if accepted {
        return Ok(());
    }
    Err(format!(
        "`{raw}` is not a {}; special files such as FIFOs, sockets and devices are refused \
         because they can block a tool indefinitely",
        match kind {
            PathKind::File => "regular file",
            PathKind::Directory => "directory",
            PathKind::FileOrDirectory => "regular file or directory",
        }
    ))
}
