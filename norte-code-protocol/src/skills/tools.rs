#![forbid(unsafe_code)]
#![allow(missing_docs)]
#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::missing_errors_doc)]
#![allow(unused_imports)]

use std::fmt::Write as _;
use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use norte::{FnExecutor, Result, SessionId, ToolContext, ToolDeclaration, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

/// The `skill` tool arguments.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillArgs {
    /// What to do with the skill registry.
    pub action: SkillAction,
    /// The skill for `activate` and `deactivate`; omit it for `list`.
    #[serde(default)]
    pub name: Option<String>,
}

/// The actions of the `skill` tool.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillAction {
    /// List every installed skill with its one-line description.
    List,
    /// Load a skill's full instructions and its resource listing.
    Activate,
    /// Stop treating a skill as active for this session.
    Deactivate,
}

/// The `skill` tool.
#[derive(Clone, Debug)]
pub struct SkillTool {
    registry: Arc<super::SkillRegistry>,
}

impl SkillTool {
    /// A tool over `registry`.
    #[must_use]
    pub fn new(registry: Arc<super::SkillRegistry>) -> Self {
        Self { registry }
    }

    /// Turns the tool into a declaration the core registry accepts.
    #[must_use]
    pub fn into_declaration(self) -> ToolDeclaration<SkillArgs> {
        ToolDeclaration::new(
            "skill",
            "List the installed skills, or activate one to load its full instructions. Activate when a skill's description matches the task; the result contains the skill body and a listing of its resources.",
            Arc::new(self),
        )
    }

    fn list(&self, session: Option<SessionId>) -> ToolOutput {
        if self.registry.is_empty() {
            let mut text = "No skills are installed.".to_owned();
            for warning in self.registry.warnings() {
                let _ = write!(
                    text,
                    "\nSkipped {}: {}",
                    super::validation::escape(&warning.path.display().to_string()),
                    super::validation::escape(&warning.message)
                );
            }
            return ToolOutput::text(text);
        }
        let mut text = String::new();
        for skill in self.registry.skills() {
            let active = if self.registry.is_active(session, &skill.name) {
                " (active)"
            } else {
                ""
            };
            let _ = writeln!(
                text,
                "- {}{active}: {}\n  {}",
                super::validation::escape(&skill.name),
                super::validation::escape(&skill.description),
                super::validation::escape(&skill.root.display().to_string())
            );
        }
        ToolOutput::text(super::validation::clip(&text, super::MAX_BODY_BYTES))
    }

    fn activate(&self, session: Option<SessionId>, name: &str) -> ToolOutput {
        match self.registry.activate(session, name) {
            Ok(skill) => {
                let mut text = format!(
                    "# skill: {}\n\n{}",
                    super::validation::escape(&skill.name),
                    super::validation::escape(&skill.body)
                );
                let resources = skill.resources();
                if resources.is_empty() {
                    text.push_str("\n\n(no resources)");
                } else {
                    text.push_str("\n\nResources (read only when needed):");
                    for resource in resources {
                        let _ = write!(text, "\n- {}", super::validation::escape(&resource));
                    }
                }
                ToolOutput::text(super::validation::clip(&text, super::MAX_BODY_BYTES))
            }
            Err(message) => ToolOutput::error(message),
        }
    }
}

#[async_trait]
impl FnExecutor<SkillArgs, ToolOutput> for SkillTool {
    async fn call(&self, args: SkillArgs) -> Result<ToolOutput> {
        Ok(match args.action {
            SkillAction::List => self.list(None),
            SkillAction::Activate => match args.name.as_deref() {
                Some(name) => self.activate(None, name),
                None => ToolOutput::error("`activate` needs a `name`"),
            },
            SkillAction::Deactivate => match args.name.as_deref() {
                Some(name) => {
                    self.registry.deactivate(None, name);
                    ToolOutput::text(format!("Skill `{name}` is no longer active."))
                }
                None => ToolOutput::error("`deactivate` needs a `name`"),
            },
        })
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: SkillArgs,
    ) -> Result<ToolOutput> {
        Ok(match args.action {
            SkillAction::List => self.list(context.session),
            SkillAction::Activate => match args.name.as_deref() {
                Some(name) => self.activate(context.session, name),
                None => ToolOutput::error("`activate` needs a `name`"),
            },
            SkillAction::Deactivate => match args.name.as_deref() {
                Some(name) => {
                    self.registry.deactivate(context.session, name);
                    ToolOutput::text(format!("Skill `{name}` is no longer active."))
                }
                None => ToolOutput::error("`deactivate` needs a `name`"),
            },
        })
    }
}

/// The `skill_resource` tool arguments.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillResourceArgs {
    /// The skill that owns the file.
    pub skill: String,
    /// A path relative to the skill root, such as `references/REFERENCE.md`.
    pub path: String,
}

