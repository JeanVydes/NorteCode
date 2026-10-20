//! The optional OS sandbox for the `command` tool.
//!
//! The wrapper sits inside the tool executor, after policies, approvals and the
//! effect journal. A human approves and the audit records the original command;
//! only the process invocation is rewritten.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use norte::{FnExecutor, ToolContext, ToolDeclaration, ToolOutput};
use norte_code_config::SandboxConfig;
use norte_tools::CommandRequest;

#[cfg(any(target_os = "macos", test))]
use std::fmt::Write as _;

/// Wraps the command tool's executor so the OS sandbox runs the same call.
///
/// The wrapper is deliberately inside the executor, after policies, approvals
/// and the effect journal: the human approves and the audit records the
/// original command, while only the process invocation gains the sandbox.
pub(super) fn sandboxed_command(
    declaration: ToolDeclaration<CommandRequest>,
    workspace: &Path,
    config: &SandboxConfig,
) -> ToolDeclaration<CommandRequest> {
    let writable = config
        .writable
        .iter()
        .filter_map(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                workspace.join(path)
            };
            std::fs::canonicalize(path).ok()
        })
        .collect();
    ToolDeclaration {
        name: declaration.name,
        description: declaration.description,
        parameters: declaration.parameters,
        executor: Arc::new(SandboxExecutor {
            inner: declaration.executor,
            workspace: workspace.to_path_buf(),
            network: config.network,
            writable,
        }),
        approval: declaration.approval,
    }
}

/// Runs the command tool's request through the platform sandbox.
struct SandboxExecutor {
    inner: Arc<dyn FnExecutor<CommandRequest, ToolOutput>>,
    workspace: PathBuf,
    network: bool,
    writable: Vec<PathBuf>,
}

#[async_trait]
impl FnExecutor<CommandRequest, ToolOutput> for SandboxExecutor {
    async fn call(&self, args: CommandRequest) -> norte::Result<ToolOutput> {
        let args = sandbox_request(&args, &self.workspace, self.network, &self.writable)
            .map_err(norte::Error::InvalidInput)?;
        self.inner.call(args).await
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: CommandRequest,
    ) -> norte::Result<ToolOutput> {
        let args = sandbox_request(&args, &self.workspace, self.network, &self.writable)
            .map_err(norte::Error::InvalidInput)?;
        self.inner.call_with_context(context, args).await
    }
}

/// Rewrites one command request to run under the platform sandbox.
pub(super) fn sandbox_request(
    request: &CommandRequest,
    workspace: &Path,
    network: bool,
    writable: &[PathBuf],
) -> std::result::Result<CommandRequest, String> {
    let value = serde_json::to_value(request).map_err(|error| error.to_string())?;
    let value = sandboxed_arguments(&value, workspace, network, writable)?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

/// Replaces the request's command with the sandbox launcher and the original
/// command. Every other field (cwd, env, stdin, limits) is kept.
pub(super) fn sandboxed_arguments(
    arguments: &serde_json::Value,
    workspace: &Path,
    network: bool,
    writable: &[PathBuf],
) -> std::result::Result<serde_json::Value, String> {
    let mut request = arguments
        .as_object()
        .cloned()
        .ok_or_else(|| "tool arguments must be a JSON object".to_owned())?;
    let (program, args, cwd) = {
        let command = request
            .get("command")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| "missing `command`".to_owned())?;
        let kind = command
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "missing command kind".to_owned())?;
        let (program, args) = match kind {
            "program" => {
                let program = command
                    .get("program")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing `program`".to_owned())?
                    .to_owned();
                let args = match command.get("args") {
                    None => Vec::new(),
                    Some(serde_json::Value::Array(values)) => values
                        .iter()
                        .map(|value| {
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| "`args` must contain only strings".to_owned())
                        })
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                    Some(_) => return Err("`args` must be an array".to_owned()),
                };
                (program, args)
            }
            "shell" => {
                let shell = command
                    .get("shell")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("sh");
                let script = command
                    .get("script")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing `script`".to_owned())?;
                (shell.to_owned(), vec!["-c".to_owned(), script.to_owned()])
            }
            other => return Err(format!("unknown command kind `{other}`")),
        };
        let cwd = match request.get("cwd") {
            None => None,
            Some(serde_json::Value::String(path)) => Some(PathBuf::from(path)),
            Some(_) => return Err("`cwd` must be a string path".to_owned()),
        };
        (program, args, cwd)
    };
    let (program, args) =
        wrap_command(program, args, cwd.as_deref(), workspace, network, writable)?;
    request.insert(
        "command".to_owned(),
        serde_json::json!({
            "kind": "program",
            "program": program,
            "args": args,
        }),
    );
    Ok(serde_json::Value::Object(request))
}

