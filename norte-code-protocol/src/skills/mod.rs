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

mod tests {
    #![allow(clippy::wildcard_imports)]
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
    fn untrusted_skill_text_is_escaped() {
        assert_eq!(escape("a\u{202e}b\x1b[2J"), "a\\u{202e}b\\u{1b}[2J");
        assert_eq!(escape("a\n\tb"), "a\n\tb");
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
        let skill = validation::parse_skill(
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
    fn the_prompt_order_puts_the_most_specific_root_first() {
        // The shape this exists for: twenty-five user-wide skills whose names
        // sort early, and one workspace skill whose name sorts late. A name
        // sort gives the budget to the AWS set and leaves the workspace's own
        // skill out.
        let user = temp_root("precedence-user");
        let project = temp_root("precedence-project");
        write_skill(
            &user,
            "amazon-bedrock",
            "name: amazon-bedrock\ndescription: aws",
            "b",
        );
        write_skill(
            &user,
            "aws-compute",
            "name: aws-compute\ndescription: aws",
            "b",
        );
        write_skill(
            &project,
            "rust-best-practices",
            "name: rust-best-practices\ndescription: rust",
            "b",
        );

        // The workspace root is last, which makes it the most specific.
        let registry = SkillRegistry::discover(&[user.clone(), project.clone()], 16);
        let ordered: Vec<&str> = registry
            .skills_by_precedence()
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert_eq!(
            ordered,
            ["rust-best-practices", "amazon-bedrock", "aws-compute"],
            "the workspace skill comes first, the rest by name inside their root"
        );
        // And the alphabetical view is still available for a listing.
        let by_name: Vec<&str> = registry
            .skills()
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert_eq!(by_name[0], "amazon-bedrock", "{by_name:?}");

        let _ = std::fs::remove_dir_all(user);
        let _ = std::fs::remove_dir_all(project);
    }

    #[test]
    fn the_prompt_budget_favours_the_workspace_skill() {
        // The point of the order: with room for one, the workspace skill is the
        // one that reaches the prompt.
        let user = temp_root("budget-user");
        let project = temp_root("budget-project");
        for name in ["amazon-bedrock", "aws-compute", "aws-storage"] {
            write_skill(
                &user,
                name,
                &format!(
                    "name: {name}\ndescription: a long winded AWS description that fills the budget"
                ),
                "b",
            );
        }
        write_skill(
            &project,
            "zzz-workspace",
            "name: zzz-workspace\ndescription: the one this repo is for",
            "b",
        );

        let registry = SkillRegistry::discover(&[user.clone(), project.clone()], 16);
        let index: Vec<(String, String)> = registry
            .skills_by_precedence()
            .iter()
            .map(|skill| (skill.name.clone(), skill.description.clone()))
            .collect();
        assert_eq!(
            index[0].0, "zzz-workspace",
            "the workspace skill leads the index the prompt is built from"
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

    #[cfg(unix)]
    #[test]
    fn a_skill_file_cannot_symlink_outside_the_skill_root() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let outside = temp_root("symlink-outside");
        std::fs::write(
            outside.join("SKILL.md"),
            "---\nname: escaped\ndescription: Outside the root.\n---\nsecret",
        )
        .expect("outside skill");
        let directory = root.join("escaped");
        std::fs::create_dir_all(&directory).expect("skill dir");
        symlink(outside.join("SKILL.md"), directory.join("SKILL.md")).expect("symlink");

        assert!(load_skill(&directory).is_err());
        let registry = SkillRegistry::discover(std::slice::from_ref(&root), 8);
        assert!(registry.is_empty());
        assert!(
            registry
                .warnings()
                .iter()
                .any(|warning| warning.message.contains("leaves the skill directory"))
        );
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn a_missing_closing_fence_is_an_error() {
        assert!(validation::parse_skill("---\nname: x\n", "x").is_err());
        assert!(validation::parse_skill("no frontmatter", "x").is_err());
    }

    #[tokio::test]
    async fn a_script_runs_without_a_shell_and_with_argv_entries() {
        use norte::FnExecutor as _;

        let root = temp_root("script");
        let directory = write_skill(
            &root,
            "runner",
            "name: runner\ndescription: Runs a script.",
            "body",
        );
        std::fs::create_dir_all(directory.join("scripts")).expect("scripts");
        std::fs::write(
            directory.join("scripts/echo.sh"),
            "#!/bin/sh\nprintf '%s' \"$1\"\n",
        )
        .expect("script");
        let registry = Arc::new(SkillRegistry::discover(std::slice::from_ref(&root), 8));
        let tool = SkillScriptTool::new(registry, SkillScriptConfig::in_workspace(root.clone()));
        let output = tool
            .call(SkillScriptArgs {
                skill: "runner".to_owned(),
                script: "scripts/echo.sh".to_owned(),
                args: vec!["hello world".to_owned()],
                timeout_ms: Some(5_000),
            })
            .await
            .expect("script output");
        assert!(!output.is_error, "{output:?}");
        let text = format!("{:?}", output.parts);
        assert!(text.contains("hello world"));
        let escaped = tool
            .call(SkillScriptArgs {
                skill: "runner".to_owned(),
                script: "../echo.sh".to_owned(),
                args: Vec::new(),
                timeout_ms: None,
            })
            .await
            .expect("escape output");
        assert!(escaped.is_error);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn script_capture_keeps_at_most_the_limit() {
        let input = [b'x'; 32];
        let (bytes, truncated) = tools::read_limited(&input[..], 10).await.expect("read");
        assert_eq!(bytes.len(), 10);
        assert!(truncated);

        let (bytes, truncated) = tools::read_limited(&input[..], 64).await.expect("read");
        assert_eq!(bytes.len(), 32);
        assert!(!truncated);
    }

    #[test]
    fn a_resource_read_is_bounded() {
        let root = temp_root("resource-bound");
        let directory = write_skill(
            &root,
            "bounded",
            "name: bounded\ndescription: Has a large reference.",
            "body",
        );
        std::fs::create_dir_all(directory.join("references")).expect("references");
        std::fs::write(directory.join("references/big.txt"), "x".repeat(1024)).expect("resource");
        let registry = SkillRegistry::discover(std::slice::from_ref(&root), 8);
        let skill = registry.get("bounded").expect("skill");
        let text = skill
            .read_resource("references/big.txt", 16)
            .expect("bounded read");
        assert!(text.starts_with(&"x".repeat(16)));
        assert!(text.contains("truncated at 16 bytes"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_huge_shebang_is_refused() {
        let root = temp_root("shebang");
        let script = root.join("huge.sh");
        let mut body = String::from("#!");
        body.push_str(&"x".repeat(5_000));
        body.push('\n');
        std::fs::write(&script, body).expect("script");
        let error = tools::interpreter(&script).expect_err("shebang must be bounded");
        assert!(error.contains("longer than"));
        let _ = std::fs::remove_dir_all(root);
    }
}
