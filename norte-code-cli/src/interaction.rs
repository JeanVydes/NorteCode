//! The interactive turn loop: input, steering, approval handoff and
//! cancellation.

use super::{
    Arc, Budget, BudgetAction, BudgetUnit, ClientRequest, ClientRequestKind, EventStream,
    GrantInfo, GrantScope, Phase, Reasoning, ReasoningScope, Renderer, ServerEvent,
    ServerEventKind, Service, SessionKey, StreamExt, Style, VecDeque, mpsc, short,
};
use crate::approval::{ApprovalChoice, ApprovalOutcome, approval_request, ask_approval};
use crate::prompt::{PromptInput, answer_if_asked, prompt_input};
use crate::render::{
    BudgetPrompt, format_measure, parse_reasoning_setting, reasoning_setting_label, scope_name,
    unit_name,
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

/// A validated interactive command.
///
/// Commands with arguments parse into closed variants. Anything that is not a
/// known command stays a normal message, so a prompt may still start with `/`.
#[derive(Debug, PartialEq)]
pub(crate) enum ChatCommand {
    /// Show the command list.
    Help,
    /// Summarise the session on demand.
    Compact,
    /// Re-scan the skill roots.
    Refresh,
    /// Leave the chat.
    Exit,
    /// Change an approval rule.
    Approval {
        allow: bool,
        tool: String,
        global: bool,
    },
    /// Set a reasoning override.
    Reasoning {
        scope: ReasoningScope,
        reasoning: Reasoning,
    },
    /// Revoke one grant, or list the grants when omitted.
    Revoke { grant: Option<String> },
    /// Request a model switch.
    Model { model: String },
}

/// Parses one interactive command.
///
/// `None` means the text is an ordinary message. `Some(Err(_))` means the
/// command was recognised but its arguments were rejected; it must not be sent
/// to the model as a message.
pub(crate) fn parse_chat_command(text: &str) -> Option<Result<ChatCommand, String>> {
    let text = text.trim();
    let exact = match text {
        "/help" => Some(ChatCommand::Help),
        "/compact" => Some(ChatCommand::Compact),
        "/refresh" => Some(ChatCommand::Refresh),
        "/exit" | "/quit" | "exit" | "quit" => Some(ChatCommand::Exit),
        _ => None,
    };
    if let Some(command) = exact {
        return Some(Ok(command));
    }
    if let Some(rule) = parse_approval_command(text) {
        return Some(rule.map(|(allow, tool, global)| ChatCommand::Approval {
            allow,
            tool,
            global,
        }));
    }
    if let Some(rest) = command_rest(text, "/reasoning") {
        return Some(parse_reasoning_command(rest));
    }
    if command_rest(text, "/steer").is_some() {
        return Some(Err(
            "/steer was removed; type the text directly while a turn is running".to_owned(),
        ));
    }
    if let Some(rest) = command_rest(text, "/revoke") {
        let mut parts = rest.split_whitespace();
        let grant = parts.next().map(str::to_owned);
        if parts.next().is_some() {
            return Some(Err("usage: /revoke [grant-id-or-tool]".to_owned()));
        }
        return Some(Ok(ChatCommand::Revoke { grant }));
    }
    if let Some(rest) = command_rest(text, "/model") {
        return Some(parse_model_command(rest));
    }
    None
}

/// Returns the argument text after an exact command name.
fn command_rest<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(command)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some(rest)
}

fn parse_reasoning_command(rest: &str) -> Result<ChatCommand, String> {
    const USAGE: &str = "usage: /reasoning <setting> [session|message]";
    let parts: Vec<&str> = rest.split_whitespace().collect();
    let (scope, setting) = match parts.as_slice() {
        [setting] => (ReasoningScope::Session, *setting),
        [first, second] if is_reasoning_scope(first) => (scope(first), *second),
        [setting, second] if is_reasoning_scope(second) => (scope(second), *setting),
        _ => return Err(USAGE.to_owned()),
    };
    let reasoning = parse_reasoning_setting(setting)?;
    Ok(ChatCommand::Reasoning { scope, reasoning })
}

