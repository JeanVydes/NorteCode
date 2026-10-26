//! The interactive turn loop: input, steering, approval handoff and
//! cancellation.

use super::{
    Arc, Budget, BudgetAction, BudgetUnit, ClientRequest, ClientRequestKind, EventStream,
    GrantInfo, GrantScope, JobInfo, Phase, Reasoning, ReasoningScope, Renderer, ServerEvent,
    ServerEventKind, Service, SessionKey, StreamExt, Style, VecDeque, mpsc, short,
};
use crate::approval::{ApprovalChoice, ApprovalOutcome, approval_request, ask_approval};
use crate::input::{EditorAction, EditorEvent, InteractiveInput};
use crate::prompt::{PromptInput, answer_if_asked, prompt_input};
use crate::render::{
    BudgetPrompt, Verbosity, format_measure, parse_reasoning_setting, reasoning_setting_label,
    scope_name, unit_name,
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
/// What `/mcp` was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpAction {
    /// List the connected servers and their tools.
    List,
    /// Turn a server on.
    Enable,
    /// Turn a server off.
    Disable,
}

impl McpAction {
    /// Whether the action needs a server name.
    fn needs_server(self) -> bool {
        match self {
            Self::List => false,
            Self::Enable | Self::Disable => true,
        }
    }

    /// The word the usage line shows.
    fn usage(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Enable => "enable",
            Self::Disable => "disable",
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum ChatCommand {
    /// Show the command list.
    Help,
    /// Summarise the session on demand.
    Compact,
    /// Re-read everything that can be re-read while the process runs.
    Refresh,
    /// List the MCP servers, or switch one off or on.
    Mcp {
        action: McpAction,
        server: Option<String>,
    },
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
    /// Set the current provider.
    Provider { provider: String },
    /// List the background jobs, read one, or stop one.
    Jobs {
        action: JobListAction,
        id: Option<String>,
    },
    /// Start a fresh session, leaving this one saved.
    Restart,
}

/// What `/jobs` was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobListAction {
    /// Every job of the session.
    List,
    /// One job, with what it has written.
    Check,
    /// End one job.
    Stop,
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
        "/refresh" | "/reload" => Some(ChatCommand::Refresh),
        "/mcp" => Some(ChatCommand::Mcp {
            action: McpAction::List,
            server: None,
        }),
        "/exit" | "/quit" | "exit" | "quit" => Some(ChatCommand::Exit),
        "/restart" => Some(ChatCommand::Restart),
        "/jobs" => Some(ChatCommand::Jobs {
            action: JobListAction::List,
            id: None,
        }),
        _ => None,
    };
    if let Some(rest) = command_rest(text, "/jobs") {
        return Some(parse_jobs_command(rest));
    }
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
    if let Some(rest) = command_rest(text, "/mcp") {
        let mut parts = rest.split_whitespace();
        let action = match parts.next() {
            None | Some("list") => McpAction::List,
            Some("enable") => McpAction::Enable,
            Some("disable") => McpAction::Disable,
            Some(other) => {
                return Some(Err(format!(
                    "unknown /mcp action `{other}`; use list, enable or disable"
                )));
            }
        };
        let server = parts.next().map(str::to_owned);
        if parts.next().is_some() {
            return Some(Err("usage: /mcp [list|enable|disable] [server]".to_owned()));
        }
        if action.needs_server() && server.is_none() {
            return Some(Err(format!("usage: /mcp {} <server>", action.usage())));
        }
        if !action.needs_server() && server.is_some() {
            return Some(Err("usage: /mcp list  (it takes no server)".to_owned()));
        }
        return Some(Ok(ChatCommand::Mcp { action, server }));
    }
    if let Some(rest) = command_rest(text, "/model") {
        return Some(parse_model_command(rest));
    }
    if let Some(rest) = command_rest(text, "/provider") {
        return Some(parse_provider_command(rest));
    }
    None
}

/// Returns the argument text after an exact command name.
fn command_rest<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(command)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some(rest)
}

