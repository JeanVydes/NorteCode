//! Interactive approval prompts, input handling, model selection and previews.

use super::{
    Arc, Budget, Catalog, ClientRequest, ClientRequestKind, Config, DenyScope, Duration,
    EventStream, GrantScope, ModelInfo, ModelSpec, ModelsArgs, Phase, Reasoning, Renderer,
    ServerEvent, ServerEventKind, Service, ServiceError, SessionKey, Source, StreamExt, Style,
    VecDeque, fallback_env, human_tokens, model_list, mpsc, reasoning_label, short, source_label,
};

/// Parses `/allow` and `/deny` into `(allow, tool, global)`.
///
/// The scope is optional and defaults to the current session. Anything that is
/// not one of these commands returns `None`, so an ordinary message that starts
/// with `/` is still sent to the model.
pub(crate) fn parse_approval_command(text: &str) -> Option<Result<(bool, String, bool), String>> {
    let (allow, rest) = text
        .strip_prefix("/allow")
        .map(|rest| (true, rest))
        .or_else(|| text.strip_prefix("/deny").map(|rest| (false, rest)))?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let command = if allow { "/allow" } else { "/deny" };
    let mut parts = rest.split_whitespace();
    let Some(tool) = parts.next() else {
        return Some(Err(format!("usage: {command} <tool> [session|global]")));
    };
    let scope = parts.next().unwrap_or("session");
    if parts.next().is_some() {
        return Some(Err(format!("usage: {command} <tool> [session|global]")));
    }
    let global = match scope {
        "session" => false,
        "global" => true,
        other => {
            return Some(Err(format!(
                "unknown scope `{other}`; use session or global"
            )));
        }
    };
    Some(Ok((allow, tool.to_owned(), global)))
}

pub(crate) fn print_chat_help(style: Style) {
    eprintln!("{}", style.bold("chat commands"));
    eprintln!("  /help                     show this list");
    eprintln!("  /compact                  summarise and continue on a new session");
    eprintln!("  /allow <tool> [session|global]  allow without asking");
    eprintln!("  /deny <tool> [session|global]   refuse without asking");
    eprintln!("  /refresh                  re-scan the skills and their tools");
    eprintln!("  /exit, /quit              leave the chat");
    eprintln!(
        "{}",
        style.dim("  scope defaults to session; global writes the user config")
    );
}

/// Reads and sends messages until the human exits, a turn parks or the budget
/// stops the session.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn interactive_loop(
    service: &Arc<Service>,
    mut session: Option<SessionKey>,
    renderer: &mut Renderer,
    style: Style,
    input: &mut mpsc::UnboundedReceiver<Option<String>>,
    message_budget: Option<Budget>,
    reasoning: Option<Reasoning>,
    mut pending: Option<String>,
) -> Result<Option<SessionKey>, String> {
    if !renderer.quiet {
        eprintln!(
            "{}",
            style.dim("interactive: /help for commands; /compact, /allow, /deny, /refresh, /exit")
        );
    }
    let mut request_id = 2_u64;
    loop {
        let next = if let Some(text) = pending.take() {
            Some(text)
        } else {
            read_repl_line(input, style).await?
        };
        let Some(text) = next else {
            break;
        };
        if matches!(text.as_str(), "/exit" | "/quit" | "exit" | "quit") {
            break;
        }
        if text == "/help" {
            print_chat_help(style);
            continue;
        }
        if text == "/compact" {
            let Some(current) = session.clone() else {
                eprintln!(
                    "{}",
                    style.yellow("there is no session to compact yet; send a message first")
                );
                continue;
            };
            let stream = service
                .submit(ClientRequest {
                    request_id,
                    kind: ClientRequestKind::Compact { session: current },
                })
                .await
                .map_err(|error| error.to_string())?;
            request_id = request_id.saturating_add(1);
            let outcome = drive_turns(service, stream, renderer, style, Some(&mut *input)).await?;
            renderer.ensure_newline();
            queue_after_turn(renderer, style, &mut pending, &outcome);
            session = outcome.session.or(session);
            if outcome.parked || renderer.budget_stopped {
                break;
            }
            continue;
        }
        if let Some(rule) = parse_approval_command(&text) {
            match rule {
                Ok((allow, tool, global)) => {
                    match service
                        .set_approval_rule(session.as_ref(), &tool, allow, global)
                        .await
                    {
                        Ok(()) => {
                            let verb = if allow { "allowed" } else { "denied" };
                            let scope = if global {
                                "globally"
                            } else {
                                "for this session"
                            };
                            eprintln!("{}", style.green(format!("{verb} `{tool}` {scope}")));
                        }
                        Err(error) => {
                            eprintln!("{}", style.red(format!("could not set the rule: {error}")));
                        }
                    }
                }
                Err(message) => eprintln!("{}", style.yellow(message)),
            }
            continue;
        }
        if text == "/refresh" {
            let count = service.refresh_skills();
            eprintln!("{}", style.green(format!("skills refreshed: {count}")));
            continue;
        }
        let stream = service
            .submit(ClientRequest {
                request_id,
                kind: ClientRequestKind::SendMessage {
                    session: session.clone(),
                    text,
                    budget: message_budget.clone(),
                    reasoning: reasoning.clone(),
                },
            })
            .await
            .map_err(|error| error.to_string())?;
        request_id = request_id.saturating_add(1);
        let outcome = drive_turns(service, stream, renderer, style, Some(&mut *input)).await?;
        renderer.ensure_newline();
        queue_after_turn(renderer, style, &mut pending, &outcome);
        session = outcome.session.or(session);
        if outcome.parked || renderer.budget_stopped {
            break;
        }
    }
    Ok(session)
}