/// The `skill_resource` tool.
#[derive(Clone, Debug)]
pub struct SkillResourceTool {
    registry: Arc<super::SkillRegistry>,
    max_bytes: usize,
}

impl SkillResourceTool {
    /// A tool over `registry`, with a bounded read.
    #[must_use]
    pub fn new(registry: Arc<super::SkillRegistry>) -> Self {
        Self {
            registry,
            max_bytes: super::DEFAULT_RESOURCE_BYTES,
        }
    }

    /// Lowers or raises the read cap.
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes.max(1);
        self
    }

    /// Turns the tool into a declaration the core registry accepts.
    #[must_use]
    pub fn into_declaration(self) -> ToolDeclaration<SkillResourceArgs> {
        ToolDeclaration::new(
            "skill_resource",
            "Read a file that belongs to an installed skill, such as a reference document, template or script. Paths are relative to the skill root; activate the skill first to see what it provides.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<SkillResourceArgs, ToolOutput> for SkillResourceTool {
    async fn call(&self, args: SkillResourceArgs) -> Result<ToolOutput> {
        let Some(skill) = self.registry.get(&args.skill) else {
            return Ok(ToolOutput::error(format!(
                "unknown skill `{}`; {}",
                args.skill,
                self.registry.names_hint()
            )));
        };
        Ok(match skill.read_resource(&args.path, self.max_bytes) {
            Ok(text) => ToolOutput::text(text),
            Err(message) => ToolOutput::error(format!("skill `{}`: {message}", args.skill)),
        })
    }
}

/// The `skill_script` tool arguments.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillScriptArgs {
    /// The skill that owns the script.
    pub skill: String,
    /// The script path, under the skill's `scripts/` directory.
    pub script: String,
    /// Arguments passed to the script as separate argv entries.
    #[serde(default)]
    pub args: Vec<String>,
    /// A time bound in milliseconds, clamped to the host's ceiling.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Host settings for `skill_script`.
#[derive(Clone, Debug)]
pub struct SkillScriptConfig {
    /// The workspace root the script runs in.
    pub workspace_root: PathBuf,
    /// The `PATH` the script inherits, if any.
    pub path: Option<String>,
    /// The default timeout.
    pub timeout: Duration,
    /// The output kept per stream.
    pub max_output_bytes: usize,
    /// The output that reaches the model, per stream.
    pub max_model_bytes: usize,
}

impl Default for SkillScriptConfig {
    fn default() -> Self {
        Self {
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            path: std::env::var("PATH").ok(),
            timeout: super::DEFAULT_SCRIPT_TIMEOUT,
            max_output_bytes: super::MAX_SCRIPT_OUTPUT_BYTES,
            max_model_bytes: super::MAX_SCRIPT_MODEL_BYTES,
        }
    }
}

impl SkillScriptConfig {
    /// Settings rooted at `workspace_root`.
    #[must_use]
    pub fn in_workspace(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            ..Self::default()
        }
    }
}

/// The `skill_script` tool.
#[derive(Clone, Debug)]
pub struct SkillScriptTool {
    registry: Arc<super::SkillRegistry>,
    config: SkillScriptConfig,
}

impl SkillScriptTool {
    /// A tool over `registry`, disabled unless the host registers it.
    #[must_use]
    pub fn new(registry: Arc<super::SkillRegistry>, config: SkillScriptConfig) -> Self {
        Self { registry, config }
    }

    /// Turns the tool into a declaration the core registry accepts.
    #[must_use]
    pub fn into_declaration(self) -> ToolDeclaration<SkillScriptArgs> {
        ToolDeclaration::new(
            "skill_script",
            "Run an executable script bundled with an installed skill. Use it only when the skill's instructions call for it. Arguments are passed as separate argv entries; the script runs in the workspace root with no provider secrets in its environment.",
            Arc::new(self),
        )
    }

