//! Workspace-root policy: re-root tool paths and refuse escapes.

use std::path::{Path, PathBuf};

use norte::{Decision, Policy, PolicyView};

/// Re-roots tool paths into the workspace and refuses an escape.
///
/// This is a gate on arguments, not a sandbox: a `command` can still reach
/// outside the workspace. It stops the common mistake of a model reading
/// `../../etc/passwd`, resolves symbolic links before comparing them with the
/// root, and refuses FIFOs, sockets and devices so a tool cannot block forever.
///
/// The root check is what `tools.confinement.workspace_only` turns off. The
/// special-file check stays either way: it protects the tool from blocking, not
/// the filesystem from the model.
pub(super) struct WorkspacePolicy {
    pub(super) root: PathBuf,
    /// Whether a path outside the workspace is refused.
    ///
    /// Configured by `tools.confinement.workspace_only`. Off, the path argument
    /// is handed through as the caller wrote it, so a host working across
    /// repositories can name any file.
    pub(super) enforce_root: bool,
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

impl Policy for WorkspacePolicy {
    fn name(&self) -> &'static str {
        "workspace-root"
    }

    fn decide(&self, view: &PolicyView<'_>) -> Decision {
        let tool = view.call.name.as_str();
        match tool {
            "read" | "write" | "edit" => self.apply(view, "path", false, PathKind::File),
            "glob" | "list_dir" => self.apply(view, "path", true, PathKind::Directory),
            "grep" => self.apply(view, "path", true, PathKind::FileOrDirectory),
            "command" => self.apply(view, "cwd", false, PathKind::Directory),
            _ => Decision::Allow,
        }
    }
}

impl WorkspacePolicy {
    /// Re-roots one path argument, then refuses a symlink escape or a special
    /// file. The kind check is what keeps a FIFO from blocking `read` forever.
    fn apply(
        &self,
        view: &PolicyView<'_>,
        key: &str,
        default_to_root: bool,
        kind: PathKind,
    ) -> Decision {
        let arguments = if self.enforce_root {
            match transform_path(&view.call.arguments, key, &self.root, default_to_root) {
                Ok(arguments) => arguments,
                Err(reason) => return Decision::Deny(reason),
            }
        } else {
            view.call.arguments.clone()
        };
        match check_path_kind(&arguments, key, kind) {
            Ok(()) => Decision::Transform(arguments),
            Err(reason) => Decision::Deny(reason),
        }
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
