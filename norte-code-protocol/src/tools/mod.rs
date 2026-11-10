//! The tools that Norte does not ship.
//!
//! `glob` / `list_dir`, `grep`, `todo`, `ask_user`, `subagent` and the opt-in
//! `web_fetch`. Each tool has its own file; what they share — the output
//! bounds, the escaping every tool result goes through, and the workspace
//! walk — lives here.
//!
//! They follow Norte's conventions: strict arguments, bounded output, failures
//! returned as an error tool result rather than an engine error, and no path may
//! silently escape the workspace through `..`.

#![forbid(unsafe_code)]

mod grep;
mod question;
mod todo;
mod web_fetch;

pub use grep::{GrepArgs, GrepTool};
pub use question::{
    AskUserArgs, AskUserTool, DurableQuestion, QUESTION_TIMEOUT, QuestionBroker, QuestionEvent,
    QuestionStore,
};
pub use todo::{SubagentArgs, TodoArgs, TodoItem, TodoState, TodoStatus, TodoTool};
pub use web_fetch::{WebFetchArgs, WebFetchTool};

use std::fmt::Write as _;
use std::path::Path;

use ignore::{Walk, WalkBuilder};

/// Default cap on search results.
pub const DEFAULT_MAX_RESULTS: usize = 500;
/// Hard cap on search results, whatever the request asks for.
pub const MAX_RESULTS: usize = 2000;
/// Largest file `grep` reads.
pub const MAX_GREP_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Longest line shown by `grep`.
pub const MAX_LINE_WIDTH: usize = 500;
/// Largest response body `web_fetch` keeps.
pub const MAX_FETCH_BYTES: usize = 512 * 1024;
/// Converts a request cap into a bounded one.
fn cap(requested: Option<usize>, default: usize, ceiling: usize) -> usize {
    requested.unwrap_or(default).clamp(1, ceiling)
}

fn escape(text: &str) -> String {
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

/// Whether a character can reorder or hide the rest of a line in a terminal.
///
/// These are Unicode format characters, not control characters, so the control
/// check above does not catch them. A right-to-left override inside a grep
/// result could otherwise make the model read text the human cannot see.
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

fn clip_line(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_WIDTH) {
        Some((end, _)) => format!("{}…", escape(&line[..end])),
        None => escape(line),
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Walks `root` with the semantics both search tools document: `.gitignore`
/// applies even when the workspace is not inside a git repository, and hidden
/// files and directories are skipped.
fn walk(root: &Path) -> Walk {
    let mut builder = WalkBuilder::new(root);
    builder.hidden(true).require_git(false);
    builder.build()
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