fn is_reasoning_scope(value: &str) -> bool {
    matches!(value, "session" | "message")
}

fn scope(value: &str) -> ReasoningScope {
    match value {
        "message" => ReasoningScope::Message,
        _ => ReasoningScope::Session,
    }
}

fn parse_model_command(rest: &str) -> Result<ChatCommand, String> {
    const USAGE: &str = "usage: /model <provider/model>";
    let parts: Vec<&str> = rest.split_whitespace().collect();
    let [model] = parts.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let Some((provider, name)) = model.split_once('/') else {
        return Err(USAGE.to_owned());
    };
    if provider.is_empty() || name.is_empty() {
        return Err(USAGE.to_owned());
    }
    Ok(ChatCommand::Model {
        model: (*model).to_owned(),
    })
}

/// One dimmed completion shown by the input editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandSuggestion {
    /// The text Tab inserts at the cursor.
    pub(crate) completion: String,
    /// The dim hint shown in the footer row above the input.
    pub(crate) hint: String,
}

const CHAT_COMMANDS: &[&str] = &[
    "/help",
    "/compact",
    "/allow",
    "/deny",
    "/reasoning",
    "/model",
    "/revoke",
    "/refresh",
    "/exit",
    "/quit",
];

const REASONING_SUGGESTIONS: &[&str] = &[
    "auto", "off", "minimal", "low", "medium", "high", "xhigh", "max",
];

const REASONING_SCOPES: &[&str] = &["session", "message"];
const TOOL_SCOPES: &[&str] = &["session", "global"];

/// Suggests a command name or a command argument for the current input line.
///
/// The completion is only offered at the end of the line; moving the cursor
/// into the middle never rewrites what the human is editing.
pub(crate) fn command_suggestion(text: &str, cursor: usize) -> Option<CommandSuggestion> {
    if cursor != text.len() || !text.starts_with('/') {
        return None;
    }
    let Some(space) = text.find(char::is_whitespace) else {
        let prefix = text.to_ascii_lowercase();
        let matches: Vec<&str> = CHAT_COMMANDS
            .iter()
            .copied()
            .filter(|command| command.starts_with(&prefix))
            .collect();
        if matches.is_empty() {
            return None;
        }
        let completion = matches[0][text.len()..].to_owned();
        let hint = if matches.len() == 1 {
            command_usage(matches[0]).unwrap_or(matches[0]).to_owned()
        } else {
            matches.join("  ")
        };
        return Some(CommandSuggestion { completion, hint });
    };
    let command = &text[..space];
    let usage = command_usage(command)?;
    let rest = &text[space..];
    let ends_with_space = rest.ends_with(char::is_whitespace);
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let completed = if ends_with_space {
        tokens.len()
    } else {
        tokens.len().saturating_sub(1)
    };
    let partial = if ends_with_space {
        ""
    } else {
        tokens.last().copied().unwrap_or("")
    };
    let Some(candidates) = argument_candidates(command, completed) else {
        return Some(CommandSuggestion {
            completion: String::new(),
            hint: usage.to_owned(),
        });
    };
    if command == "/reasoning" && completed == 0 && partial.starts_with("budget:") {
        return Some(CommandSuggestion {
            completion: String::new(),
            hint: "budget:<tokens>".to_owned(),
        });
    }
    if candidates.is_empty() {
        return Some(CommandSuggestion {
            completion: String::new(),
            hint: usage.to_owned(),
        });
    }
    let completion = candidate_completion(candidates, partial);
    let mut hint = candidates.join("  ");
    if command == "/reasoning" && completed == 0 {
        hint.push_str("  budget:<tokens>");
    }
    Some(CommandSuggestion {
        completion: completion.unwrap_or_default(),
        hint,
    })
}

fn command_usage(command: &str) -> Option<&'static str> {
    match command {
        "/help" => Some("/help"),
        "/compact" => Some("/compact"),
        "/allow" => Some("/allow <tool> [session|global]"),
        "/deny" => Some("/deny <tool> [session|global]"),
        "/reasoning" => Some("/reasoning <setting> [session|message]"),
        "/model" => Some("/model <provider/model>"),
        "/revoke" => Some("/revoke [grant-id-or-tool]"),
        "/refresh" => Some("/refresh"),
        "/exit" => Some("/exit"),
        "/quit" => Some("/quit"),
        _ => None,
    }
}