/// Turns queued lines into the next message, unless the turn stopped.
fn queue_after_turn(
    renderer: &Renderer,
    style: Style,
    pending: &mut Option<String>,
    outcome: &DriveOutcome,
) {
    if outcome.parked || renderer.budget_stopped || outcome.queued.is_empty() {
        return;
    }
    if !renderer.quiet {
        eprintln!(
            "{}",
            style.blue(format!(
                "↳ sending {} queued message(s)",
                outcome.queued.len()
            ))
        );
    }
    *pending = Some(outcome.queued.join("\n"));
}

/// Prints the interactive prompt and waits for the next message.
///
/// Ctrl-C, Ctrl-D, `/exit` and `quit` all leave the loop without inventing a
/// message. Empty lines are ignored.
pub(crate) async fn read_repl_line(
    input: &mut mpsc::UnboundedReceiver<Option<String>>,
    style: Style,
) -> Result<Option<String>, String> {
    let _ = style;
    loop {
        let line = tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                eprintln!();
                return Ok(None);
            }
            line = input.recv() => line,
        };
        let Some(Some(line)) = line else {
            eprintln!();
            return Ok(None);
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line, "/exit" | "/quit" | "exit" | "quit") {
            return Ok(None);
        }
        return Ok(Some(line.to_owned()));
    }
}

#[derive(Debug, Default)]
pub(crate) struct DriveOutcome {
    /// The session seen on the stream, if the stream carried one.
    pub(crate) session: Option<SessionKey>,
    /// The turn stopped with an approval still waiting for a human.
    pub(crate) parked: bool,
    /// Lines typed while the turn was running, in arrival order.
    pub(crate) queued: Vec<String>,
}

/// What woke the turn loop: a signal, a completed input line, or an event.
enum Wake {
    /// Ctrl-C or another interrupt.
    Interrupt,
    /// A line completed in the input box.
    Line(String),
    /// End of input.
    Eof,
    /// The next server event.
    Event(Box<Option<ServerEvent>>),
}

