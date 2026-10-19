//! Agent Skills discovery, validation and progressive-disclosure tools.
//!
//! A skill is a directory with a `SKILL.md` file: YAML frontmatter plus Markdown
//! instructions, and optionally `scripts/`, `references/` and `assets/`. This
//! crate follows the Agent Skills specification at
//! <https://agentskills.io/specification>.
//!
//! Discovery never executes anything. The [`SkillRegistry`] validates each
//! skill, keeps its metadata and body, and exposes three tools:
//!
//! - `skill`: list skills, or activate one and return its instructions.
//! - `skill_resource`: read a file inside a skill root, on demand.
//! - `skill_script`: run a bundled script, only when the host enables it.
//!
//! The tools follow Norte's conventions: strict arguments, bounded output,
//! failures as [`ToolOutput::error`], and no engine errors for user mistakes.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use norte::{FnExecutor, Result, SessionId, ToolContext, ToolDeclaration, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Largest `SKILL.md` accepted, before parsing.
pub const MAX_SKILL_FILE_BYTES: usize = 1024 * 1024;
/// Largest body returned by the `skill` tool.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Largest resource returned by `skill_resource` unless the host lowers it.
pub const DEFAULT_RESOURCE_BYTES: usize = 256 * 1024;
/// Recommended longest `SKILL.md` before the host warns.
pub const RECOMMENDED_LINES: usize = 500;
/// Script output kept per stream.
pub const MAX_SCRIPT_OUTPUT_BYTES: usize = 1024 * 1024;
/// Script output that reaches the model, per stream.
pub const MAX_SCRIPT_MODEL_BYTES: usize = 32 * 1024;
/// Default script timeout.
pub const DEFAULT_SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);
/// Longest script timeout a request may ask for.
pub const MAX_SCRIPT_TIMEOUT: Duration = Duration::from_secs(600);

/// The roots discovery uses, in increasing precedence.
///
/// The user root is the platform config directory; `extra` paths are relative
/// to `workspace` unless absolute, and the project roots come last.
#[must_use]
pub fn discovery_roots(workspace: &Path, extra: &[PathBuf], interop: bool) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let config_base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    if let Some(base) = config_base {
        roots.push(base.join("norte-code").join("skills"));
    }
    if interop {
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join(".agents").join("skills"));
        }
    }
    for path in extra {
        roots.push(if path.is_absolute() {
            path.clone()
        } else {
            workspace.join(path)
        });
    }
    roots.push(workspace.join(".norte-code").join("skills"));
    if interop {
        roots.push(workspace.join(".agents").join("skills"));
    }
    roots
}

/// The frontmatter as the specification defines it.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
}

/// One valid skill.
#[derive(Clone, Debug)]
pub struct Skill {
    /// The validated name, equal to the directory name.
    pub name: String,
    /// What the skill does and when to use it.
    pub description: String,
    /// An optional license name or file reference.
    pub license: Option<String>,
    /// Optional environment requirements.
    pub compatibility: Option<String>,
    /// Free-form string metadata.
    pub metadata: BTreeMap<String, String>,
    /// Tools the skill declares as pre-approved (experimental).
    pub allowed_tools: Vec<String>,
    /// The canonical skill directory.
    pub root: PathBuf,
    /// The Markdown body after the frontmatter.
    pub body: String,
}

