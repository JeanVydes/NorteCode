#![forbid(unsafe_code)]
#![allow(missing_docs)]
#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::missing_errors_doc)]
#![allow(unused_imports)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use super::discovery::Frontmatter;
use tokio::io::AsyncReadExt;

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
    /// Which discovery root it came from, and where that root sits in the
    /// precedence order.
    ///
    /// Roots are searched most-specific last (a workspace skill shadows a
    /// bundled one), which is the order a reader expects and not the order a
    /// name sorts in. The index keeps that order recoverable after the registry
    /// has filed the skill by name.
    pub(crate) origin: (usize, PathBuf),
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
        // Read one byte past the cap so truncation is detectable without
        // loading a multi-gigabyte resource into memory first.
        let file = std::fs::File::open(&canonical)
            .map_err(|error| format!("could not read `{path}`: {error}"))?;
        let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
        file.take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("could not read `{path}`: {error}"))?;
        let truncated = bytes.len() > max_bytes;
        bytes.truncate(max_bytes);
        let mut text = escape(&String::from_utf8_lossy(&bytes));
        if truncated {
            let _ = write!(
                text,
                "\n\n[truncated at {max_bytes} bytes; read a smaller file or a narrower resource]"
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

pub fn parse_skill(text: &str, directory_name: &str) -> std::result::Result<Skill, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (frontmatter, body) = split_frontmatter(text)?;
    let frontmatter: Frontmatter = serde_norway::from_str(&frontmatter)
        .map_err(|error| format!("invalid YAML frontmatter: {error}"))?;
    validate_name(&frontmatter.name, Some(directory_name))?;
    validate_description(&frontmatter.description)?;
    if let Some(license) = &frontmatter.license
        && license.trim().is_empty()
    {
        return Err("`license` must not be empty when present".to_owned());
    }
    if let Some(compatibility) = &frontmatter.compatibility
        && compatibility.chars().count() > 500
    {
        return Err("`compatibility` is longer than 500 characters".to_owned());
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
        origin: (usize::MAX, PathBuf::new()),
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
    if let Some(directory) = directory
        && name != directory
    {
        return Err(format!(
            "`name` must match the directory name (`{directory}`)"
        ));
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
pub(super) fn bash_allows(pattern: &str, arguments: &serde_json::Value) -> bool {
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

pub(super) fn escape(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        if (character.is_control() && character != '\n' && character != '\t') || is_bidi(character)
        {
            let _ = write!(output, "\\u{{{:x}}}", u32::from(character));
        } else {
            output.push(character);
        }
    }
    output
}

/// Whether a character can reorder or hide the rest of a line.
///
/// These are Unicode format characters, not control characters. A skill body
/// or resource with a right-to-left override could otherwise make the model
/// read instructions the human cannot see.
fn is_bidi(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
    )
}

pub(super) fn clip(text: &str, max_bytes: usize) -> String {
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