/// Streams the primary turn, then any turns resumed by an approval.
///
/// A turn may park for several approvals; each decision resumes on its own
/// stream, which can park again, so the loop drains a queue of streams.
#[allow(clippy::too_many_lines)]
pub(crate) async fn drive_turns(
    service: &Arc<Service>,
    primary: EventStream,
    renderer: &mut Renderer,
    style: Style,
    mut input: Option<&mut mpsc::UnboundedReceiver<Option<String>>>,
) -> Result<DriveOutcome, String> {
    let mut streams: VecDeque<EventStream> = VecDeque::new();
    streams.push_back(primary);
    let mut pending: VecDeque<(SessionKey, String, ApprovalChoice)> = VecDeque::new();
    let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::channel::<()>(4);
    let signal = tokio::spawn(async move {
        loop {
            if tokio::signal::ctrl_c().await.is_err() || interrupt_tx.send(()).await.is_err() {
                break;
            }
        }
    });
    let mut current: Option<SessionKey> = None;
    let mut parked = false;
    let mut queued: Vec<String> = Vec::new();
    let mut cancel_requested: Option<std::time::Instant> = None;
    while let Some(mut stream) = streams.pop_front() {
        loop {
            let wake = if let Some(receiver) = input.as_mut() {
                tokio::select! {
                    biased;
                    Some(()) = interrupt_rx.recv() => Wake::Interrupt,
                    line = receiver.recv() => match line {
                        Some(Some(line)) => Wake::Line(line),
                        Some(None) | None => Wake::Eof,
                    },
                    event = stream.next() => Wake::Event(Box::new(event)),
                }
            } else {
                tokio::select! {
                    biased;
                    Some(()) = interrupt_rx.recv() => Wake::Interrupt,
                    event = stream.next() => Wake::Event(Box::new(event)),
                }
            };
            let event = match wake {
                Wake::Interrupt => {
                    let Some(session) = current.clone() else {
                        continue;
                    };
                    match cancel_requested {
                        None => {
                            cancel_requested = Some(std::time::Instant::now());
                            renderer.mark_cancelled();
                            eprintln!("{}", style.yellow("cancelling the running turn…"));
                            eprintln!(
                                "  {}",
                                style.dim(
                                    "the turn is saved and can be resumed; press Ctrl-C again \
                                     after 5s to force quit"
                                )
                            );
                            let request = ClientRequest {
                                request_id: 0,
                                kind: ClientRequestKind::CancelTurn { session },
                            };
                            if let Err(error) = service.submit(request).await {
                                eprintln!(
                                    "{}",
                                    style.yellow(format!(
                                        "warning: could not cancel the turn: {error}"
                                    ))
                                );
                            }
                        }
                        Some(at) if at.elapsed() < Duration::from_secs(5) => {
                            eprintln!("{}", style.yellow("still cancelling…"));
                            eprintln!(
                                "  {}",
                                style.dim(
                                    "a blocking tool may take a moment to stop; Ctrl-C again \
                                     after 5s will force quit"
                                )
                            );
                        }
                        Some(_) => {
                            eprintln!("{}", style.red("interrupted; forcing exit"));
                            signal.abort();
                            return Err("interrupted".to_owned());
                        }
                    }
                    continue;
                }
                Wake::Line(line) => {
                    let line = line.trim();
                    if !line.is_empty() {
                        queued.push(line.to_owned());
                    }
                    continue;
                }
                Wake::Eof => {
                    input = None;
                    continue;
                }
                Wake::Event(event) => *event,
            };
            let Some(event) = event else { break };
            if let Some(session) = &event.session {
                current = Some(session.clone());
            }
            match &event.kind {
                ServerEventKind::TurnStarted { .. } => parked = false,
                ServerEventKind::TurnFinished { phase, .. } => {
                    parked = *phase == Phase::AwaitingApproval;
                }
                _ => {}
            }
            match ask_approval(&event, style, input.as_deref_mut(), &mut interrupt_rx).await? {
                ApprovalOutcome::Decision(decision) => pending.push_back(decision),
                ApprovalOutcome::None => {
                    if matches!(&event.kind, ServerEventKind::ApprovalRequested { .. }) {
                        parked = true;
                    }
                }
                ApprovalOutcome::Cancelled => {
                    renderer.mark_cancelled();
                    eprintln!(
                        "{}",
                        style.yellow(
                            "approval left pending; the session is saved and can be resumed"
                        )
                    );
                    signal.abort();
                    return Ok(DriveOutcome {
                        session: current,
                        parked: true,
                        queued,
                    });
                }
            }
            if answer_if_asked(service, &event, input.as_deref_mut(), &mut interrupt_rx).await? {
                renderer.mark_cancelled();
                eprintln!(
                    "{}",
                    style.yellow("question left pending; the session is saved and can be resumed")
                );
                signal.abort();
                return Ok(DriveOutcome {
                    session: current,
                    parked: true,
                    queued,
                });
            }
            renderer.event(&event)?;
        }
        while let Some((session, approval, choice)) = pending.pop_front() {
            let request = approval_request(session, approval.clone(), choice);
            match service.submit(request).await {
                Ok(next) => streams.push_back(next),
                Err(error) => {
                    eprintln!(
                        "{}",
                        style.yellow(format!(
                            "warning: could not resolve approval {}: {error}",
                            short(&approval)
                        ))
                    );
                }
            }
        }
    }
    signal.abort();
    Ok(DriveOutcome {
        session: current,
        parked,
        queued,
    })
}

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
        key: key.to_owned(),
        value: value.to_owned(),
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
        label: label.to_owned(),
        lines,
    }
}

pub(crate) fn compact_json(value: &serde_json::Value, max: usize) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    clip_line(&text, max)
}

pub(crate) fn clip_line(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let mut shortened: String = line.chars().take(max).collect();
    shortened.push('…');
    shortened
}

/// Shortens a multi-line block on a character boundary.
pub(crate) fn clip_text(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_owned(), false);
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