fn argument_candidates(command: &str, completed: usize) -> Option<&'static [&'static str]> {
    match command {
        "/reasoning" => match completed {
            0 => Some(REASONING_SUGGESTIONS),
            1 => Some(REASONING_SCOPES),
            _ => Some(&[]),
        },
        "/allow" | "/deny" => match completed {
            0 => None,
            1 => Some(TOOL_SCOPES),
            _ => Some(&[]),
        },
        _ => None,
    }
}

fn candidate_completion(candidates: &[&str], partial: &str) -> Option<String> {
    let partial = partial.to_ascii_lowercase();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.to_ascii_lowercase().starts_with(&partial))?;
    Some(candidate[partial.len()..].to_owned())
}

pub(crate) fn print_chat_help(style: Style) {
    eprintln!("{}", style.bold("chat commands"));
    eprintln!("  /help                     show this list");
    eprintln!("  /compact                  summarise and continue on a new session");
    eprintln!("  /allow <tool> [session|global]  allow without asking");
    eprintln!("  /deny <tool> [session|global]   refuse without asking");
    eprintln!("  /reasoning <setting> [session|message]  set reasoning");
    eprintln!("  /model <provider/model>   switch at the next turn boundary");
    eprintln!("  /revoke [grant-or-tool]   list or revoke an approval grant");
    eprintln!("  /refresh                  re-scan the skills and their tools");
    eprintln!("  /exit, /quit              leave the chat");
    eprintln!(
        "{}",
        style.dim(
            "  reasoning: auto, off, minimal, low, medium, high, xhigh, max or budget:<tokens>"
        )
    );
    eprintln!(
        "{}",
        style.dim("  text typed while a turn runs is steered into the next turn")
    );
    eprintln!(
        "{}",
        style.dim("  scope defaults to session; global writes the user config")
    );
}

/// Echoes a message the human sent into the transcript, above the footer.
pub(crate) fn echo_user_message(style: Style, text: &str) {
    let mut lines = text.lines();
    match lines.next() {
        Some(first) => eprintln!("{} {}", style.cyan("›"), first),
        None => eprintln!("{}", style.cyan("›")),
    }
    for line in lines {
        eprintln!("  {}", crate::text::terminal(line));
    }
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
) -> Result<Option<SessionKey>, String> {
    if !renderer.quiet {
        eprintln!(
            "{}",
            style.dim("interactive: /help for commands; /compact, /allow, /deny, /refresh, /exit")
        );
    }
    let mut request_id = 2_u64;
    loop {
        let Some(text) = read_repl_line(input, style).await? else {
            break;
        };
        let command = match parse_chat_command(&text) {
            None => None,
            Some(Ok(command)) => Some(command),
            Some(Err(message)) => {
                eprintln!("{}", style.yellow(message));
                continue;
            }
        };
        if let Some(command) = command {
            match command {
                ChatCommand::Exit => break,
                ChatCommand::Help => print_chat_help(style),
                ChatCommand::Compact => {
                    let Some(current) = session.clone() else {
                        eprintln!(
                            "{}",
                            style
                                .yellow("there is no session to compact yet; send a message first")
                        );
                        continue;
                    };
                    renderer.mark_working();
                    let stream = service
                        .submit(ClientRequest {
                            request_id,
                            kind: ClientRequestKind::Compact { session: current },
                        })
                        .await
                        .map_err(|error| error.to_string())?;
                    request_id = request_id.saturating_add(1);
                    let outcome =
                        drive_turns(service, stream, renderer, style, Some(&mut *input)).await?;
                    renderer.ensure_newline();
                    session = outcome.session.or(session);
                    if outcome.parked || renderer.budget_stopped {
                        break;
                    }
                }
                ChatCommand::Refresh => {
                    let count = service.refresh_skills();
                    eprintln!("{}", style.green(format!("skills refreshed: {count}")));
                }
                ChatCommand::Approval {
                    allow,
                    tool,
                    global,
                } => match service
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
                },
                ChatCommand::Reasoning { scope, reasoning } => {
                    if let Some(current) = session.clone() {
                        run_reasoning(service, current, scope, reasoning, style).await;
                    } else {
                        eprintln!(
                            "{}",
                            style.yellow("there is no session yet; send a message first")
                        );
                    }
                }
                ChatCommand::Model { model } => {
                    if let Some(current) = session.clone() {
                        run_model(service, current, model, style).await;
                    } else {
                        eprintln!(
                            "{}",
                            style.yellow("there is no session yet; send a message first")
                        );
                    }
                }
                ChatCommand::Revoke { grant } => {
                    if let Some(current) = session.clone() {
                        run_revoke(service, current, grant, style).await;
                    } else {
                        eprintln!(
                            "{}",
                            style.yellow("there is no session yet; send a message first")
                        );
                    }
                }
            }
            continue;
        }
        echo_user_message(style, &text);
        renderer.mark_working();
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
        session = outcome.session.or(session);
        if outcome.parked || renderer.budget_stopped {
            break;
        }
    }
    Ok(session)
}