/// The display name of the platform sandbox.
pub(super) fn sandbox_name() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "bwrap"
    }
    #[cfg(target_os = "macos")]
    {
        "seatbelt"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        "sandbox"
    }
}

fn wrap_command(
    program: String,
    args: Vec<String>,
    cwd: Option<&Path>,
    workspace: &Path,
    network: bool,
    writable: &[PathBuf],
) -> std::result::Result<(String, Vec<String>), String> {
    #[cfg(target_os = "linux")]
    {
        linux_wrap(program, args, cwd, workspace, network, writable)
    }
    #[cfg(target_os = "macos")]
    {
        macos_wrap(program, args, cwd, workspace, network, writable)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (program, args, cwd, workspace, network, writable);
        Err("the command sandbox needs bubblewrap (Linux) or sandbox-exec (macOS)".to_owned())
    }
}

/// Bubblewrap: a new user/mount/pid/net/ipc/uts/cgroup namespace, a read-only
/// host root, and explicit read-write binds for the workspace and extras.
#[cfg(target_os = "linux")]
fn linux_wrap(
    program: String,
    args: Vec<String>,
    cwd: Option<&Path>,
    workspace: &Path,
    network: bool,
    writable: &[PathBuf],
) -> std::result::Result<(String, Vec<String>), String> {
    let workspace = workspace
        .to_str()
        .ok_or_else(|| "the workspace path is not UTF-8".to_owned())?;
    let mut wrapped = vec![
        "--die-with-parent".to_owned(),
        "--new-session".to_owned(),
        "--unshare-all".to_owned(),
        "--cap-drop".to_owned(),
        "ALL".to_owned(),
        "--ro-bind".to_owned(),
        "/".to_owned(),
        "/".to_owned(),
        "--dev".to_owned(),
        "/dev".to_owned(),
        "--proc".to_owned(),
        "/proc".to_owned(),
        "--tmpfs".to_owned(),
        "/tmp".to_owned(),
        "--bind".to_owned(),
        workspace.to_owned(),
        workspace.to_owned(),
    ];
    if network {
        wrapped.push("--share-net".to_owned());
    }
    for path in writable {
        let path = path
            .to_str()
            .ok_or_else(|| "a writable sandbox path is not UTF-8".to_owned())?;
        if path != workspace {
            wrapped.extend(["--bind".to_owned(), path.to_owned(), path.to_owned()]);
        }
    }
    let cwd = match cwd {
        Some(path) => path
            .to_str()
            .ok_or_else(|| "the command working directory is not UTF-8".to_owned())?,
        None => workspace,
    };
    wrapped.extend(["--chdir".to_owned(), cwd.to_owned(), "--".to_owned()]);
    wrapped.push(program);
    wrapped.extend(args);
    Ok(("bwrap".to_owned(), wrapped))
}

/// The seatbelt policy for macOS: reads stay open, writes are limited to the
/// workspace, the configured extras and the temporary roots, and the network is
/// denied unless it was enabled.
#[cfg(any(target_os = "macos", test))]
pub(super) fn macos_profile(
    workspace: &Path,
    network: bool,
    writable: &[PathBuf],
) -> std::result::Result<String, String> {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    if !network {
        profile.push_str("(deny network*)\n");
    }
    profile.push_str("(deny file-write*)\n(allow file-write*\n");
    let mut roots: Vec<&Path> = vec![workspace];
    roots.extend(writable.iter().map(PathBuf::as_path));
    roots.push(Path::new("/tmp"));
    roots.push(Path::new("/private/tmp"));
    roots.push(Path::new("/dev"));
    for root in roots {
        let path = root
            .to_str()
            .ok_or_else(|| "a sandbox path is not UTF-8".to_owned())?;
        if path.chars().any(char::is_control) {
            return Err("a sandbox path contains a control character".to_owned());
        }
        let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
        writeln!(profile, "  (subpath \"{escaped}\")").expect("writing to a String");
    }
    profile.push_str(")\n");
    Ok(profile)
}

/// macOS seatbelt: wraps the command in `sandbox-exec` with [`macos_profile`].
#[cfg(target_os = "macos")]
fn macos_wrap(
    program: String,
    args: Vec<String>,
    cwd: Option<&Path>,
    workspace: &Path,
    network: bool,
    writable: &[PathBuf],
) -> std::result::Result<(String, Vec<String>), String> {
    let _ = cwd;
    let mut wrapped = vec![
        "-p".to_owned(),
        macos_profile(workspace, network, writable)?,
        "--".to_owned(),
        program,
    ];
    wrapped.extend(args);
    Ok(("sandbox-exec".to_owned(), wrapped))
}