    async fn run(
        &self,
        context: &ToolContext,
        args: SkillScriptArgs,
    ) -> std::result::Result<ToolOutput, String> {
        let skill = self
            .registry
            .get(&args.skill)
            .ok_or_else(|| format!("unknown skill `{}`", args.skill))?;
        let script = skill.script_path(&args.script)?;
        let (program, prefix) = interpreter(&script)?;
        let mut command = tokio::process::Command::new(&program);
        command
            .args(prefix)
            .arg(&script)
            .args(&args.args)
            .current_dir(&self.config.workspace_root)
            .env_clear()
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(path) = &self.config.path {
            command.env("PATH", path);
        }
        command.env("SKILL_DIR", &skill.root);

        let timeout = args
            .timeout_ms
            .map_or(self.config.timeout, Duration::from_millis)
            .min(super::MAX_SCRIPT_TIMEOUT);
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not start `{program}`: {error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "could not capture stdout".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "could not capture stderr".to_owned())?;
        let output_limit = self.config.max_output_bytes;
        // Read both pipes while the child runs, but keep at most `output_limit`
        // bytes for each. Draining the rest is what prevents a chatty script
        // from filling its pipe and deadlocking before the timeout.
        let run = Box::pin(async move {
            let (stdout, stderr) = tokio::join!(
                read_limited(stdout, output_limit),
                read_limited(stderr, output_limit)
            );
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, stdout?, stderr?))
        });
        let (status, stdout, stderr) = tokio::select! {
            () = context.cancelled() => {
                return Ok(ToolOutput::error("skill_script cancelled"));
            }
            result = tokio::time::timeout(timeout, run) => {
                match result {
                    Ok(output) => output.map_err(|error| format!("could not run `{program}`: {error}"))?,
                    Err(_) => return Ok(ToolOutput::error(format!(
                        "`{}` did not finish within {} ms",
                        args.script,
                        timeout.as_millis()
                    ))),
                }
            }
        };

        let mut text = format!(
            "exit: {status}\nstdout:\n{}\nstderr:\n{}",
            render(&stdout.0, self.config.max_model_bytes),
            render(&stderr.0, self.config.max_model_bytes)
        );
        if stdout.1 {
            let _ = write!(text, "\n[stdout truncated at {output_limit} bytes]");
        }
        if stderr.1 {
            let _ = write!(text, "\n[stderr truncated at {output_limit} bytes]");
        }
        text = super::validation::clip(&text, self.config.max_model_bytes.saturating_mul(2));
        if status.success() {
            Ok(ToolOutput::text(text))
        } else {
            Ok(ToolOutput::error(text))
        }
    }
}

#[async_trait]
impl FnExecutor<SkillScriptArgs, ToolOutput> for SkillScriptTool {
    async fn call(&self, args: SkillScriptArgs) -> Result<ToolOutput> {
        let context = ToolContext::new();
        match self.run(&context, args).await {
            Ok(output) => Ok(output),
            Err(message) => Ok(ToolOutput::error(message)),
        }
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: SkillScriptArgs,
    ) -> Result<ToolOutput> {
        match self.run(context, args).await {
            Ok(output) => Ok(output),
            Err(message) => Ok(ToolOutput::error(message)),
        }
    }
}

/// Reads a stream, keeping at most `limit` bytes but draining the rest.
///
/// The boolean is true when bytes after the cap were discarded.
pub(super) async fn read_limited<R>(mut reader: R, limit: usize) -> std::io::Result<(Vec<u8>, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        if bytes.len() < limit {
            let take = (limit - bytes.len()).min(read);
            bytes.extend_from_slice(&buffer[..take]);
            if take < read {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }
    Ok((bytes, truncated))
}

fn render(bytes: &[u8], max_bytes: usize) -> String {
    let shown = &bytes[..bytes.len().min(max_bytes)];
    let mut text = super::validation::escape(&String::from_utf8_lossy(shown));
    if bytes.len() > max_bytes {
        let _ = write!(
            text,
            "\n[truncated at {max_bytes} bytes of {}]",
            bytes.len()
        );
    }
    text.trim_end().to_owned()
}

pub(super) fn interpreter(script: &Path) -> std::result::Result<(String, Vec<String>), String> {
    let file = std::fs::File::open(script)
        .map_err(|error| format!("could not read {}: {error}", script.display()))?;
    let mut first_line = Vec::new();
    std::io::BufReader::new(file)
        .take(super::MAX_SHEBANG_BYTES as u64)
        .read_until(b'\n', &mut first_line)
        .map_err(|error| format!("could not read {}: {error}", script.display()))?;
    if first_line.len() == super::MAX_SHEBANG_BYTES && !first_line.ends_with(b"\n") {
        return Err(format!(
            "`{}` has a shebang line longer than {} bytes",
            script.display(),
            super::MAX_SHEBANG_BYTES
        ));
    }
    let first_line = String::from_utf8_lossy(&first_line);
    let Some(shebang) = first_line.strip_prefix("#!") else {
        return Err(format!(
            "`{}` has no `#!` line, so its interpreter is unknown",
            script.display()
        ));
    };
    let mut parts = shebang.split_whitespace();
    let Some(first) = parts.next() else {
        return Err("the script has an empty shebang".to_owned());
    };
    if first == "/usr/bin/env" {
        let program = parts
            .next()
            .ok_or_else(|| "`#!/usr/bin/env` needs an interpreter".to_owned())?;
        return Ok((program.to_owned(), parts.map(ToOwned::to_owned).collect()));
    }
    Ok((first.to_owned(), parts.map(ToOwned::to_owned).collect()))
}
