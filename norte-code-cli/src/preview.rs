//! Tool previews: a call rendered as something a person can read.
//!
//! The rendering is exercised by the tests and kept for the transcript path
//! that reads it; the module is private to the crate.
#![allow(dead_code)]
//!
//! The panel that used to print these asked for a decision; it is gone with the
//! approval it was deciding, and what is left is the rendering, which the
//! transcript uses to show what a call is about to do.

/// One line of a tool preview, already split from the raw JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreviewLine {
    /// A named scalar: `path`, `cwd`, `pattern`.
    Field { key: String, value: String },
    /// A multi-line value shown in a gutter.
    Block { label: String, lines: Vec<String> },
    /// A quiet note about a flag or a bound.
    Note(String),
}

/// Renders a tool call as something a human can read at a glance.
///
/// A preview is never a JSON dump: for the common tools the useful parts are
/// the path, the command and a short excerpt of the text that will be written.
pub(crate) fn argument_preview(tool: &str, arguments: &serde_json::Value) -> Vec<PreviewLine> {
    let Some(object) = arguments.as_object() else {
        return vec![PreviewLine::Note(compact_json(arguments, 200))];
    };
    let lines = match tool {
        "shell" => shell_preview(object),
        "read" => read_preview(object),
        "write" => write_preview(object),
        "edit" => edit_preview(object),
        "grep" => path_tool_preview(object),
        "ask_user" => ask_preview(object),
        _ => generic_preview(object),
    };
    if lines.is_empty() {
        vec![PreviewLine::Note(compact_json(arguments, 200))]
    } else {
        lines
    }
}

pub(crate) fn shell_preview(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(script) = object.get("script").and_then(serde_json::Value::as_str) {
        lines.push(block("script", script, 14));
    }
    if let Some(shell) = object.get("shell").and_then(serde_json::Value::as_str) {
        lines.push(field("shell", shell));
    }
    if let Some(cwd) = object.get("cwd").and_then(serde_json::Value::as_str) {
        lines.push(field("cwd", cwd));
    }
    if let Some(env) = object.get("env").and_then(serde_json::Value::as_object) {
        if !env.is_empty() {
            lines.push(PreviewLine::Note(format!(
                "{} environment variable(s)",
                env.len()
            )));
        }
    }
    if let Some(stdin) = object.get("stdin").and_then(serde_json::Value::as_str) {
        lines.push(block("stdin", stdin, 6));
    }
    if let Some(timeout) = object.get("timeout_ms").and_then(serde_json::Value::as_u64) {
        lines.push(PreviewLine::Note(format!("timeout {timeout} ms")));
    }
    if object
        .get("merge_stderr")
        .and_then(serde_json::Value::as_bool)
        .is_some_and(|merge| merge)
    {
        lines.push(PreviewLine::Note("stderr is merged into stdout".to_owned()));
    }
    lines
}

pub(crate) fn read_preview(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(path) = object.get("path").and_then(serde_json::Value::as_str) {
        lines.push(field("path", path));
    }
    if let Some(range) = object.get("lines").and_then(serde_json::Value::as_object) {
        let start = range
            .get("start")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1);
        match range.get("end").and_then(serde_json::Value::as_u64) {
            Some(end) => lines.push(PreviewLine::Note(format!("lines {start}–{end}"))),
            None => lines.push(PreviewLine::Note(format!("lines {start} to the end"))),
        }
    }
    if let Some(tail) = object.get("tail").and_then(serde_json::Value::as_u64) {
        lines.push(PreviewLine::Note(format!("last {tail} line(s)")));
    }
    if let Some(pattern) = object.get("pattern").and_then(serde_json::Value::as_str) {
        lines.push(field("pattern", pattern));
    }
    if let Some(context) = object.get("context").and_then(serde_json::Value::as_u64) {
        lines.push(PreviewLine::Note(format!("{context} line(s) of context")));
    }
    for (key, label) in [
        ("ignore_case", "ignore case"),
        ("invert", "invert the match"),
        ("count_only", "count only"),
    ] {
        if object
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .is_some_and(|set| set)
        {
            lines.push(PreviewLine::Note(label.to_owned()));
        }
    }
    lines
}

pub(crate) fn write_preview(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(path) = object.get("path").and_then(serde_json::Value::as_str) {
        lines.push(field("path", path));
    }
    if object
        .get("overwrite")
        .and_then(serde_json::Value::as_bool)
        .is_some_and(|overwrite| overwrite)
    {
        lines.push(PreviewLine::Note(
            "replaces the file if it exists".to_owned(),
        ));
    }
    if let Some(content) = object.get("content").and_then(serde_json::Value::as_str) {
        lines.push(block("content", content, 12));
    }
    lines
}