async fn run_reasoning(
    service: &Arc<Service>,
    session: SessionKey,
    scope: ReasoningScope,
    reasoning: Reasoning,
    style: Style,
) {
    let kind = ClientRequestKind::SetReasoning {
        session,
        scope,
        reasoning: reasoning.clone(),
    };
    match drain_request(service, kind).await {
        Ok(events) => {
            if let Some(message) = stream_error(&events) {
                eprintln!(
                    "{}",
                    style.red(format!("could not set reasoning: {message}"))
                );
            } else {
                let scope = match scope {
                    ReasoningScope::Session => "for this session",
                    ReasoningScope::Message => "for the next message",
                };
                eprintln!(
                    "{}",
                    style.green(format!(
                        "reasoning set to {} {scope}",
                        reasoning_setting_label(&reasoning)
                    ))
                );
            }
        }
        Err(error) => eprintln!("{}", style.red(format!("could not set reasoning: {error}"))),
    }
}

async fn steer_text(
    service: &Arc<Service>,
    session: SessionKey,
    text: String,
    style: Style,
    quiet: bool,
) {
    match drain_request(service, ClientRequestKind::Steer { session, text }).await {
        Ok(events) => match stream_error(&events) {
            Some(message) => eprintln!("{}", style.red(format!("could not steer: {message}"))),
            None if !quiet => eprintln!("{}", style.dim("steered into the next turn boundary")),
            None => {}
        },
        Err(error) => eprintln!("{}", style.red(format!("could not steer: {error}"))),
    }
}

async fn handle_live_command(
    service: &Arc<Service>,
    command: ChatCommand,
    current: Option<&SessionKey>,
    style: Style,
) {
    match command {
        ChatCommand::Exit => eprintln!(
            "{}",
            style.yellow("wait until the current turn finishes, then /exit")
        ),
        ChatCommand::Help => print_chat_help(style),
        ChatCommand::Compact => eprintln!(
            "{}",
            style.yellow("wait until the current turn finishes, then /compact")
        ),
        ChatCommand::Refresh => {
            let count = service.refresh_skills();
            eprintln!("{}", style.green(format!("skills refreshed: {count}")));
        }
        ChatCommand::Approval {
            allow,
            tool,
            global,
        } => match service
            .set_approval_rule(current, &tool, allow, global)
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
            Err(error) => eprintln!("{}", style.red(format!("could not set the rule: {error}"))),
        },
        ChatCommand::Reasoning { scope, reasoning } => {
            if let Some(session) = current {
                run_reasoning(service, session.clone(), scope, reasoning, style).await;
            } else {
                eprintln!(
                    "{}",
                    style.yellow("the session is not ready yet; try again")
                );
            }
        }
        ChatCommand::Model { model } => {
            if let Some(session) = current {
                run_model(service, session.clone(), model, style).await;
            } else {
                eprintln!(
                    "{}",
                    style.yellow("the session is not ready yet; try again")
                );
            }
        }
        ChatCommand::Revoke { grant } => {
            if let Some(session) = current {
                run_revoke(service, session.clone(), grant, style).await;
            } else {
                eprintln!(
                    "{}",
                    style.yellow("the session is not ready yet; try again")
                );
            }
        }
    }
}