impl Skill {
    /// A bounded list of files under `scripts/`, `references/` and `assets/`.
    #[must_use]
    pub fn resources(&self) -> Vec<String> {
        let mut resources = Vec::new();
        for directory in ["references", "scripts", "assets"] {
            let path = self.root.join(directory);
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(std::result::Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect();
            names.sort();
            for name in names {
                resources.push(format!("{directory}/{name}"));
            }
        }
        resources
    }

    /// Reads a resource inside this skill, refusing escapes and oversized files.
    ///
    /// # Errors
    ///
    /// Returns a message when the path is absolute, leaves the skill root, is
    /// not a regular file, or cannot be read.
    pub fn read_resource(
        &self,
        path: &str,
        max_bytes: usize,
    ) -> std::result::Result<String, String> {
        let relative = safe_relative(path)?;
        let target = self.root.join(&relative);
        let canonical = std::fs::canonicalize(&target)
            .map_err(|error| format!("could not open `{path}`: {error}"))?;
        if !canonical.starts_with(&self.root) {
            return Err(format!("`{path}` leaves the skill directory"));
        }
        let metadata = std::fs::metadata(&canonical)
            .map_err(|error| format!("could not inspect `{path}`: {error}"))?;
        if !metadata.is_file() {
            return Err(format!("`{path}` is not a file"));
        }
        let bytes = std::fs::read(&canonical)
            .map_err(|error| format!("could not read `{path}`: {error}"))?;
        let truncated = bytes.len() > max_bytes;
        let shown = &bytes[..bytes.len().min(max_bytes)];
        let mut text = escape(&String::from_utf8_lossy(shown));
        if truncated {
            let _ = write!(
                text,
                "\n\n[truncated at {max_bytes} bytes of {}; read a smaller file or a narrower resource]",
                bytes.len()
            );
        }
        Ok(text)
    }

    /// The script path, resolved under `scripts/` and checked against escapes.
    ///
    /// # Errors
    ///
    /// Returns a message when the path is not a `scripts/` file inside the
    /// skill.
    pub fn script_path(&self, script: &str) -> std::result::Result<PathBuf, String> {
        let relative = safe_relative(script)?;
        let mut components = relative.components();
        match components.next() {
            Some(Component::Normal(first)) if first == "scripts" => {}
            _ => return Err("scripts must live under `scripts/`".to_owned()),
        }
        let target = self.root.join(&relative);
        let canonical = std::fs::canonicalize(&target)
            .map_err(|error| format!("could not open `{script}`: {error}"))?;
        if !canonical.starts_with(self.root.join("scripts")) {
            return Err(format!("`{script}` leaves the skill's scripts directory"));
        }
        if !canonical.is_file() {
            return Err(format!("`{script}` is not a file"));
        }
        Ok(canonical)
    }
}

/// A skill that was skipped, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillWarning {
    /// The path that failed.
    pub path: PathBuf,
    /// A message written for the user.
    pub message: String,
}

/// The skills discovered from a set of roots.
#[derive(Debug)]
pub struct SkillRegistry {
    skills: BTreeMap<String, Skill>,
    warnings: Vec<SkillWarning>,
    active: Mutex<BTreeMap<SessionId, BTreeSet<String>>>,
}