/// Parses the arguments of `/jobs`.
///
/// The action is optional and defaults to `list`, which is what a bare `/jobs`
/// asks for. `list` takes no id and the others require one: a `check` with
/// nothing to check is a usage error, not an empty listing.
fn parse_jobs_command(rest: &str) -> Result<ChatCommand, String> {
    let mut parts = rest.split_whitespace();
    let action = match parts.next() {
        None | Some("list") => JobListAction::List,
        Some("check") => JobListAction::Check,
        Some("stop") => JobListAction::Stop,
        Some(other) => {
            return Err(format!(
                "unknown /jobs action `{other}`; use list, check or stop"
            ));
        }
    };
    let id = parts.next().map(str::to_owned);
    if parts.next().is_some() {
        return Err("usage: /jobs [list|check|stop] [job-id]".to_owned());
    }
    match (action, &id) {
        (JobListAction::List, Some(_)) => Err("/jobs list takes no job id".to_owned()),
        (JobListAction::Check | JobListAction::Stop, None) => {
            Err("/jobs check and /jobs stop need a job id".to_owned())
        }
        _ => Ok(ChatCommand::Jobs { action, id }),
    }
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

fn parse_provider_command(rest: &str) -> Result<ChatCommand, String> {
    const USAGE: &str = "usage: /provider <provider>  (no args lists available providers)";
    let parts: Vec<&str> = rest.split_whitespace().collect();
    match parts.as_slice() {
        [] => Ok(ChatCommand::Provider {
            provider: String::new(),
        }),
        [provider] => {
            if provider.is_empty() {
                return Err(USAGE.to_owned());
            }
            Ok(ChatCommand::Provider {
                provider: (*provider).to_owned(),
            })
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn parse_model_command(rest: &str) -> Result<ChatCommand, String> {
    const USAGE: &str = "usage: /model <model>  or  /model <provider/model>";
    let parts: Vec<&str> = rest.split_whitespace().collect();
    match parts.as_slice() {
        [] => Ok(ChatCommand::Model {
            model: String::new(),
        }),
        [model] => {
            if model.is_empty() {
                return Err(USAGE.to_owned());
            }
            Ok(ChatCommand::Model {
                model: (*model).to_owned(),
            })
        }
        _ => Err(USAGE.to_owned()),
    }
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
    "/provider",
    "/revoke",
    "/refresh",
    "/reload",
    "/mcp",
    "/jobs",
    "/restart",
    "/exit",
    "/quit",
];

const REASONING_SUGGESTIONS: &[&str] = &[
    "auto", "off", "minimal", "low", "medium", "high", "xhigh", "max",
];

const REASONING_SCOPES: &[&str] = &["session", "message"];
const TOOL_SCOPES: &[&str] = &["session", "global"];

/// The actions `/jobs` accepts, for the completion hint.
const JOB_ACTIONS: &[&str] = &["list", "check", "stop"];

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
        "/model" => Some("/model <model>"),
        "/provider" => Some("/provider <provider>"),
        "/revoke" => Some("/revoke [grant-id-or-tool]"),
        "/refresh" | "/reload" => Some("/refresh  (alias: /reload)"),
        "/mcp" => Some("/mcp [list|enable|disable] [server]"),
        "/jobs" => Some("/jobs [list|check|stop] [job-id]"),
        "/restart" => Some("/restart"),
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
        "/jobs" => match completed {
            0 => Some(JOB_ACTIONS),
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

/// Re-reads the configuration and reports what happened.
///
/// The per-session rules and the model a session already resolved are session
/// state and are not rebuilt, so the notice names what needs a new session.
fn report_reload(service: &Arc<Service>, style: Style) {
    match service.reload_config() {
        Ok(reloaded) => {
            eprintln!(
                "{}",
                style.green(format!(
                    "configuration reloaded ({} providers)",
                    reloaded.config.providers.len()
                ))
            );
            for warning in &reloaded.warnings {
                eprintln!(
                    "{}",
                    style.yellow(format!(
                        "warning: unknown config key `{}`",
                        crate::text::terminal(warning)
                    ))
                );
            }
            eprintln!(
                "{}",
                style.dim(
                    "the open session keeps its model and its rules; a new session uses the new default"
                )
            );
        }
        Err(error) => eprintln!(
            "{}",
            style.red(format!("could not reload the configuration: {error}"))
        ),
    }
}

/// Re-reads everything that can be re-read while the process runs.
///
/// The configuration, the skill roots and the MCP servers that failed to start
/// are the three things that can change underneath a running agent, so one
/// command covers them: what a person means by "refresh" is not one of the
/// three, and asking them to remember which is which was the mistake.
async fn report_refresh(service: &Arc<Service>, style: Style) {
    report_reload(service, style);
    let count = service.refresh_skills();
    eprintln!("{}", style.green(format!("skills refreshed: {count}")));
    let recovered = service.retry_mcp().await;
    match recovered {
        0 => {
            let failed = service.mcp_failures();
            if !failed.is_empty() {
                eprintln!(
                    "{}",
                    style.yellow(format!("{} mcp server(s) still unreachable", failed.len()))
                );
            }
        }
        count => eprintln!(
            "{}",
            style.green(format!("mcp server(s) connected: {count}"))
        ),
    }
}

/// Reports the MCP servers, or switches one off or on.
///
/// A server's tools ride in every prompt, so the count is worth seeing; a server
/// that is off keeps its process, which is what makes turning it back on
/// instant.
fn report_mcp(service: &Arc<Service>, action: McpAction, server: Option<&str>, style: Style) {
    if let Some(name) = server {
        let active = action == McpAction::Enable;
        match service.set_mcp_active(name, active) {
            Some(state) => {
                let word = if state { "enabled" } else { "disabled" };
                eprintln!("{}", style.green(format!("mcp server `{name}` {word}")));
                if !state {
                    eprintln!(
                        "{}",
                        style.dim("its tools leave the next turn; the process stays up")
                    );
                }
            }
            None => eprintln!(
                "{}",
                style.yellow(format!("no mcp server named `{name}` connected"))
            ),
        }
        return;
    }

    let status = service.mcp_status();
    if status.is_empty() {
        eprintln!("{}", style.dim("no MCP server connected"));
        return;
    }
    for server in &status {
        let mark = if server.active {
            style.green("on")
        } else {
            style.faint("off")
        };
        eprintln!(
            "{} {} {} {}",
            mark,
            style.bold(server.name.clone()),
            style.dim(server.command.clone()),
            style.dim(format!("{} tool(s)", server.tools.len()))
        );
        if server.active {
            for tool in &server.tools {
                eprintln!("  {}", style.dim(tool.clone()));
            }
        }
        for warning in &server.filtered {
            eprintln!("  {}", style.yellow(warning.clone()));
        }
    }
}

pub(crate) fn print_chat_help(style: Style) {
    eprintln!("{}", style.bold("chat commands"));
    eprintln!("  /help                     show this list");
    eprintln!("  /compact                  summarise and continue on a new session");
    eprintln!("  /allow <tool> [session|global]  allow without asking");
    eprintln!("  /deny <tool> [session|global]   refuse without asking");
    eprintln!("  /reasoning <setting> [session|message]  set reasoning");
    eprintln!("  /model <model>             switch model (uses current provider)");
    eprintln!("  /provider <provider>       switch provider (keeps current model)");
    eprintln!("  /revoke [grant-or-tool]   list or revoke an approval grant");
    eprintln!("  /refresh                  re-read the config, the skills and the MCP servers");
    eprintln!("  /reload                   an alias for /refresh");
    eprintln!("  /exit, /quit              leave the chat");
    eprintln!(
        "{}",
        style.dim(
            "  reasoning: auto, off, minimal, low, medium, high, xhigh, max or budget:<tokens>"
        )
    );
    eprintln!(
        "{}",
        style.dim("  text typed while a turn runs is steered into that turn")
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
    input: &mut InteractiveInput,
    message_budget: Option<Budget>,
    reasoning: Option<Reasoning>,
) -> Result<Option<SessionKey>, String> {
    if renderer.verbosity.at_least(Verbosity::Medium) {
        eprintln!(
            "{}",
            style.dim("interactive: /help for commands; /compact, /allow, /deny, /refresh, /exit")
        );
    }
    let mut request_id = 2_u64;
    loop {
        let Some(text) = read_repl_line(service, session.as_ref(), input, style).await? else {
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
                ChatCommand::Refresh => report_refresh(service, style).await,
                ChatCommand::Mcp { action, server } => {
                    report_mcp(service, action, server.as_deref(), style);
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
                    if model.is_empty() {
                        // List available models for the current provider
                        let provider = renderer
                            .model_name
                            .as_ref()
                            .and_then(|m| m.split_once('/').map(|(p, _)| p.to_owned()))
                            .unwrap_or_else(|| "openai".to_owned());
                        let catalog = service.catalog();
                        eprintln!("{}", style.bold(format!("Models for {provider}:")));
                        if let Some(p) = catalog.provider(&provider) {
                            let mut models: Vec<String> = p.models.keys().cloned().collect();
                            models.sort();
                            for m in &models {
                                eprintln!("  {}", style.cyan(m));
                            }
                            if models.is_empty() {
                                eprintln!("  {}", style.dim("(none in catalogue)"));
                            }
                        } else {
                            eprintln!(
                                "  {}",
                                style.dim("(provider not in catalogue; any model name accepted)")
                            );
                        }
                    } else if let Some(current) = session.clone() {
                        // If model doesn't contain '/', prepend the provider from
                        // the renderer's current model name.
                        let full_model = if model.contains('/') {
                            model
                        } else {
                            let provider = renderer
                                .model_name
                                .as_ref()
                                .and_then(|m| m.split_once('/').map(|(p, _)| p.to_owned()))
                                .unwrap_or_else(|| "openai".to_owned());
                            format!("{provider}/{model}")
                        };
                        renderer.model_name = Some(full_model.clone());
                        run_model(service, current, full_model, style).await;
                    } else {
                        eprintln!(
                            "{}",
                            style.yellow("there is no session yet; send a message first")
                        );
                    }
                }
                ChatCommand::Provider { provider } => {
                    if provider.is_empty() {
                        // List available providers
                        let catalog = service.catalog();
                        let config = service.config();
                        eprintln!("{}", style.bold("Providers:"));
                        let mut providers: Vec<String> =
                            catalog.providers.keys().cloned().collect();
                        for id in config.providers.keys() {
                            if !providers.contains(id) {
                                providers.push(id.clone());
                            }
                        }
                        providers.sort();
                        for p in &providers {
                            let in_catalog = catalog.provider(p).is_some();
                            let tag = if in_catalog { "catalog" } else { "config" };
                            eprintln!("  {} ({})", style.cyan(p), style.dim(tag));
                        }
                    } else if let Some(current) = session.clone() {
                        // Switch provider while keeping the current model name.
                        let model_name = renderer
                            .model_name
                            .as_ref()
                            .and_then(|m| m.split_once('/').map(|(_, name)| name.to_owned()))
                            .unwrap_or_else(|| "default".to_owned());
                        let full = format!("{provider}/{model_name}");
                        renderer.model_name = Some(full.clone());
                        run_model(service, current, full, style).await;
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
                ChatCommand::Jobs { action, id } => match session.clone() {
                    Some(current) => report_jobs(service, current, action, id, style).await,
                    None => eprintln!(
                        "{}",
                        style.yellow("there is no session yet; send a message first")
                    ),
                },
                // A restart drops the session and starts a fresh one. The
                // service creates a session on the first message, so clearing
                // this is the whole of it, and the old session stays saved and
                // resumable. Only between turns: during one, the service is
                // still writing to the session being dropped.
                ChatCommand::Restart => {
                    session = None;
                    eprintln!(
                        "{}",
                        style.green(
                            "starting a fresh session; the previous one is saved and can be \
                             resumed"
                        )
                    );
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
                    // The transcript is already on screen; replaying it would
                    // print the conversation twice.
                    replay: false,
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

/// The model a session is on, without its provider.
///
/// A provider switch keeps the model and replaces the provider, so the name has
/// to come from the session's own spec rather than from the renderer: during a
/// turn the renderer is mid-answer and its name is the one already in flight.
fn model_name_of(service: &Arc<Service>, session: &SessionKey) -> Option<String> {
    service
        .session_model(session)
        .and_then(|reference| reference.split_once('/').map(|(_, name)| name.to_owned()))
}

/// Steers a message into the running turn and names it, so the client can
/// tell it apart when the engine reports it applied.
async fn steer_text(
    service: &Arc<Service>,
    session: SessionKey,
    text: String,
    style: Style,
    quiet: bool,
) -> Option<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let outcome = drain_request(
        service,
        ClientRequestKind::Steer {
            session,
            text,
            id: id.clone(),
        },
    )
    .await;
    match outcome {
        Ok(events) => {
            if let Some(message) = stream_error(&events) {
                eprintln!("{}", style.red(format!("could not steer: {message}")));
                return None;
            }
            if !quiet {
                eprintln!("{}", style.dim("steered into the running turn"));
            }
            Some(id)
        }
        Err(error) => {
            eprintln!("{}", style.red(format!("could not steer: {error}")));
            None
        }
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
        ChatCommand::Refresh => report_refresh(service, style).await,
        ChatCommand::Mcp { action, server } => {
            report_mcp(service, action, server.as_deref(), style);
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
        // A provider switch is a model switch: the same request, with the
        // provider replaced and the model name kept. During a turn it queues
        // exactly as `/model` does — the refusal here was the odd one out, and
        // it meant a person who wanted the next turn on another provider had to
        // wait for the current one to end and then remember to ask again.
        ChatCommand::Provider { provider } => {
            if provider.is_empty() {
                eprintln!(
                    "{}",
                    style.yellow("wait until the current turn finishes, then /provider")
                );
            } else if let Some(session) = current {
                let model_name =
                    model_name_of(service, session).unwrap_or_else(|| "default".to_owned());
                run_model(
                    service,
                    session.clone(),
                    format!("{provider}/{model_name}"),
                    style,
                )
                .await;
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
        ChatCommand::Jobs { action, id } => match current {
            Some(session) => report_jobs(service, session.clone(), action, id, style).await,
            None => eprintln!("{}", style.yellow("no session yet; send a message first")),
        },
        // A restart ends the session, so it cannot wait for the turn; the
        // turn loop does it, where the boundary is known.
        ChatCommand::Restart => eprintln!(
            "{}",
            style.yellow("wait until the current turn finishes, then /restart")
        ),
    }
}

/// Asks for a new limit after the budget stopped the turn.
///
/// A line that is not a limit is not thrown away: it is steered into the
/// session, because the human may have been typing when the budget ran out.
async fn prompt_budget_limit(
    service: &Arc<Service>,
    session: &SessionKey,
    input: &mut InteractiveInput,
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
                    Err(error) => {
                        eprintln!("{}", style.yellow(error));
                        eprintln!(
                            "{}",
                            style.dim("that is not a limit; it is steered into the session")
                        );
                        steer_text(service, session.clone(), line.to_owned(), style, true).await;
                    }
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

/// Reports the session's background jobs: a listing, one job, or a stop.
///
/// A job belongs to the session that started it, and the service is asked
/// rather than the registry read directly: the CLI has no registry, and a job
/// may be running in a turn this client never started.
async fn report_jobs(
    service: &Arc<Service>,
    session: SessionKey,
    action: JobListAction,
    id: Option<String>,
    style: Style,
) {
    match action {
        // `stop` names one job; `list` names none. The service treats a stop
        // with no id as every job of the session, which is what Ctrl-C sends.
        JobListAction::Stop => {
            let request = ClientRequest {
                request_id: 0,
                kind: ClientRequestKind::StopJobs {
                    session,
                    id: id.clone(),
                },
            };
            match service.submit(request).await {
                Ok(mut stream) => {
                    let mut stopped = None;
                    while let Some(event) = stream.next().await {
                        if let ServerEventKind::JobsStopped { stopped: count } = event.kind {
                            stopped = Some(count);
                        }
                    }
                    match stopped {
                        Some(0) => eprintln!(
                            "{}",
                            style.yellow(match &id {
                                Some(id) => format!("no running job `{id}`"),
                                None => "no background job was running".to_owned(),
                            })
                        ),
                        Some(count) => eprintln!(
                            "{}",
                            style.green(format!("stopped {count} background job(s)"))
                        ),
                        None => eprintln!("{}", style.yellow("the jobs could not be stopped")),
                    }
                }
                Err(error) => eprintln!(
                    "{}",
                    style.yellow(format!("warning: could not stop the jobs: {error}"))
                ),
            }
        }
        JobListAction::List | JobListAction::Check => {
            let jobs = match fetch_jobs(service, session).await {
                Ok(jobs) => jobs,
                Err(error) => {
                    eprintln!("{}", style.red(format!("could not list the jobs: {error}")));
                    return;
                }
            };
            if let (JobListAction::Check, Some(id)) = (&action, &id) {
                match jobs.iter().find(|job| &job.id == id) {
                    Some(job) => print_job(job, style),
                    None => eprintln!(
                        "{}",
                        style.yellow(format!("no job `{id}`; /jobs lists the ones there are"))
                    ),
                }
            } else {
                if jobs.is_empty() {
                    eprintln!("{}", style.dim("no background jobs in this session"));
                    return;
                }
                eprintln!(
                    "{}",
                    style.bold(format!("{} background job(s)", jobs.len()))
                );
                for job in &jobs {
                    eprintln!(
                        "  {} {} {}",
                        crate::text::terminal(&job.id),
                        job_state_label(&job.state, style),
                        style.dim(&job.script)
                    );
                }
            }
        }
    }
}

/// One job, with what it has written.
fn print_job(job: &JobInfo, style: Style) {
    eprintln!(
        "{} {}{} ({} ms)",
        crate::text::terminal(&job.id),
        job_state_label(&job.state, style),
        match job.exit_code {
            Some(code) => format!(", exit {code}"),
            None => String::new(),
        },
        job.elapsed_ms
    );
    eprintln!("  {}", style.dim(&job.script));
    if job.timed_out {
        eprintln!("  {}", style.yellow("it reached its time bound"));
    }
    if job.dropped {
        eprintln!(
            "  {}",
            style.yellow("older output was dropped to stay within the buffer")
        );
    }
    if job.output.is_empty() {
        eprintln!("  {}", style.dim("(no output yet)"));
        return;
    }
    eprintln!("{}", crate::text::terminal(&job.output));
}

/// The state of a job as it is printed.
fn job_state_label(state: &str, style: Style) -> String {
    match state {
        "running" => style.cyan(state.to_owned()),
        "cancelled" => style.yellow(state.to_owned()),
        _ => style.green(state.to_owned()),
    }
}

/// Asks the service for the session's jobs.
async fn fetch_jobs(service: &Arc<Service>, session: SessionKey) -> Result<Vec<JobInfo>, String> {
    let events = drain_request(service, ClientRequestKind::ListJobs { session }).await?;
    for event in events {
        match event.kind {
            ServerEventKind::JobList { jobs } => return Ok(jobs),
            ServerEventKind::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
    Err("the service did not return a job list".to_owned())
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
///
/// Ctrl-C is read from the editor rather than from a signal: the terminal is in
/// raw mode, so the key never becomes `SIGINT`. At the prompt its meaning
/// depends on what is running — a background job is stopped, and with none the
/// key leaves the loop, which is what it did when it was a signal.
pub(crate) async fn read_repl_line(
    service: &Arc<Service>,
    session: Option<&SessionKey>,
    input: &mut InteractiveInput,
    style: Style,
) -> Result<Option<String>, String> {
    let _ = style;
    loop {
        let event = tokio::select! {
            biased;
            // A signal can still arrive from outside the terminal, so it keeps
            // its old meaning: leave the loop.
            _ = tokio::signal::ctrl_c() => {
                eprintln!();
                return Ok(None);
            }
            event = input.recv() => event,
        };
        let line = match event {
            Some(EditorEvent::Line(line)) => line,
            Some(EditorEvent::End) | None => {
                eprintln!();
                return Ok(None);
            }
            Some(EditorEvent::Action(EditorAction::CancelJobs)) => {
                let Some(session) = session.filter(|session| service.has_running_jobs(session))
                else {
                    // No job to stop: the key means what it always meant at the
                    // prompt, which is to leave.
                    eprintln!();
                    return Ok(None);
                };
                stop_jobs(service, session.clone(), style).await;
                continue;
            }
            // Nothing is running at the prompt, so nothing to cancel or detach.
            Some(EditorEvent::Action(_)) => continue,
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

/// Stops every background job of the session, and reports what it reached.
pub(crate) async fn stop_jobs(service: &Arc<Service>, session: SessionKey, style: Style) {
    let request = ClientRequest {
        request_id: 0,
        kind: ClientRequestKind::StopJobs { session, id: None },
    };
    match service.submit(request).await {
        Ok(mut stream) => {
            let mut stopped = None;
            while let Some(event) = stream.next().await {
                if let ServerEventKind::JobsStopped { stopped: count } = event.kind {
                    stopped = Some(count);
                }
            }
            match stopped {
                Some(0) => eprintln!("{}", style.yellow("no background job was running")),
                Some(count) => eprintln!(
                    "{}",
                    style.yellow(format!("stopped {count} background job(s)"))
                ),
                None => eprintln!("{}", style.yellow("the jobs could not be stopped")),
            }
        }
        Err(error) => eprintln!(
            "{}",
            style.yellow(format!("warning: could not stop the jobs: {error}"))
        ),
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
    /// Escape, Ctrl-C or Ctrl-Shift-B reached the editor and asked for
    /// something that is not a line.
    Action(EditorAction),
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
    // The editor itself, not one of its channels: a turn needs the lines *and*
    // the actions, and asking for them through one handle is what keeps the
    // select below honest.
    mut input: Option<&mut crate::input::InteractiveInput>,
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
    // The steering messages the human sent while the turn was running, with
    // their text. The service applies them to the request in flight and names
    // them when it does; the ones still waiting need the next turn to start.
    let mut steered: Vec<(String, String)> = Vec::new();
    // Whether the last turn closed normally. A cancelled or budget-stopped
    // turn leaves the session in a state a resume cannot pick up.
    let mut committed = false;
    loop {
        while let Some(mut stream) = streams.pop_front() {
            loop {
                let wake = if let Some(receiver) = input.as_deref_mut() {
                    tokio::select! {
                        biased;
                        Some(()) = interrupt_rx.recv() => Wake::Interrupt,
                        event = receiver.recv() => match event {
                            Some(EditorEvent::Line(line)) => Wake::Line(line),
                            Some(EditorEvent::Action(action)) => Wake::Action(action),
                            Some(EditorEvent::End) | None => Wake::Eof,
                        },
                        next = stream.next() => Wake::Event(Box::new(next)),
                    }
                } else {
                    tokio::select! {
                        biased;
                        Some(()) = interrupt_rx.recv() => Wake::Interrupt,
                        event = stream.next() => Wake::Event(Box::new(event)),
                    }
                };
                let event = match wake {
                    // A keystroke the editor could not act on itself: it knows
                    // which key was pressed, not what a turn is. All three
                    // decisions are made here, where that is known.
                    Wake::Action(action) => {
                        match action {
                            EditorAction::CancelTurn => {
                                if let Some(session) = current.clone() {
                                    renderer.mark_cancelled();
                                    eprintln!("{}", style.yellow("cancelling the running turn…"));
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
                                continue;
                            }
                            EditorAction::CancelJobs => {
                                // The session's jobs, and only its: a job
                                // belongs to the work it was started for. With
                                // none running, the key says so rather than
                                // silently doing nothing.
                                let Some(session) = current.clone() else {
                                    continue;
                                };
                                if !service.has_running_jobs(&session) {
                                    eprintln!("{}", style.dim("no background job is running"));
                                    continue;
                                }
                                stop_jobs(service, session, style).await;
                                continue;
                            }
                            EditorAction::Detach => {
                                let Some(session) = current.clone() else {
                                    continue;
                                };
                                let request = ClientRequest {
                                    request_id: 0,
                                    kind: ClientRequestKind::DetachTurn { session },
                                };
                                if let Err(error) = service.submit(request).await {
                                    eprintln!(
                                        "{}",
                                        style.yellow(format!(
                                            "warning: could not detach the turn: {error}"
                                        ))
                                    );
                                }
                                continue;
                            }
                        }
                    }
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
                                let Some(session) = current.clone() else {
                                    eprintln!(
                                        "{}",
                                        style.yellow("the session is not ready yet; try again")
                                    );
                                    continue;
                                };
                                // The message is not in the history until the
                                // engine applies it, so it is not printed as if
                                // it were: it waits above the input, where the
                                // human can still see what they sent, and moves
                                // into the transcript when the engine reports
                                // that it landed.
                                if let Some(id) = steer_text(
                                    service,
                                    session,
                                    text.clone(),
                                    style,
                                    !renderer.verbosity.at_least(Verbosity::Medium),
                                )
                                .await
                                {
                                    steered.push((id, text));
                                    let waiting: Vec<String> =
                                        steered.iter().map(|(_, text)| text.clone()).collect();
                                    renderer.pinned.set_pending_steers(waiting);
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
                    ServerEventKind::SteerApplied { ids, messages } => {
                        // Every applied message now belongs to the transcript.
                        // One the human typed is written where the conversation
                        // reached, which is where it belongs; one they did not
                        // (a finished job's report) is shown with the time it
                        // arrived, because there is nowhere else to see it.
                        for (id, text) in ids.iter().zip(messages.iter()) {
                            let typed = steered.iter().find(|(name, _)| name == id).is_some();
                            if typed {
                                renderer.arrived_message(text);
                            } else {
                                renderer.steer_arrived(text);
                            }
                        }
                        steered.retain(|(id, _)| !ids.contains(id));
                        let waiting: Vec<String> =
                            steered.iter().map(|(_, text)| text.clone()).collect();
                        renderer.pinned.set_pending_steers(waiting);
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
                                if let Some(limit) = prompt_budget_limit(
                                    service,
                                    &session,
                                    receiver,
                                    &mut interrupt_rx,
                                    style,
                                    prompt,
                                )
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
        // An approval submits a new stream that continues the parked turn.
        // Drive it before deciding the loop is done, or the decision is lost
        // and the caller sees a parked session that is already approved.
        if !streams.is_empty() {
            continue;
        }
        // The human steered text while the turn was running. The service
        // applies it to the request in flight; what the turn never reached
        // waits for the next turn boundary, so that boundary has to start.
        // With nothing left to apply, the turn loop is done: returning here is
        // what hands the terminal back to the input line.
        if steered.is_empty() {
            break;
        }
        let Some(session) = current.clone() else {
            break;
        };
        if parked || renderer.budget_stopped || !committed {
            break;
        }
        // The waiting messages are not forgotten here: a resume is the turn
        // that will reach them, and they must stay above the input until the
        // engine says it did. Clearing the list would hide a message that is
        // still on its way to the model.
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
    // Nothing is waiting any more: the turn ended, whatever is left was never
    // applied, and leaving it above the input would promise a delivery that is
    // not coming.
    if !steered.is_empty() {
        renderer.pinned.set_pending_steers(Vec::new());
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
        BudgetUnit, CHAT_COMMANDS, ChatCommand, JobListAction, McpAction, Reasoning,
        ReasoningScope, SignalGuard, command_suggestion, command_usage, parse_budget_limit,
        parse_chat_command,
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
        assert_eq!(
            parse_chat_command("/model deepseek"),
            Some(Ok(ChatCommand::Model {
                model: "deepseek".to_owned()
            }))
        );
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
    fn mcp_lists_switches_and_refuses_a_bad_argument() {
        // Bare `/mcp` lists.
        assert_eq!(
            parse_chat_command("/mcp"),
            Some(Ok(ChatCommand::Mcp {
                action: McpAction::List,
                server: None,
            }))
        );
        // The three actions, with the server they act on.
        for (text, action) in [
            ("/mcp enable everything", McpAction::Enable),
            ("/mcp disable everything", McpAction::Disable),
        ] {
            assert_eq!(
                parse_chat_command(text),
                Some(Ok(ChatCommand::Mcp {
                    action,
                    server: Some("everything".to_owned()),
                })),
                "{text}"
            );
        }
        // A switch without a server, and an unknown action, are refused rather
        // than sent to the model as a message.
        assert!(matches!(parse_chat_command("/mcp enable"), Some(Err(_))));
        assert!(matches!(
            parse_chat_command("/mcp frobnicate x"),
            Some(Err(_))
        ));
        // Prose is still a message.
        assert!(parse_chat_command("mcp enable everything").is_none());
        assert!(CHAT_COMMANDS.contains(&"/mcp"), "the list offers it");
    }

    #[test]
    fn refresh_and_reload_are_the_same_command() {
        // A command a user types must never reach the model as text, and the
        // two spellings do the same work: one re-reads what can be re-read.
        assert!(matches!(
            parse_chat_command("/refresh"),
            Some(Ok(ChatCommand::Refresh))
        ));
        assert!(matches!(
            parse_chat_command("/reload"),
            Some(Ok(ChatCommand::Refresh))
        ));
        assert!(
            parse_chat_command("refresh the config").is_none(),
            "prose is a message, not the command"
        );
        assert!(command_usage("/refresh").is_some());
        assert!(command_usage("/reload").is_some());
        assert!(CHAT_COMMANDS.contains(&"/refresh"), "the list offers it");
        assert!(CHAT_COMMANDS.contains(&"/reload"), "and its alias");
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

    #[test]
    fn jobs_takes_a_list_check_or_stop_and_validates_its_id() {
        // A bare `/jobs` is the listing, which is what a user reaching for it
        // without arguments means.
        assert_eq!(
            parse_chat_command("/jobs"),
            Some(Ok(ChatCommand::Jobs {
                action: JobListAction::List,
                id: None
            }))
        );
        assert_eq!(
            parse_chat_command("/jobs list"),
            Some(Ok(ChatCommand::Jobs {
                action: JobListAction::List,
                id: None
            }))
        );
        assert_eq!(
            parse_chat_command("/jobs check job-2"),
            Some(Ok(ChatCommand::Jobs {
                action: JobListAction::Check,
                id: Some("job-2".to_owned())
            }))
        );
        assert_eq!(
            parse_chat_command("/jobs stop job-2"),
            Some(Ok(ChatCommand::Jobs {
                action: JobListAction::Stop,
                id: Some("job-2".to_owned())
            }))
        );
        // `check` with nothing to check is a usage error, not an empty listing.
        assert!(matches!(parse_chat_command("/jobs check"), Some(Err(_))));
        assert!(matches!(parse_chat_command("/jobs stop"), Some(Err(_))));
        assert!(matches!(
            parse_chat_command("/jobs list job-1"),
            Some(Err(_))
        ));
        assert!(matches!(
            parse_chat_command("/jobs show job-1"),
            Some(Err(_))
        ));
        assert!(matches!(parse_chat_command("/jobs one two"), Some(Err(_))));
    }

    #[test]
    fn restart_is_an_exact_command_and_offered_by_help() {
        assert!(matches!(
            parse_chat_command("/restart"),
            Some(Ok(ChatCommand::Restart))
        ));
        assert!(
            CHAT_COMMANDS.contains(&"/jobs") && CHAT_COMMANDS.contains(&"/restart"),
            "the list must offer both: {CHAT_COMMANDS:?}"
        );
        assert_eq!(
            command_usage("/jobs"),
            Some("/jobs [list|check|stop] [job-id]")
        );
        assert_eq!(command_usage("/restart"), Some("/restart"));
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