async fn prompt_budget_limit(
    input: &mut mpsc::UnboundedReceiver<Option<String>>,
    interrupts: &mut mpsc::Receiver<()>,
    style: Style,
    prompt: BudgetPrompt,
) -> Result<Option<f64>, String> {
    eprintln!(
        "{}",
        style.yellow(format!(
            "budget exhausted for the {} ({} of {} {}); enter a new limit, or leave it empty to stop",
            scope_name(prompt.scope),
            format_measure(prompt.unit, prompt.spent),
            format_measure(prompt.unit, prompt.limit),
            unit_name(prompt.unit)
        ))
    );
    loop {
        match prompt_input("new limit> ", input, interrupts).await? {
            PromptInput::Line(line) => {
                let line = line.trim();
                if line.is_empty()
                    || matches!(line.to_ascii_lowercase().as_str(), "n" | "no" | "stop")
                {
                    return Ok(None);
                }
                match parse_budget_limit(prompt.unit, line) {
                    Ok(limit) => return Ok(Some(limit)),
                    Err(error) => eprintln!("{}", style.yellow(error)),
                }
            }
            PromptInput::Eof | PromptInput::Cancelled => return Ok(None),
        }
    }
}

fn parse_budget_limit(unit: BudgetUnit, value: &str) -> Result<f64, String> {
    let mut text = value.trim().replace('_', "");
    if matches!(unit, BudgetUnit::Usd) {
        text = text.trim_start_matches('$').to_owned();
    }
    let limit = text
        .parse::<f64>()
        .map_err(|_| format!("`{value}` is not a number"))?;
    if !limit.is_finite() || limit <= 0.0 {
        return Err("the new limit must be greater than zero".to_owned());
    }
    if matches!(unit, BudgetUnit::Tokens | BudgetUnit::Requests) && limit.fract() != 0.0 {
        return Err(format!("{} are counted in whole numbers", unit_name(unit)));
    }
    Ok(limit)
}

async fn run_model(service: &Arc<Service>, session: SessionKey, model: String, style: Style) {
    match drain_request(service, ClientRequestKind::SetModel { session, model }).await {
        Ok(events) => {
            let mut reported = false;
            for event in events {
                match event.kind {
                    ServerEventKind::ModelChanged { model } => {
                        reported = true;
                        eprintln!("{}", style.green(format!("model set to {model}")));
                    }
                    ServerEventKind::ModelQueued { model } => {
                        reported = true;
                        eprintln!(
                            "{}",
                            style.yellow(format!(
                                "model switch to {model} queued for the next turn boundary"
                            ))
                        );
                    }
                    ServerEventKind::Error { message, .. } => {
                        reported = true;
                        eprintln!(
                            "{}",
                            style.red(format!("could not switch model: {message}"))
                        );
                    }
                    _ => {}
                }
            }
            if !reported {
                eprintln!("{}", style.yellow("the model switch was not acknowledged"));
            }
        }
        Err(error) => eprintln!("{}", style.red(format!("could not switch model: {error}"))),
    }
}