impl SkillRegistry {
    /// Discovers skills from `roots`, in increasing precedence.
    ///
    /// A later root shadows an earlier one with the same name, and the shadowing
    /// is reported. A malformed skill is skipped with a warning; discovery never
    /// fails and never executes anything.
    #[must_use]
    pub fn discover(roots: &[PathBuf], max_skills: usize) -> Self {
        let mut registry = Self {
            skills: BTreeMap::new(),
            warnings: Vec::new(),
            active: Mutex::new(BTreeMap::new()),
        };
        let limit = max_skills.max(1);
        for root in roots {
            if !root.is_dir() {
                continue;
            }
            let mut entries: Vec<PathBuf> = match std::fs::read_dir(root) {
                Ok(entries) => entries
                    .filter_map(std::result::Result::ok)
                    .map(|entry| entry.path())
                    .collect(),
                Err(error) => {
                    registry.warn(root.clone(), format!("could not read skills root: {error}"));
                    continue;
                }
            };
            entries.sort();
            for path in entries {
                if registry.skills.len() >= limit {
                    registry.warn(
                        path,
                        format!("more than {limit} skills; the rest were ignored"),
                    );
                    break;
                }
                match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.file_type().is_dir() => {}
                    Ok(_) => continue,
                    Err(error) => {
                        registry.warn(path, format!("could not inspect skill: {error}"));
                        continue;
                    }
                }
                match load_skill(&path) {
                    Ok((skill, warning)) => {
                        if let Some(warning) = warning {
                            registry.warn(path.clone(), warning);
                        }
                        if registry.skills.contains_key(&skill.name) {
                            registry.warn(
                                path,
                                format!(
                                    "skill `{}` shadows an earlier skill with the same name",
                                    skill.name
                                ),
                            );
                        }
                        registry.skills.insert(skill.name.clone(), skill);
                    }
                    Err(message) => registry.warn(path, message),
                }
            }
        }
        registry
    }

    fn warn(&mut self, path: PathBuf, message: impl Into<String>) {
        self.warnings.push(SkillWarning {
            path,
            message: message.into(),
        });
    }

    /// Every valid skill, ordered by name.
    #[must_use]
    pub fn skills(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    /// One skill by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// How many valid skills were discovered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether no valid skill was discovered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// The skills that were skipped.
    #[must_use]
    pub fn warnings(&self) -> &[SkillWarning] {
        &self.warnings
    }

    /// Marks a skill active for a session, returning the skill.
    ///
    /// # Errors
    ///
    /// Returns a message when no skill has that name.
    pub fn activate(
        &self,
        session: Option<SessionId>,
        name: &str,
    ) -> std::result::Result<&Skill, String> {
        let skill = self
            .get(name)
            .ok_or_else(|| format!("unknown skill `{name}`; {}", self.names_hint()))?;
        if let Some(session) = session {
            self.lock_active()
                .entry(session)
                .or_default()
                .insert(name.to_owned());
        }
        Ok(skill)
    }

    /// Marks a skill inactive for a session. Unknown names are harmless.
    pub fn deactivate(&self, session: Option<SessionId>, name: &str) {
        if let Some(session) = session {
            if let Some(active) = self.lock_active().get_mut(&session) {
                active.remove(name);
            }
        }
    }

    /// Whether a skill is active for a session.
    #[must_use]
    pub fn is_active(&self, session: Option<SessionId>, name: &str) -> bool {
        session.is_some_and(|session| {
            self.lock_active()
                .get(&session)
                .is_some_and(|active| active.contains(name))
        })
    }

    /// Whether an active skill's experimental `allowed-tools` pre-approves a
    /// call. This only ever turns an approval into an allow; a policy that
    /// denies still wins when the decisions are combined.
    #[must_use]
    pub fn preapproves(
        &self,
        session: SessionId,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> bool {
        let Some(active) = self.lock_active().get(&session).cloned() else {
            return false;
        };
        for name in active {
            let Some(skill) = self.skills.get(&name) else {
                continue;
            };
            for entry in &skill.allowed_tools {
                let allowed = match entry.as_str() {
                    "Read" => tool == "read",
                    "Write" => tool == "write",
                    "Edit" => tool == "edit",
                    "Bash" => tool == "command",
                    entry if tool == "command" => entry
                        .strip_prefix("Bash(")
                        .and_then(|inner| inner.strip_suffix(')'))
                        .is_some_and(|pattern| bash_allows(pattern, arguments)),
                    _ => false,
                };
                if allowed {
                    return true;
                }
            }
        }
        false
    }

    /// The names active for a session.
    #[must_use]
    pub fn active(&self, session: Option<SessionId>) -> Vec<String> {
        session.map_or_else(Vec::new, |session| {
            self.lock_active()
                .get(&session)
                .map(|active| active.iter().cloned().collect())
                .unwrap_or_default()
        })
    }

    fn names_hint(&self) -> String {
        if self.skills.is_empty() {
            return "no skills are installed".to_owned();
        }
        let names: Vec<&str> = self.skills.keys().map(String::as_str).collect();
        format!("known skills: {}", names.join(", "))
    }

    fn lock_active(&self) -> MutexGuard<'_, BTreeMap<SessionId, BTreeSet<String>>> {
        self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Loads and validates one skill directory.
fn load_skill(directory: &Path) -> std::result::Result<(Skill, Option<String>), String> {
    let Some(directory_name) = directory.file_name().and_then(|name| name.to_str()) else {
        return Err("the skill directory name is not UTF-8".to_owned());
    };
    let file = directory.join("SKILL.md");
    let metadata = std::fs::metadata(&file)
        .map_err(|error| format!("could not read {}: {error}", file.display()))?;
    if metadata.len() > MAX_SKILL_FILE_BYTES as u64 {
        return Err(format!(
            "SKILL.md is larger than {MAX_SKILL_FILE_BYTES} bytes"
        ));
    }
    let text = std::fs::read_to_string(&file)
        .map_err(|error| format!("could not read {}: {error}", file.display()))?;
    let parsed = parse_skill(&text, directory_name)?;
    let lines = parsed.body.lines().count();
    let warning = (lines > RECOMMENDED_LINES).then(|| {
        format!(
            "SKILL.md has {lines} lines; the specification recommends under {RECOMMENDED_LINES}, with detail in `references/`"
        )
    });
    let root = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
    Ok((
        Skill {
            name: parsed.name,
            description: parsed.description,
            license: parsed.license,
            compatibility: parsed.compatibility,
            metadata: parsed.metadata,
            allowed_tools: parsed.allowed_tools,
            root,
            body: parsed.body,
        },
        warning,
    ))
}

/// Parses and validates `SKILL.md` text.
///
/// # Errors
///
/// Returns the first specification violation.
pub fn parse_skill(text: &str, directory_name: &str) -> std::result::Result<Skill, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (frontmatter, body) = split_frontmatter(text)?;
    let frontmatter: Frontmatter = serde_norway::from_str(&frontmatter)
        .map_err(|error| format!("invalid YAML frontmatter: {error}"))?;
    validate_name(&frontmatter.name, Some(directory_name))?;
    validate_description(&frontmatter.description)?;
    if let Some(license) = &frontmatter.license {
        if license.trim().is_empty() {
            return Err("`license` must not be empty when present".to_owned());
        }
    }
    if let Some(compatibility) = &frontmatter.compatibility {
        if compatibility.chars().count() > 500 {
            return Err("`compatibility` is longer than 500 characters".to_owned());
        }
    }
    let allowed_tools = frontmatter
        .allowed_tools
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect();
    Ok(Skill {
        name: frontmatter.name,
        description: frontmatter.description,
        license: frontmatter.license,
        compatibility: frontmatter.compatibility,
        metadata: frontmatter.metadata,
        allowed_tools,
        root: PathBuf::new(),
        body: body.to_owned(),
    })
}

/// Splits `---`-delimited frontmatter from the Markdown body.
fn split_frontmatter(text: &str) -> std::result::Result<(String, &str), String> {
    let (first, mut rest) = take_line(text);
    if first.trim() != "---" {
        return Err("SKILL.md must start with a `---` frontmatter line".to_owned());
    }
    let mut frontmatter = String::new();
    loop {
        if rest.is_empty() {
            return Err("the YAML frontmatter has no closing `---` line".to_owned());
        }
        let (line, after) = take_line(rest);
        if line.trim() == "---" {
            return Ok((frontmatter, after));
        }
        frontmatter.push_str(line);
        frontmatter.push('\n');
        rest = after;
    }
}

fn take_line(text: &str) -> (&str, &str) {
    match text.find('\n') {
        Some(index) => {
            let line = text[..index].strip_suffix('\r').unwrap_or(&text[..index]);
            (line, &text[index + 1..])
        }
        None => (text.strip_suffix('\r').unwrap_or(text), ""),
    }
}

/// Validates a skill name and, when given, that it matches its directory.
///
/// # Errors
///
/// Returns the specification rule the name breaks.
pub fn validate_name(name: &str, directory: Option<&str>) -> std::result::Result<(), String> {
    if name.is_empty() || name.chars().count() > 64 {
        return Err("`name` must be 1-64 characters".to_owned());
    }
    if !name.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return Err("`name` may only contain lowercase letters, digits and hyphens".to_owned());
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("`name` must not start or end with a hyphen".to_owned());
    }
    if name.contains("--") {
        return Err("`name` must not contain consecutive hyphens".to_owned());
    }
    if let Some(directory) = directory {
        if name != directory {
            return Err(format!(
                "`name` must match the directory name (`{directory}`)"
            ));
        }
    }
    Ok(())
}

