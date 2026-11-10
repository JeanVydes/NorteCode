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
#![allow(unused_imports, dead_code, clippy::empty_line_after_doc_comments)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use norte::{FnExecutor, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

/// Largest `SKILL.md` accepted, before parsing.
pub const MAX_SKILL_FILE_BYTES: usize = 1024 * 1024;
/// Largest body returned by the `skill` tool.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Largest resource returned by `skill_resource` unless the host lowers it.
pub const DEFAULT_RESOURCE_BYTES: usize = 256 * 1024;
/// Recommended longest `SKILL.md` before the host warns.
pub const RECOMMENDED_LINES: usize = 500;
/// Longest shebang line accepted from a bundled script.
const MAX_SHEBANG_BYTES: usize = 4096;
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
mod discovery;
mod tools;
mod validation;

pub use discovery::*;
pub use tools::*;
pub use validation::*;

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
        let mut full = false;
        for (rank, root) in roots.iter().enumerate() {
            if full {
                break;
            }
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
                    full = true;
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
                        let mut skill = skill;
                        skill.origin = (rank, root.clone());
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

    /// Every valid skill, most-specific root first, by name within a root.
    ///
    /// This is the order a prompt wants. A workspace skill is the one the
    /// session was set up for, and a bundled or user-wide one is context the
    /// session may never need; a name sort puts `amazon-bedrock` ahead of
    /// `rust-best-practices` and gives the budget to the wrong one.
    #[must_use]
    pub fn skills_by_precedence(&self) -> Vec<&Skill> {
        let mut out: Vec<&Skill> = self.skills.values().collect();
        // The later root in `discovery_roots` is the more specific one, so the
        // ranking is reversed: a workspace skill shadows a user-wide one.
        out.sort_by(|a, b| {
            b.origin
                .0
                .cmp(&a.origin.0)
                .then_with(|| a.name.cmp(&b.name))
        });
        out
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
        if let Some(session) = session
            && let Some(active) = self.lock_active().get_mut(&session)
        {
            active.remove(name);
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
                        .is_some_and(|pattern| validation::bash_allows(pattern, arguments)),
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
    let root = std::fs::canonicalize(directory)
        .map_err(|error| format!("could not resolve {}: {error}", directory.display()))?;
    let file = root.join("SKILL.md");
    let metadata = std::fs::metadata(&file)
        .map_err(|error| format!("could not read {}: {error}", file.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", file.display()));
    }
    if metadata.len() > MAX_SKILL_FILE_BYTES as u64 {
        return Err(format!(
            "SKILL.md is larger than {MAX_SKILL_FILE_BYTES} bytes"
        ));
    }
    // A project can carry `SKILL.md -> /etc/passwd`. Discovery runs before the
    // workspace policy, so check the resolved file against the skill root.
    let canonical_file = std::fs::canonicalize(&file)
        .map_err(|error| format!("could not resolve {}: {error}", file.display()))?;
    if !canonical_file.starts_with(&root) {
        return Err(format!("{} leaves the skill directory", file.display()));
    }
    let mut text = String::new();
    let source = std::fs::File::open(&canonical_file)
        .map_err(|error| format!("could not read {}: {error}", file.display()))?;
    source
        .take(MAX_SKILL_FILE_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("could not read {}: {error}", file.display()))?;
    if text.len() > MAX_SKILL_FILE_BYTES {
        return Err(format!(
            "SKILL.md is larger than {MAX_SKILL_FILE_BYTES} bytes"
        ));
    }
    let parsed = validation::parse_skill(&text, directory_name)?;
    let lines = parsed.body.lines().count();
    let warning = (lines > RECOMMENDED_LINES).then(|| {
        format!(
            "SKILL.md has {lines} lines; the specification recommends under {RECOMMENDED_LINES}, with detail in `references/`"
        )
    });
    Ok((
        Skill {
            name: parsed.name,
            description: parsed.description,
            license: parsed.license,
            compatibility: parsed.compatibility,
            metadata: parsed.metadata,
            allowed_tools: parsed.allowed_tools,
            root,
            // Filled in by `discover`, which is what knows the root's rank.
            origin: (usize::MAX, PathBuf::new()),
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

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