async fn run_revoke(
    service: &Arc<Service>,
    session: SessionKey,
    grant: Option<String>,
    style: Style,
) {
    let grants = match fetch_grants(service, session.clone()).await {
        Ok(grants) => grants,
        Err(error) => {
            eprintln!("{}", style.red(format!("could not list grants: {error}")));
            return;
        }
    };
    let Some(choice) = grant else {
        if grants.is_empty() {
            eprintln!("{}", style.dim("no approval grants"));
            return;
        }
        eprintln!("{}", style.bold("approval grants"));
        for grant in grants {
            eprintln!(
                "  {}{}  tool {}  {}  approval {}  issued {}",
                crate::text::terminal(&grant.grant),
                if grant.revoked { " (revoked)" } else { "" },
                crate::text::terminal(&grant.tool),
                grant_scope_label(grant.scope),
                crate::text::terminal(&grant.approval),
                crate::text::terminal(&grant.issued_at),
            );
        }
        return;
    };
    let matching: Vec<_> = grants
        .iter()
        .filter(|grant| !grant.revoked && (grant.grant == choice || grant.tool == choice))
        .collect();
    let found = match matching.as_slice() {
        [grant] => *grant,
        [] => {
            eprintln!(
                "{}",
                style.yellow(format!(
                    "no active grant matches `{choice}`; use /revoke to list grants"
                ))
            );
            return;
        }
        _ => {
            eprintln!(
                "{}",
                style.yellow(format!(
                    "`{choice}` matches more than one grant; use the full grant id"
                ))
            );
            return;
        }
    };
    let approval = found.approval.clone();
    let grant = found.grant.clone();
    match drain_request(
        service,
        ClientRequestKind::Revoke {
            session,
            approval,
            grant: grant.clone(),
        },
    )
    .await
    {
        Ok(events) => match stream_error(&events) {
            Some(message) => eprintln!("{}", style.red(format!("could not revoke: {message}"))),
            None => eprintln!("{}", style.green(format!("grant {grant} revoked"))),
        },
        Err(error) => eprintln!("{}", style.red(format!("could not revoke: {error}"))),
    }
}