/// Answers a `QuestionAsked` event, prompting on a terminal and leaving a note
/// when the client is not interactive.
///
/// With closed `options`, the prompt is a numbered list plus an automatic
/// "Something else" entry. Picking that entry (or `other`) asks for free-form
/// text, which travels through the same [`ClientRequestKind::AnswerQuestion`]
/// channel as a plain answer. A multi-select answer joins the chosen option
/// strings with `, `.
pub(crate) async fn answer_if_asked(
    service: &Arc<Service>,
    event: &ServerEvent,
    input: Option<&mut mpsc::UnboundedReceiver<Option<String>>>,
    interrupts: &mut mpsc::Receiver<()>,
) -> Result<bool, String> {
    let ServerEventKind::QuestionAsked {
        question,
        prompt,
        options,
        multiple,
        ..
    } = &event.kind
    else {
        return Ok(false);
    };
    let session = event
        .session
        .clone()
        .ok_or("a question arrived without a session")?;
    let question = question.clone();
    eprintln!("? {prompt}");
    let Some(input) = input else {
        eprintln!("(no interactive client; answering with a note)");
        let answer =
            "(no interactive client is attached; continue with your best judgment)".to_owned();
        return submit_answer(service, session, question, answer).await;
    };
    let answer = if options.is_empty() {
        match prompt_input("answer> ", &mut *input, interrupts).await? {
            PromptInput::Line(line) => line,
            PromptInput::Eof => {
                "(no interactive client is attached; continue with your best judgment)".to_owned()
            }
            PromptInput::Cancelled => return Ok(true),
        }
    } else {
        for (index, option) in options.iter().enumerate() {
            eprintln!("  {}. {option}", index + 1);
        }
        eprintln!("  {}. Something else (type it)", options.len() + 1);
        eprintln!(
            "  {}",
            if *multiple {
                "choose one or more numbers, separated by commas"
            } else {
                "choose a number, or type an option"
            }
        );
        loop {
            let line = match prompt_input("choice> ", &mut *input, interrupts).await? {
                PromptInput::Line(line) => line,
                PromptInput::Eof => {
                    break "(no interactive client is attached; continue with your best judgment)"
                        .to_owned();
                }
                PromptInput::Cancelled => return Ok(true),
            };
            let Some((selected, other)) = parse_selection(&line, options, *multiple) else {
                eprintln!("  choose a listed number");
                continue;
            };
            let extra = if other {
                match prompt_input("something else> ", &mut *input, interrupts).await? {
                    PromptInput::Line(text) => Some(text),
                    PromptInput::Eof => {
                        break "(no interactive client is attached; continue with your best judgment)"
                            .to_owned();
                    }
                    PromptInput::Cancelled => return Ok(true),
                }
            } else {
                None
            };
            if selected.is_empty() && extra.as_deref().is_none_or(str::is_empty) {
                eprintln!("  type an answer, or choose a listed number");
                continue;
            }
            break compose_selection(options, &selected, extra.as_deref());
        }
    };
    submit_answer(service, session, question, answer).await
}

/// Sends one answer and drains its acknowledgement.
pub(crate) async fn submit_answer(
    service: &Arc<Service>,
    session: SessionKey,
    question: String,
    answer: String,
) -> Result<bool, String> {
    let response = ClientRequest {
        request_id: 0,
        kind: ClientRequestKind::AnswerQuestion {
            session,
            question,
            answer,
        },
    };
    let mut acknowledgement = service.submit(response).await.map_err(|e| e.to_string())?;
    while acknowledgement.next().await.is_some() {}
    Ok(false)
}

/// The three outcomes of waiting for one line of input.
pub(crate) enum PromptInput {
    /// A line, with its trailing newline removed.
    Line(String),
    /// End of input, or the reader thread stopped.
    Eof,
    /// Ctrl-C arrived before a line did.
    Cancelled,
}

/// Prints a prompt and waits for one line or Ctrl-C.
pub(crate) async fn prompt_input(
    prompt: &str,
    input: &mut mpsc::UnboundedReceiver<Option<String>>,
    interrupts: &mut mpsc::Receiver<()>,
) -> Result<PromptInput, String> {
    if !prompt.is_empty() {
        eprintln!("{prompt}");
    }
    let line = tokio::select! {
        biased;
        Some(()) = interrupts.recv() => return Ok(PromptInput::Cancelled),
        line = input.recv() => line,
    };
    match line {
        Some(Some(line)) => Ok(PromptInput::Line(line)),
        Some(None) | None => Ok(PromptInput::Eof),
    }
}

