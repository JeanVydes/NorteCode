//! Approval prompt: tool previews, decisions and their protocol requests.

use crate::prompt::{PromptInput, prompt_input};

use super::{
    ClientRequest, ClientRequestKind, DenyScope, GrantScope, ServerEvent, ServerEventKind,
    SessionKey, Style, mpsc, short,
};

/// What the human chose for a parked call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ApprovalChoice {
    /// Run this call once.
    Once,
    /// Run any call to this tool for the rest of the session.
    ToolSession,
    /// Run this tool in every session, stored in the user config.
    ToolGlobal,
    /// Refuse this call, with the reason the human typed, if any.
    DenyOnce(Option<String>),
    /// Refuse this tool for the rest of the session.
    DenySession(Option<String>),
    /// Refuse this tool in every session, stored in the user config.
    DenyGlobal(Option<String>),
}

/// What happened at an approval panel.
pub(crate) enum ApprovalOutcome {
    /// A decision to submit once the stream is drained.
    Decision((SessionKey, String, ApprovalChoice)),
    /// No decision: no interactive client, or the event was not an approval.
    None,
    /// The human pressed Ctrl-C while the prompt waited for input.
    Cancelled,
}

/// Prints an approval request and asks for a decision when stdin is a terminal.
///
/// Input is read by a background thread and selected against the Ctrl-C
/// channel, so a parked approval cannot swallow the interrupt. A cancelled
/// prompt leaves the decision pending and the session resumable.
pub(crate) async fn ask_approval(
    event: &ServerEvent,
    style: Style,
    input: Option<&mut mpsc::UnboundedReceiver<Option<String>>>,
    interrupts: &mut mpsc::Receiver<()>,
) -> Result<ApprovalOutcome, String> {
    let ServerEventKind::ApprovalRequested {
        approval,
        tool,
        call_id,
        arguments,
        sandbox,
        ..
    } = &event.kind
    else {
        return Ok(ApprovalOutcome::None);
    };
    let Some(session) = event.session.clone() else {
        return Ok(ApprovalOutcome::None);
    };
    eprintln!();
    let rule = style.yellow("─".repeat(rule_width()));
    eprintln!("{rule}");
    eprintln!(
        "{}  {}  {}",
        style.yellow("approval required"),
        style.tool(tool),
        style.dim(format!("#{}", short(call_id)))
    );
    if let Some(sandbox) = sandbox {
        eprintln!("  {}", style.green(format!("[{sandbox}] sandboxed")));
    }
    if let Some(arguments) = arguments {
        for line in argument_preview(tool, arguments) {
            match line {
                PreviewLine::Field { key, value } => {
                    eprintln!("  {} {}", style.dim(format!("{key:<10}")), value);
                }
                PreviewLine::Block { label, lines } => {
                    eprintln!("  {}", style.dim(label));
                    for line in lines {
                        eprintln!("  {} {}", style.cyan("│"), line);
                    }
                }
                PreviewLine::Note(note) => {
                    eprintln!("  {}", style.dim(note));
                }
            }
        }
    }
    eprintln!("{rule}");
    let Some(input) = input else {
        eprintln!(
            "  {}",
            style.dim(
                "not an interactive terminal; the session stays parked. Use --approval-mode \
                 auto-edit or yolo to run without asking."
            )
        );
        return Ok(ApprovalOutcome::None);
    };
    loop {
        eprintln!(
            "  {}  {}  {}",
            style.green("[y] allow once"),
            style.cyan("[s] allow this tool for this session"),
            style.cyan("[a] allow this tool always")
        );
        eprintln!(
            "  {}  {}  {}",
            style.red("[n] deny once"),
            style.red("[ns] deny this tool for this session"),
            style.red("[ng] deny this tool always")
        );
        match prompt_input("", &mut *input, interrupts).await? {
            PromptInput::Line(line) => {
                if let Some(choice) = parse_approval_choice(&line) {
                    return Ok(ApprovalOutcome::Decision((
                        session,
                        approval.clone(),
                        choice,
                    )));
                }
            }
            PromptInput::Eof => {
                return Ok(ApprovalOutcome::Decision((
                    session,
                    approval.clone(),
                    ApprovalChoice::DenyOnce(None),
                )));
            }
            PromptInput::Cancelled => return Ok(ApprovalOutcome::Cancelled),
        }
        eprintln!(
            "  {}",
            style.dim("answer y, s, a, n, ns or ng; add a message after n to tell the model why")
        );
    }
}