async fn fetch_grants(
    service: &Arc<Service>,
    session: SessionKey,
) -> Result<Vec<GrantInfo>, String> {
    let events = drain_request(service, ClientRequestKind::ListGrants { session }).await?;
    for event in events {
        match event.kind {
            ServerEventKind::GrantList { grants } => return Ok(grants),
            ServerEventKind::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
    Err("the service did not return a grant list".to_owned())
}

fn grant_scope_label(scope: GrantScope) -> &'static str {
    match scope {
        GrantScope::Once => "once",
        GrantScope::Tool => "tool/session",
        GrantScope::Session => "session",
        GrantScope::Always => "global",
    }
}

/// Sends one request and drains its acknowledgement.
async fn drain_request(
    service: &Arc<Service>,
    kind: ClientRequestKind,
) -> Result<Vec<ServerEvent>, String> {
    let stream = service
        .submit(ClientRequest {
            request_id: 0,
            kind,
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(stream.collect().await)
}

fn stream_error(events: &[ServerEvent]) -> Option<String> {
    events.iter().find_map(|event| match &event.kind {
        ServerEventKind::Error { message, .. } => Some(message.clone()),
        _ => None,
    })
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
}

/// Aborts the Ctrl-C listener when the turn loop returns, including on error.
struct SignalGuard(tokio::task::JoinHandle<()>);

impl Drop for SignalGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
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
    let _signal = SignalGuard(tokio::spawn(async move {
        loop {
            if tokio::signal::ctrl_c().await.is_err() || interrupt_tx.send(()).await.is_err() {
                break;
            }
        }
    }));
    let mut current: Option<SessionKey> = None;
    let mut parked = false;
    let mut cancel_requested = false;
    // Text the human typed while a turn was running. It waits in the service
    // until the next turn boundary, and the loop below starts that boundary.
    let mut steered = false;
    // Whether the last turn closed normally. A cancelled or budget-stopped
    // turn leaves the session in a state a resume cannot pick up.
    let mut committed = false;
    loop {
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
                        if cancel_requested {
                            eprintln!("{}", style.red("interrupted; forcing exit"));
                            return Err("interrupted".to_owned());
                        }
                        cancel_requested = true;
                        renderer.mark_cancelled();
                        eprintln!("{}", style.yellow("cancelling the running turn…"));
                        eprintln!(
                        "  {}",
                        style.dim(
                            "the running tool is being stopped; press Ctrl-C again to force quit"
                        )
                    );
                        let request = ClientRequest {
                            request_id: 0,
                            kind: ClientRequestKind::CancelTurn { session },
                        };
                        if let Err(error) = service.submit(request).await {
                            eprintln!(
                                "{}",
                                style
                                    .yellow(format!("warning: could not cancel the turn: {error}"))
                            );
                        }
                        continue;
                    }
                    Wake::Line(line) => {
                        let text = line.trim().to_owned();
                        if text.is_empty() {
                            continue;
                        }
                        match parse_chat_command(&text) {
                            Some(Ok(command)) => {
                                handle_live_command(service, command, current.as_ref(), style)
                                    .await;
                            }
                            Some(Err(message)) => eprintln!("{}", style.yellow(message)),
                            None => {
                                if let Some(session) = current.clone() {
                                    steer_text(service, session, text, style, renderer.quiet).await;
                                    steered = true;
                                } else {
                                    eprintln!(
                                        "{}",
                                        style.yellow("the session is not ready yet; try again")
                                    );
                                }
                            }
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
                    ServerEventKind::TurnFinished {
                        phase,
                        committed: done,
                        ..
                    } => {
                        parked = *phase == Phase::AwaitingApproval;
                        committed = *done;
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
                        return Ok(DriveOutcome {
                            session: current,
                            parked: true,
                        });
                    }
                }
                if answer_if_asked(service, &event, input.as_deref_mut(), &mut interrupt_rx).await?
                {
                    renderer.mark_cancelled();
                    eprintln!(
                        "{}",
                        style.yellow(
                            "question left pending; the session is saved and can be resumed"
                        )
                    );
                    return Ok(DriveOutcome {
                        session: current,
                        parked: true,
                    });
                }
                renderer.event(&event)?;
                if matches!(
                    &event.kind,
                    ServerEventKind::TurnFinished {
                        committed: false,
                        ..
                    }
                ) {
                    if let Some(prompt) = renderer.take_budget_prompt() {
                        if renderer.can_prompt() {
                            if let (Some(receiver), Some(session)) =
                                (input.as_deref_mut(), current.clone())
                            {
                                if let Some(limit) =
                                    prompt_budget_limit(receiver, &mut interrupt_rx, style, prompt)
                                        .await?
                                {
                                    let budget = Budget {
                                        unit: prompt.unit,
                                        limit,
                                        action: BudgetAction::Ask,
                                    };
                                    let set = service
                                        .submit(ClientRequest {
                                            request_id: 0,
                                            kind: ClientRequestKind::SetBudget {
                                                session: session.clone(),
                                                budget,
                                            },
                                        })
                                        .await;
                                    match set {
                                        Ok(next) => {
                                            streams.push_back(next);
                                            match service
                                            .submit(ClientRequest {
                                                request_id: 0,
                                                kind: ClientRequestKind::Resume {
                                                    session: session.clone(),
                                                },
                                            })
                                            .await
                                        {
                                            Ok(next) => {
                                                renderer.mark_working();
                                                streams.push_back(next);
                                            }
                                            Err(error) => eprintln!(
                                                "{}",
                                                style.yellow(format!(
                                                    "warning: budget raised but the session was not resumed: {error}"
                                                ))
                                            ),
                                        }
                                        }
                                        Err(error) => eprintln!(
                                            "{}",
                                            style.yellow(format!(
                                                "warning: could not raise the budget: {error}"
                                            ))
                                        ),
                                    }
                                }
                            }
                        }
                    }
                }
            }
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
        // The human steered text while the turn was running. The service holds
        // it durably and applies it at the next turn boundary, so a turn has
        // to start for it to be delivered.
        if steered {
            steered = false;
            let Some(session) = current.clone() else {
                break;
            };
            if parked || renderer.budget_stopped || !committed {
                break;
            }
            let request = ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::Resume { session },
            };
            match service.submit(request).await {
                Ok(next) => {
                    renderer.mark_working();
                    streams.push_back(next);
                }
                Err(error) => {
                    eprintln!(
                        "{}",
                        style.yellow(format!("warning: could not apply the steer: {error}"))
                    );
                    break;
                }
            }
        }
    }
    Ok(DriveOutcome {
        session: current,
        parked,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        BudgetUnit, ChatCommand, Reasoning, ReasoningScope, SignalGuard, command_suggestion,
        parse_budget_limit, parse_chat_command,
    };

    #[test]
    fn reasoning_commands_accept_only_known_settings_and_scopes() {
        assert_eq!(
            parse_chat_command("/reasoning high"),
            Some(Ok(ChatCommand::Reasoning {
                scope: ReasoningScope::Session,
                reasoning: Reasoning::Effort {
                    effort: "high".to_owned()
                }
            }))
        );
        assert_eq!(
            parse_chat_command("/reasoning message low"),
            Some(Ok(ChatCommand::Reasoning {
                scope: ReasoningScope::Message,
                reasoning: Reasoning::Effort {
                    effort: "low".to_owned()
                }
            }))
        );
        assert_eq!(
            parse_chat_command("/reasoning budget:4096"),
            Some(Ok(ChatCommand::Reasoning {
                scope: ReasoningScope::Session,
                reasoning: Reasoning::Budget { tokens: 4096 }
            }))
        );
        assert!(matches!(
            parse_chat_command("/reasoning fast"),
            Some(Err(_))
        ));
        assert!(matches!(
            parse_chat_command("/reasoning high sometimes"),
            Some(Err(_))
        ));
        assert!(matches!(
            parse_chat_command("/reasoning budget:0"),
            Some(Err(_))
        ));
    }

    #[test]
    fn command_arguments_are_validated_instead_of_sent_as_messages() {
        assert_eq!(
            parse_chat_command("/model openrouter/deepseek"),
            Some(Ok(ChatCommand::Model {
                model: "openrouter/deepseek".to_owned()
            }))
        );
        assert!(matches!(
            parse_chat_command("/model deepseek"),
            Some(Err(_))
        ));
        assert_eq!(
            parse_chat_command("/revoke"),
            Some(Ok(ChatCommand::Revoke { grant: None }))
        );
        assert_eq!(
            parse_chat_command("/revoke grant-1"),
            Some(Ok(ChatCommand::Revoke {
                grant: Some("grant-1".to_owned())
            }))
        );
        assert!(matches!(
            parse_chat_command("/revoke one two"),
            Some(Err(_))
        ));
        assert!(matches!(parse_chat_command("/steer hello"), Some(Err(_))));
    }

    #[test]
    fn budget_limits_are_positive_and_respect_the_unit() {
        assert_eq!(parse_budget_limit(BudgetUnit::Usd, "$1.25"), Ok(1.25));
        assert_eq!(parse_budget_limit(BudgetUnit::Tokens, "1_000"), Ok(1000.0));
        assert!(parse_budget_limit(BudgetUnit::Requests, "1.5").is_err());
        assert!(parse_budget_limit(BudgetUnit::Usd, "0").is_err());
        assert!(parse_budget_limit(BudgetUnit::Tokens, "many").is_err());
    }

    #[test]
    fn slash_commands_and_arguments_have_dimmed_suggestions() {
        let commands = command_suggestion("/", 1).expect("command list");
        assert_eq!(commands.completion, "help");
        assert!(commands.hint.contains("/reasoning"));

        let prefix = command_suggestion("/rea", 4).expect("command prefix");
        assert_eq!(prefix.completion, "soning");

        let settings = command_suggestion("/reasoning ", "/reasoning ".len()).expect("settings");
        assert_eq!(settings.completion, "auto");
        assert!(settings.hint.contains("high"));
        assert!(settings.hint.contains("budget:<tokens>"));

        let partial = command_suggestion("/reasoning h", "/reasoning h".len()).expect("effort");
        assert_eq!(partial.completion, "igh");

        let scope =
            command_suggestion("/reasoning high ", "/reasoning high ".len()).expect("scope");
        assert_eq!(scope.completion, "session");

        let tool_scope = command_suggestion("/allow bash ", "/allow bash ".len()).expect("scope");
        assert_eq!(tool_scope.completion, "session");

        assert!(command_suggestion("/steer", "/steer".len()).is_none());
    }

    #[tokio::test]
    async fn dropping_the_guard_aborts_the_interrupt_listener() {
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let guard = SignalGuard(tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            flag.store(true, Ordering::SeqCst);
        }));
        drop(guard);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!ran.load(Ordering::SeqCst));
    }
}
