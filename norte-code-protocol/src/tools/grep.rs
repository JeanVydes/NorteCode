use std::fmt::Write as _;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use globset::Glob;
use norte::{FnExecutor, Result, ToolDeclaration, ToolOutput};
use regex::RegexBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    DEFAULT_MAX_RESULTS, MAX_GREP_FILE_BYTES, MAX_RESULTS, cap, clip_line, relative, walk,
};

/// Arguments for `grep`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrepArgs {
    /// A Rust regular expression, applied line by line.
    pub pattern: String,
    /// The file or directory to search; defaults to the workspace root.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Only search files whose path matches this glob.
    #[serde(default)]
    pub glob: Option<String>,
    /// Match without regard to case.
    #[serde(default)]
    pub ignore_case: bool,
    /// The largest number of matching lines to return.
    #[serde(default)]
    pub max_matches: Option<usize>,
    /// Skip this many matching lines first, for paging.
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Search a project for a regular expression.
#[derive(Clone, Debug, Default)]
pub struct GrepTool {
    root: PathBuf,
}

impl GrepTool {
    /// A grep tool rooted at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Turns the tool into a declaration the core registry accepts.
    #[must_use]
    pub fn into_declaration(self) -> ToolDeclaration<GrepArgs> {
        ToolDeclaration::new(
            "grep",
            "Search files line by line with a regular expression. Honors .gitignore, skips binary files and returns `path:line: text`. Use it to find a symbol or string before reading the surrounding code.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<GrepArgs, ToolOutput> for GrepTool {
    #[allow(clippy::too_many_lines)]
    async fn call(&self, args: GrepArgs) -> Result<ToolOutput> {
        let pattern = match RegexBuilder::new(&args.pattern)
            .case_insensitive(args.ignore_case)
            .size_limit(1024 * 1024)
            .dfa_size_limit(256 * 1024)
            .build()
        {
            Ok(pattern) => pattern,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "invalid regular expression `{}`: {error}",
                    args.pattern
                )));
            }
        };
        let filter = match args.glob.as_deref().map(Glob::new).transpose() {
            Ok(glob) => glob.map(|glob| glob.compile_matcher()),
            Err(error) => {
                return Ok(ToolOutput::error(format!("invalid glob: {error}")));
            }
        };
        let root = args.path.unwrap_or_else(|| self.root.clone());
        let limit = cap(args.max_matches, DEFAULT_MAX_RESULTS, MAX_RESULTS);
        let offset = args.offset.unwrap_or(0);
        let mut matches = Vec::new();
        let mut seen = 0usize;
        let mut truncated = false;

        let files: Box<dyn Iterator<Item = PathBuf>> = if root.is_file() {
            Box::new(std::iter::once(root.clone()))
        } else {
            Box::new(walk(&root).flatten().filter_map(|entry| {
                entry
                    .file_type()
                    .is_some_and(|kind| kind.is_file())
                    .then(|| entry.into_path())
            }))
        };
        for file in files {
            let shown = relative(&root, &file);
            if filter
                .as_ref()
                .is_some_and(|filter| !filter.is_match(&shown))
            {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&file) else {
                continue;
            };
            if metadata.len() > MAX_GREP_FILE_BYTES {
                continue;
            }
            let Ok(source) = std::fs::File::open(&file) else {
                continue;
            };
            let mut bytes = Vec::new();
            if source
                .take(MAX_GREP_FILE_BYTES.saturating_add(1))
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() as u64 > MAX_GREP_FILE_BYTES
            {
                continue;
            }
            if bytes.iter().take(8192).any(|byte| *byte == 0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                if !pattern.is_match(line) {
                    continue;
                }
                if seen < offset {
                    seen += 1;
                    continue;
                }
                seen += 1;
                matches.push(format!(
                    "{shown}:{}: {}",
                    index + 1,
                    clip_line(line.trim_end())
                ));
                if matches.len() >= limit {
                    truncated = true;
                    break;
                }
            }
            if truncated {
                break;
            }
        }
        if matches.is_empty() {
            return Ok(ToolOutput::text(format!(
                "no matches for `{}`",
                args.pattern
            )));
        }
        let mut text = matches.join("\n");
        if truncated {
            let next = offset + matches.len();
            let _ = write!(
                text,
                "\n\n[showing {limit} matches from offset {offset}; continue with offset {next}]"
            );
        }
        Ok(ToolOutput::text(text))
    }
}