/// Parses one line of the approval prompt. `None` means ask again.
///
/// `n <message>` is a denial that reaches the model, so "no, use the existing
/// helper" is actionable feedback rather than a bare refusal. `ns` and `ng`
/// make the denial last for the session or for every session; the same suffixes
/// on `s` and `a` pick the matching allow scope.
pub(crate) fn parse_approval_choice(line: &str) -> Option<ApprovalChoice> {
    let line = line.trim();
    let (command, reason) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(command, reason)| (command, reason.trim()));
    let reason = (!reason.is_empty()).then(|| reason.to_owned());
    match command.to_ascii_lowercase().as_str() {
        "y" | "yes" | "1" if reason.is_none() => Some(ApprovalChoice::Once),
        "s" | "session" | "2" if reason.is_none() => Some(ApprovalChoice::ToolSession),
        "a" | "always" | "g" | "3" if reason.is_none() => Some(ApprovalChoice::ToolGlobal),
        "n" | "no" | "4" if reason.is_none() => Some(ApprovalChoice::DenyOnce(None)),
        "n" | "no" => Some(ApprovalChoice::DenyOnce(reason)),
        "ns" | "5" => Some(ApprovalChoice::DenySession(reason)),
        "ng" | "6" => Some(ApprovalChoice::DenyGlobal(reason)),
        "" => Some(ApprovalChoice::DenyOnce(None)),
        _ => None,
    }
}

/// One line in the approval panel, already split from the raw JSON.
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
/// The approval prompt must never be a JSON dump: for the common tools the
/// useful parts are the path, the command and a short excerpt of the text that
/// will be written.
pub(crate) fn argument_preview(tool: &str, arguments: &serde_json::Value) -> Vec<PreviewLine> {
    let Some(object) = arguments.as_object() else {
        return vec![PreviewLine::Note(compact_json(arguments, 200))];
    };
    let lines = match tool {
        "command" => command_preview(object),
        "read" => read_preview(object),
        "write" => write_preview(object),
        "edit" => edit_preview(object),
        "glob" | "grep" | "list_dir" => path_tool_preview(tool, object),
        "ask_user" => ask_preview(object),
        _ => generic_preview(object),
    };
    if lines.is_empty() {
        vec![PreviewLine::Note(compact_json(arguments, 200))]
    } else {
        lines
    }
}

pub(crate) fn command_preview(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<PreviewLine> {
    let mut lines = Vec::new();
    if let Some(command) = object.get("command").and_then(serde_json::Value::as_object) {
        match command.get("kind").and_then(serde_json::Value::as_str) {
            Some("shell") => {
                if let Some(shell) = command.get("shell").and_then(serde_json::Value::as_str) {
                    lines.push(field("shell", shell));
                }
                if let Some(script) = command.get("script").and_then(serde_json::Value::as_str) {
                    lines.push(block("script", script, 14));
                }
            }
            Some("program") => {
                if let Some(program) = command.get("program").and_then(serde_json::Value::as_str) {
                    lines.push(field("program", program));
                }
                if let Some(args) = command.get("args").and_then(serde_json::Value::as_array) {
                    let args: Vec<&str> =
                        args.iter().filter_map(serde_json::Value::as_str).collect();
                    if !args.is_empty() {
                        let joined = args.join(" ");
                        lines.push(field("args", &joined));
                    }
                }
            }
            _ => {}
        }
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
    tool: &str,
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
    if tool == "list_dir" && lines.is_empty() {
        lines.push(PreviewLine::Note("the workspace root".to_owned()));
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

pub(crate) fn rule_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|columns| columns.parse::<usize>().ok())
        .map_or(72, |columns| columns.saturating_sub(2).clamp(40, 100))
}

/// The protocol request that resolves a parked approval.
pub(crate) fn approval_request(
    session: SessionKey,
    approval: String,
    choice: ApprovalChoice,
) -> ClientRequest {
    let kind = match choice {
        ApprovalChoice::Once => ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: None,
            remember: None,
        },
        ApprovalChoice::ToolSession => ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: Some(GrantScope::Tool),
            remember: Some(true),
        },
        ApprovalChoice::ToolGlobal => ClientRequestKind::Approve {
            session,
            approval,
            arguments: None,
            scope: Some(GrantScope::Always),
            remember: Some(true),
        },
        ApprovalChoice::DenyOnce(reason) => ClientRequestKind::Deny {
            session,
            approval,
            reason,
            scope: Some(DenyScope::Once),
        },
        ApprovalChoice::DenySession(reason) => ClientRequestKind::Deny {
            session,
            approval,
            reason,
            scope: Some(DenyScope::ToolSession),
        },
        ApprovalChoice::DenyGlobal(reason) => ClientRequestKind::Deny {
            session,
            approval,
            reason,
            scope: Some(DenyScope::ToolGlobal),
        },
    };
    ClientRequest {
        request_id: 0,
        kind,
    }
}