/// Validates a description.
///
/// # Errors
///
/// Returns a message when it is empty or longer than 1024 characters.
pub fn validate_description(description: &str) -> std::result::Result<(), String> {
    let length = description.chars().count();
    if length == 0 || length > 1024 {
        return Err("`description` must be 1-1024 characters".to_owned());
    }
    if description.trim().is_empty() {
        return Err("`description` must not be blank".to_owned());
    }
    Ok(())
}

/// A relative path with no `..`, root or prefix components.
fn safe_relative(path: &str) -> std::result::Result<PathBuf, String> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err("the path must be relative to the skill directory".to_owned());
    }
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("the path must not leave the skill directory".to_owned());
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err("the path must name a file".to_owned());
    }
    Ok(relative)
}

/// Matches the experimental `Bash(git:*)` form against a command call.
fn bash_allows(pattern: &str, arguments: &serde_json::Value) -> bool {
    if pattern == "*" {
        return true;
    }
    let program = arguments.get("command").and_then(|command| {
        match command.get("kind").and_then(serde_json::Value::as_str) {
            Some("program") => command
                .get("program")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            Some("shell") => command
                .get("script")
                .and_then(serde_json::Value::as_str)
                .and_then(|script| script.split_whitespace().next())
                .map(ToOwned::to_owned),
            _ => None,
        }
    });
    let Some(program) = program else {
        return false;
    };
    match pattern.split_once(':') {
        Some((prefix, "*")) => program == prefix,
        Some(_) => false,
        None => program == pattern,
    }
}