pub(crate) fn edit_preview(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(path) = object.get("path").and_then(serde_json::Value::as_str) {
        lines.push(field("path", path));
    }
    if let Some(edits) = object.get("edits").and_then(serde_json::Value::as_array) {
        for (index, edit) in edits.iter().enumerate() {
            let Some(edit) = edit.as_object() else {
                continue;
            };
            let prefix = if edits.len() > 1 {
                format!("edit {} · ", index + 1)
            } else {
                String::new()
            };
            if let Some(search) = edit.get("search").and_then(serde_json::Value::as_str) {
                lines.push(block(&format!("{prefix}search"), search, 8));
            }
            if let Some(replace) = edit.get("replace").and_then(serde_json::Value::as_str) {
                lines.push(block(&format!("{prefix}replace"), replace, 8));
            }
        }
    }
    lines
}

pub(crate) fn path_tool_preview(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(path) = object.get("path").and_then(serde_json::Value::as_str) {
        lines.push(field("path", path));
    }
    if let Some(pattern) = object.get("pattern").and_then(serde_json::Value::as_str) {
        lines.push(field("pattern", pattern));
    }
    if let Some(include) = object.get("include").and_then(serde_json::Value::as_str) {
        lines.push(field("include", include));
    }
    if object
        .get("ignore_case")
        .and_then(serde_json::Value::as_bool)
        .is_some_and(|set| set)
    {
        lines.push(PreviewLine::Note("ignore case".to_owned()));
    }
    lines
}

pub(crate) fn ask_preview(object: &serde_json::Map<String, serde_json::Value>) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(question) = object.get("question").and_then(serde_json::Value::as_str) {
        lines.push(block("question", question, 12));
    }
    if let Some(options) = object.get("options").and_then(serde_json::Value::as_array) {
        let choices: Vec<&str> = options
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        if !choices.is_empty() {
            let multiple = object
                .get("multiple")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            lines.push(PreviewLine::Note(format!(
                "{} choice(s), {}",
                choices.len(),
                if multiple { "several allowed" } else { "one" }
            )));
            for (index, choice) in choices.iter().enumerate() {
                lines.push(field(&format!("{}.", index + 1), choice));
            }
        }
    }
    lines
}

pub(crate) fn generic_preview(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    for (key, value) in object.iter().take(10) {
        match value {
            serde_json::Value::String(text) if text.contains('\n') || text.len() > 120 => {
                lines.push(block(key, text, 8));
            }
            serde_json::Value::String(text) => lines.push(field(key, text)),
            serde_json::Value::Null => {}
            other => lines.push(field(key, &compact_json(other, 120))),
        }
    }
    if object.len() > 10 {
        lines.push(PreviewLine::Note(format!(
            "… {} more field(s)",
            object.len() - 10
        )));
    }
    lines
}

pub(crate) fn field(key: &str, value: &str) -> PreviewLine {
    PreviewLine::Field {
        key: crate::text::terminal(key).into_owned(),
        value: crate::text::terminal(value).into_owned(),
    }
}

pub(crate) fn block(label: &str, text: &str, max_lines: usize) -> PreviewLine {
    let mut lines: Vec<String> = text
        .lines()
        .take(max_lines)
        .map(|line| clip_line(line, 110))
        .collect();
    if text.lines().count() > max_lines {
        lines.push(format!(
            "… {} more line(s)",
            text.lines().count() - max_lines
        ));
    }
    PreviewLine::Block {
        label: crate::text::terminal(label).into_owned(),
        lines,
    }
}

pub(crate) fn compact_json(value: &serde_json::Value, max: usize) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    clip_line(&text, max)
}

pub(crate) fn clip_line(line: &str, max: usize) -> String {
    let line = crate::text::terminal(line);
    if line.chars().count() <= max {
        return line.into_owned();
    }
    let mut shortened: String = line.chars().take(max).collect();
    shortened.push('…');
    shortened
}

/// Shortens a multi-line block on a character boundary.
pub(crate) fn clip_text(text: &str, max: usize) -> (String, bool) {
    let text = crate::text::terminal(text);
    if text.chars().count() <= max {
        return (text.into_owned(), false);
    }
    let mut shortened: String = text.chars().take(max).collect();
    shortened.push_str("\n… [truncated]");
    (shortened, true)
}