/// Parses a numbered selection. Returns the zero-based option indexes and
/// whether the automatic "Something else" entry was chosen.
pub(crate) fn parse_selection(
    line: &str,
    options: &[String],
    multiple: bool,
) -> Option<(Vec<usize>, bool)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if !multiple {
        if line.eq_ignore_ascii_case("other") || line.eq_ignore_ascii_case("o") {
            return Some((Vec::new(), true));
        }
        if let Some(index) = options
            .iter()
            .position(|option| option.eq_ignore_ascii_case(line))
        {
            return Some((vec![index], false));
        }
    }
    let tokens: Vec<&str> = line
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() || (!multiple && tokens.len() != 1) {
        return None;
    }
    if tokens.len() == 1 && tokens[0].eq_ignore_ascii_case("all") {
        return multiple.then(|| ((0..options.len()).collect(), false));
    }
    let mut selected = Vec::new();
    let mut other = false;
    for token in tokens {
        if token.eq_ignore_ascii_case("other") || token.eq_ignore_ascii_case("o") {
            if !multiple && other {
                return None;
            }
            other = true;
        } else if let Ok(number) = token.parse::<usize>() {
            if number == 0 || number > options.len() + 1 {
                return None;
            }
            if number == options.len() + 1 {
                if !multiple && other {
                    return None;
                }
                other = true;
            } else if !selected.contains(&(number - 1)) {
                selected.push(number - 1);
            }
        } else {
            return None;
        }
    }
    if selected.is_empty() && !other {
        None
    } else {
        selected.sort_unstable();
        Some((selected, other))
    }
}

/// Renders a selection as the answer the model reads.
pub(crate) fn compose_selection(
    options: &[String],
    selected: &[usize],
    other: Option<&str>,
) -> String {
    let mut parts: Vec<String> = selected
        .iter()
        .filter_map(|index| options.get(*index).cloned())
        .collect();
    if let Some(text) = other.filter(|text| !text.is_empty()) {
        parts.push(text.to_owned());
    }
    parts.join(", ")
}

pub(crate) fn list_models(
    config: &Config,
    catalog: &Catalog,
    source: Source,
    args: &ModelsArgs,
) -> Result<u8, String> {
    let mut models = model_list(config, catalog);
    models.retain(|model| {
        args.provider
            .as_ref()
            .is_none_or(|provider| &model.provider == provider)
    });
    if args.tools {
        models.retain(|model| model.tools);
    }
    if args.reasoning {
        models.retain(|model| model.reasoning.supported);
    }
    models.sort_by(|a, b| {
        (a.provider.as_str(), a.model.as_str()).cmp(&(b.provider.as_str(), b.model.as_str()))
    });

    if args.json {
        for model in &models {
            let line = serde_json::to_string(model).map_err(|error| error.to_string())?;
            println!("{line}");
        }
    } else {
        eprintln!(
            "catalogue: {} models from {}",
            models.len(),
            source_label(source)
        );
        for model in &models {
            print_model(model);
        }
    }
    Ok(0)
}

pub(crate) fn print_model(model: &ModelInfo) {
    let reference = format!("{}/{}", model.provider, model.model);
    let context = model
        .max_context
        .map_or_else(|| "-".to_owned(), human_tokens);
    let tools = if model.tools { "tools" } else { "-" };
    let reasoning = reasoning_label(&model.reasoning);
    let name = if model.deprecated {
        format!("{} (deprecated)", model.name)
    } else {
        model.name.clone()
    };
    println!("{reference:<54} {context:>8}  {tools:<5}  {reasoning:<18} {name}");
}

pub(crate) fn resolve_spec(
    config: &Config,
    catalog: &Catalog,
    model: Option<String>,
    endpoint: Option<String>,
) -> Result<ModelSpec, String> {
    let reference = model
        .or_else(|| config.default_model.clone())
        .ok_or_else(|| {
            "no model selected: pass `--model provider/model` or set `default_model` in the config"
                .to_owned()
        })?;
    match ModelSpec::from_catalog(config, catalog, &reference, endpoint.clone()) {
        Ok(spec) => Ok(spec),
        Err(ServiceError::UnknownModel { .. }) => {
            eprintln!("note: `{reference}` is not in the catalogue; using its provider's facts");
            ModelSpec::from_provider(config, catalog, &reference, endpoint)
                .map_err(|error| error.to_string())
        }
        Err(ServiceError::UnknownProvider(_)) => {
            eprintln!(
                "note: `{reference}` names a provider the catalogue does not list; using the fallback"
            );
            let provider = reference
                .split_once('/')
                .map_or("", |(provider, _)| provider);
            ModelSpec::resolve(config, &reference, endpoint, &fallback_env(provider))
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}