fn escape(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() && character != '\n' && character != '\t' {
            let _ = write!(output, "\\u{{{:x}}}", u32::from(character));
        } else {
            output.push(character);
        }
    }
    output
}

fn clip(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[truncated at {max_bytes} bytes; continue with a narrower request]",
        &text[..end]
    )
}

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
    registry: Arc<SkillRegistry>,
}

impl SkillTool {
    /// A tool over `registry`.
    #[must_use]
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
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
                    warning.path.display(),
                    warning.message
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
                skill.name,
                skill.description,
                skill.root.display()
            );
        }
        ToolOutput::text(clip(&text, MAX_BODY_BYTES))
    }

    fn activate(&self, session: Option<SessionId>, name: &str) -> ToolOutput {
        match self.registry.activate(session, name) {
            Ok(skill) => {
                let mut text = format!("# skill: {}\n\n{}", skill.name, skill.body);
                let resources = skill.resources();
                if resources.is_empty() {
                    text.push_str("\n\n(no resources)");
                } else {
                    text.push_str("\n\nResources (read only when needed):");
                    for resource in resources {
                        let _ = write!(text, "\n- {resource}");
                    }
                }
                ToolOutput::text(clip(&text, MAX_BODY_BYTES))
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
    registry: Arc<SkillRegistry>,
    max_bytes: usize,
}

impl SkillResourceTool {
    /// A tool over `registry`, with a bounded read.
    #[must_use]
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self {
            registry,
            max_bytes: DEFAULT_RESOURCE_BYTES,
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
            timeout: DEFAULT_SCRIPT_TIMEOUT,
            max_output_bytes: MAX_SCRIPT_OUTPUT_BYTES,
            max_model_bytes: MAX_SCRIPT_MODEL_BYTES,
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
    registry: Arc<SkillRegistry>,
    config: SkillScriptConfig,
}

impl SkillScriptTool {
    /// A tool over `registry`, disabled unless the host registers it.
    #[must_use]
    pub fn new(registry: Arc<SkillRegistry>, config: SkillScriptConfig) -> Self {
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
        .with_approval("run a script bundled with a skill")
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
            .min(MAX_SCRIPT_TIMEOUT);
        let child = command
            .spawn()
            .map_err(|error| format!("could not start `{program}`: {error}"))?;
        let mut wait = Box::pin(child.wait_with_output());
        let output = tokio::select! {
            () = context.cancelled() => {
                return Ok(ToolOutput::error("skill_script cancelled"));
            }
            result = tokio::time::timeout(timeout, &mut wait) => {
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
            "exit: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            render(&output.stdout, self.config.max_model_bytes),
            render(&output.stderr, self.config.max_model_bytes)
        );
        text = clip(&text, self.config.max_model_bytes.saturating_mul(2));
        if output.status.success() {
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

fn render(bytes: &[u8], max_bytes: usize) -> String {
    let shown = &bytes[..bytes.len().min(max_bytes)];
    let mut text = escape(&String::from_utf8_lossy(shown));
    if bytes.len() > max_bytes {
        let _ = write!(
            text,
            "\n[truncated at {max_bytes} bytes of {}]",
            bytes.len()
        );
    }
    text.trim_end().to_owned()
}

fn interpreter(script: &Path) -> std::result::Result<(String, Vec<String>), String> {
    let bytes = std::fs::read(script)
        .map_err(|error| format!("could not read {}: {error}", script.display()))?;
    let first_line = bytes
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let first_line = String::from_utf8_lossy(first_line);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, frontmatter: &str, body: &str) -> PathBuf {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).expect("skill dir");
        std::fs::write(
            directory.join("SKILL.md"),
            format!("---\n{frontmatter}\n---\n{body}"),
        )
        .expect("SKILL.md");
        directory
    }

    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("norte-code-skills-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    #[test]
    fn valid_names_follow_the_specification() {
        for name in ["pdf-processing", "data-analysis", "code-review", "a1"] {
            assert!(validate_name(name, Some(name)).is_ok(), "{name}");
        }
        for name in [
            "PDF-Processing",
            "-pdf",
            "pdf-",
            "pdf--processing",
            "pdf_processing",
        ] {
            assert!(validate_name(name, Some(name)).is_err(), "{name}");
        }
        assert!(validate_name("pdf", Some("other")).is_err());
    }

    #[test]
    fn a_valid_skill_is_parsed() {
        let skill = parse_skill(
            "---\nname: pdf-processing\ndescription: Extract PDF text and forms. Use for PDFs.\nlicense: Apache-2.0\nmetadata:\n  author: example\nallowed-tools: Bash(git:*) Read\n---\n# Body\n",
            "pdf-processing",
        )
        .expect("skill");
        assert_eq!(skill.name, "pdf-processing");
        assert_eq!(skill.allowed_tools, ["Bash(git:*)", "Read"]);
        assert_eq!(
            skill.metadata.get("author").map(String::as_str),
            Some("example")
        );
        assert!(skill.body.contains("# Body"));
    }

    #[test]
    fn malformed_skills_are_skipped_with_a_warning() {
        let root = temp_root("warn");
        write_skill(
            &root,
            "good",
            "name: good\ndescription: A good skill.",
            "body",
        );
        write_skill(&root, "Bad-Name", "name: Bad-Name\ndescription: x", "body");
        write_skill(&root, "empty", "name: empty\ndescription: \"\"", "body");
        let registry = SkillRegistry::discover(std::slice::from_ref(&root), 16);
        assert!(registry.get("good").is_some());
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.warnings().len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_later_root_shadows_an_earlier_one() {
        let user = temp_root("user");
        let project = temp_root("project");
        write_skill(&user, "same", "name: same\ndescription: user", "user body");
        write_skill(
            &project,
            "same",
            "name: same\ndescription: project",
            "project body",
        );
        let registry = SkillRegistry::discover(&[user.clone(), project.clone()], 16);
        assert_eq!(
            registry.get("same").map(|skill| skill.description.as_str()),
            Some("project")
        );
        assert!(
            registry
                .warnings()
                .iter()
                .any(|warning| warning.message.contains("shadows"))
        );
        let _ = std::fs::remove_dir_all(user);
        let _ = std::fs::remove_dir_all(project);
    }

    #[test]
    fn resources_cannot_escape_the_skill_root() {
        let root = temp_root("resources");
        let directory = write_skill(
            &root,
            "reader",
            "name: reader\ndescription: Reads things.",
            "body",
        );
        std::fs::create_dir_all(directory.join("references")).expect("references");
        std::fs::write(directory.join("references/REFERENCE.md"), "hello").expect("reference");
        std::fs::write(root.join("secret.txt"), "secret").expect("secret");
        let skill = load_skill(&directory).expect("skill").0;
        assert_eq!(
            skill
                .read_resource("references/REFERENCE.md", 1024)
                .expect("read"),
            "hello"
        );
        assert!(skill.read_resource("../secret.txt", 1024).is_err());
        assert!(skill.read_resource("/etc/passwd", 1024).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_closing_fence_is_an_error() {
        assert!(parse_skill("---\nname: x\n", "x").is_err());
        assert!(parse_skill("no frontmatter", "x").is_err());
    }
}
