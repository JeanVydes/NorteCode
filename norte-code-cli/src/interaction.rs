//! The interactive turn loop: input, steering, approval handoff and
//! cancellation.

use super::{
    Arc, Budget, BudgetAction, BudgetUnit, ClientRequest, ClientRequestKind, EventStream,
    GrantInfo, GrantScope, JobInfo, Phase, Reasoning, ReasoningScope, Renderer, ServerEvent,
    ServerEventKind, Service, SessionKey, StreamExt, Style, VecDeque, mpsc, short,
};
use crate::approval::{ApprovalChoice, ApprovalOutcome, approval_request, ask_approval};
use crate::input::{Attachment, EditorAction, EditorEvent, InteractiveInput};

use crate::prompt::{PromptInput, answer_if_asked, prompt_input};
use crate::render::{
    BudgetPrompt, Verbosity, format_measure, mcp_summary, parse_reasoning_setting,
    reasoning_setting_label, report_mcp as render_mcp, report_prompt, report_reasoning,
    report_start, report_tool_rules, report_tools, scope_name, tools_summary, unit_name,
};
use crate::tabs::SessionTab;
use norte_code_commands::Invocation;
/// How often a running turn redraws, in ms.
///
/// It is the clock behind a call's counter and the spinner. Fast enough that a
/// second never looks stuck, slow enough that a provider taking a minute costs
/// a handful of frames.
const TURN_TICK_MS: u64 = 120;

/// How often the interface redraws while nothing else is happening, in ms.
///
/// It is what keeps a typed character and a spinner on screen. Sixty
/// milliseconds is more than a terminal needs for either and few enough that a
/// client at a prompt costs nothing: the frame is diffed against the last one,
/// so a tick with nothing changed writes nothing.
const READER_TICK_MS: u64 = 60;

/// The wire's own attachment, named apart from the editor's: they carry the
/// same three fields today, and they are not the same type — one is what a
/// keystroke produced and the other is what goes on the wire.
use norte_code_protocol::Attachment as ProtocolAttachment;

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
    /// List every tool and how a call to it is decided.
    Tools,
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
    /// Name the session. A bare `/rename` asks the model for a title from the
    /// first message the session holds.
    Rename { title: Option<String> },
    /// List the background jobs, read one, or stop one.
    Jobs {
        action: JobListAction,
        id: Option<String>,
    },
    /// Start a fresh session, leaving this one saved.
    Restart,
    /// Show another tab of the session, or list them.
    Tab {
        /// The tab's name or number; `None` lists them.
        target: Option<String>,
    },

    /// Report what the process knows about itself: the model, its
    /// capabilities, the tools, the servers, the paths. Built when it is asked
    /// for, not captured at startup, so the tools are the ones registered now.
    Debug,
    /// A command a plugin declared.
    ///
    /// It arrives parsed and not as text: the registry that completed it is the
    /// same one that parsed it, so a plugin's command is a command like any
    /// other and the plugin is handed what it declared.
    Plugin(norte_code_commands::Invocation),
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
///
/// The names, the arguments and the usage lines all come from the registry, so
/// this function is only the mapping from a parsed invocation to what the client
/// does about it. That mapping is the one thing that cannot move: the registry
/// is written to be usable by something that is not this binary.
pub(crate) fn parse_chat_command(text: &str) -> Option<Result<ChatCommand, String>> {
    let text = text.trim();
    // `exit` and `quit` without a slash are what a person types at a prompt
    // that looks like a shell, and they have always left the chat.
    if matches!(text, "exit" | "quit") {
        return Some(Ok(ChatCommand::Exit));
    }
    // A command that was taken away still says so, rather than reaching the
    // model as a message that starts with a slash.
    if command_rest(text, "/steer").is_some() {
        return Some(Err(
            "/steer was removed; type the text directly while a turn is running".to_owned(),
        ));
    }
    let invocation = match crate::commands_registry::shared().parse(text)? {
        Ok(invocation) => invocation,
        Err(problem) => return Some(Err(problem)),
    };
    // A command the registry knows and this mapping does not: the registry is
    // the list of what a person may type, so a gap here is a command that does
    // nothing. The tests check the two agree.
    let command = match invocation.command.as_str() {
        "/help" => ChatCommand::Help,
        "/compact" => ChatCommand::Compact,
        "/refresh" => ChatCommand::Refresh,
        "/restart" => ChatCommand::Restart,
        "/debug" => ChatCommand::Debug,
        "/tools" => ChatCommand::Tools,
        "/exit" => ChatCommand::Exit,
        "/mcp" => match parse_mcp(&invocation) {
            Ok(command) => command,
            Err(problem) => return Some(Err(problem)),
        },
        "/jobs" => match parse_jobs(&invocation) {
            Ok(command) => command,
            Err(problem) => return Some(Err(problem)),
        },
        "/allow" | "/deny" => {
            let allow = invocation.command == "/allow";
            let global = invocation.arg("scope") == Some("global");
            let tool = invocation.arg_or_empty("tool").to_owned();
            ChatCommand::Approval {
                allow,
                tool,
                global,
            }
        }
        "/reasoning" => match parse_reasoning(&invocation) {
            Ok(command) => command,
            Err(problem) => return Some(Err(problem)),
        },
        "/revoke" => ChatCommand::Revoke {
            grant: invocation.arg("grant").map(str::to_owned),
        },
        "/model" => ChatCommand::Model {
            model: invocation.arg_or_empty("model").to_owned(),
        },
        "/provider" => ChatCommand::Provider {
            provider: invocation.arg_or_empty("provider").to_owned(),
        },
        "/tab" => ChatCommand::Tab {
            target: invocation.arg("target").map(str::to_owned),
        },
        "/rename" => ChatCommand::Rename {
            title: invocation.arg("title").map(str::to_owned),
        },
        // Everything else the registry knows came from a plugin: the client's
        // own names are all above, and a gap here would be a command declared
        // and not implemented. The tests check the two lists agree.
        _ => ChatCommand::Plugin(invocation),
    };
    Some(Ok(command))
}

/// The `/mcp` command, with its action and server checked against each other.
///
/// The registry knows the two arguments; what it cannot know is that `list`
/// takes no server and the others need one, which is a rule about this command.
fn parse_mcp(invocation: &Invocation) -> Result<ChatCommand, String> {
    let action = match invocation.arg("action").unwrap_or("list") {
        "list" => McpAction::List,
        "enable" => McpAction::Enable,
        "disable" => McpAction::Disable,
        other => {
            return Err(format!(
                "unknown /mcp action `{other}`; use list, enable or disable"
            ));
        }
    };
    let server = invocation.arg("server").map(str::to_owned);
    if action.needs_server() && server.is_none() {
        return Err(format!("usage: /mcp {} <server>", action.usage()));
    }
    if !action.needs_server() && server.is_some() {
        return Err("usage: /mcp list  (it takes no server)".to_owned());
    }
    Ok(ChatCommand::Mcp { action, server })
}

/// The `/jobs` command, with its action and id checked against each other.
fn parse_jobs(invocation: &Invocation) -> Result<ChatCommand, String> {
    let action = match invocation.arg("action").unwrap_or("list") {
        "list" => JobListAction::List,
        "check" => JobListAction::Check,
        "stop" => JobListAction::Stop,
        other => {
            return Err(format!(
                "unknown /jobs action `{other}`; use list, check or stop"
            ));
        }
    };
    let id = invocation.arg("job-id").map(str::to_owned);
    match (action, &id) {
        (JobListAction::List, Some(_)) => Err("/jobs list takes no job id".to_owned()),
        (JobListAction::Check | JobListAction::Stop, None) => {
            Err("/jobs check and /jobs stop need a job id".to_owned())
        }
        _ => Ok(ChatCommand::Jobs { action, id }),
    }
}

/// The `/reasoning` command, with the scope and the setting resolved.
fn parse_reasoning(invocation: &Invocation) -> Result<ChatCommand, String> {
    let Some(setting) = invocation.arg("setting") else {
        return Err("usage: /reasoning <setting> [session|message]".to_owned());
    };
    let scope = match invocation.arg("scope") {
        Some("message") => ReasoningScope::Message,
        _ => ReasoningScope::Session,
    };
    let reasoning = parse_reasoning_setting(setting)?;
    Ok(ChatCommand::Reasoning { scope, reasoning })
}

/// Returns the argument text after an exact command name.
fn command_rest<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(command)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some(rest)
}

/// One dimmed completion shown by the input editor.
///
/// It is what the registry returned, in the shape the editor draws: the text to
/// append and the row to show. The editor does not decide what may be typed —
/// it asks the registry and shows the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandSuggestion {
    /// The text Tab inserts at the cursor.
    pub(crate) completion: String,
    /// The dim hint shown on the row above the input.
    pub(crate) hint: String,
}

/// Reads the completion for the line as it stands.
///
/// `None` means there is nothing to offer: the text is not a command, or the
/// cursor is not at the end of it, which is what keeps a completion from
/// rewriting the middle of a line somebody is editing.
/// How many rows the completion offers for a line.
///
/// The keymap asks this to know whether the arrows belong to the menu: one row
/// is a hint and not a list, and taking the arrows for a choice nobody has to
/// make is how a menu ends up feeling broken.
pub(crate) fn completion_rows(text: &str, cursor: usize) -> usize {
    crate::commands_registry::shared()
        .candidates(text, cursor)
        .len()
}

/// What Tab inserts for the row at `pick`.
///
/// `pick` is clamped to the rows that exist, because the keyboard sends it and
/// the line it was chosen for may be gone: completing to a row that is no
/// longer offered is worse than completing to the first one.
#[must_use]
pub(crate) fn completion_text(text: &str, cursor: usize, pick: usize) -> Option<String> {
    let rows = crate::commands_registry::shared().candidates(text, cursor);
    rows.get(pick.min(rows.len().saturating_sub(1)))
        .map(|row| row.completion.clone())
        .filter(|completion| !completion.is_empty())
}

pub(crate) fn command_suggestion(text: &str, cursor: usize) -> Option<CommandSuggestion> {
    let rows = crate::commands_registry::shared().candidates(text, cursor);
    if rows.is_empty() {
        return None;
    }
    let first = &rows[0];
    let hint = if rows.len() == 1 {
        // One candidate: the row says what Tab is about to write and, after a
        // dash, what it is. `xhigh — setting` tells a person both that the word
        // is `xhigh` and that it is the setting of whatever they typed, which
        // is the question they were asking by typing a prefix of it.
        if first.note.is_empty() {
            first.text.clone()
        } else {
            format!("{} — {}", first.text, first.note)
        }
    } else {
        // Several are a list, and the names are what there is to show: a
        // terminal with no room for a menu still says what may be typed.
        rows.iter()
            .map(|row| row.text.clone())
            .collect::<Vec<String>>()
            .join("  ")
    };
    Some(CommandSuggestion {
        completion: first.completion.clone(),
        hint,
    })
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
    // The interactive hint is on screen already: the tabs row and the footer
    // say what there is, and a line printed above them would be a second
    // welcome that the frame then draws over.
    if renderer.verbosity.at_least(Verbosity::Medium) {
        let _ = &style;
    }
    let mut request_id = 2_u64;
    // The tabs open on something: without this the tools and MCP rows stay
    // blank until the first turn ends, and an empty row reads as nothing to
    // report rather than as not looked at yet.
    refresh_tab_summaries(service, session.as_ref(), &renderer.pinned, style);
    // The conversation opens with the mark and a frame: a session that showed
    // nothing until the first tool ran looked like a client that had not
    // started.
    renderer.greet();
    renderer.draw()?;
    loop {
        let Some((text, attachments)) =
            read_repl_line(service, session.as_ref(), input, renderer, style).await?
        else {
            break;
        };
        let command = match parse_chat_command(&text) {
            None => None,
            Some(Ok(command)) => Some(command),
            Some(Err(message)) => {
                renderer.notice(message);
                continue;
            }
        };
        if let Some(command) = command {
            match command {
                ChatCommand::Exit => break,
                ChatCommand::Help => print_chat_help(renderer, style),
                // The report is built now, not at startup: the tools, the model
                // and the servers are all things that change while the process
                // runs, so the useful moment to ask is after they have.
                ChatCommand::Debug => report_debug(service, session.as_ref(), renderer, style),
                ChatCommand::Tools => report_tools_now(service, session.as_ref(), renderer, style),
                // A command from a plugin: the plugin answers with the lines
                // of a panel, and the frame draws it like any other. The client
                // does not know what the command is for and does not need to:
                // what comes back is text.
                ChatCommand::Plugin(invocation) => {
                    let plugins = crate::plugins::all();
                    match plugins.run(&invocation) {
                        Some(lines) => renderer.open_modal(invocation.command.clone(), lines),
                        None => renderer.failure(format!(
                            "`{}` was declared by a plugin and nothing answered it",
                            invocation.command
                        )),
                    }
                }

                ChatCommand::Compact => {
                    let Some(current) = session.clone() else {
                        renderer.notice("there is no session to compact yet; send a message first");
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
                ChatCommand::Refresh => report_refresh(service, renderer).await,
                ChatCommand::Rename { title } => match session.clone() {
                    Some(current) => {
                        rename_session(service, current, title, renderer).await;
                    }
                    None => renderer.notice("there is no session yet; send a message first"),
                },
                ChatCommand::Mcp { action, server } => {
                    report_mcp(service, action, server.as_deref(), renderer, style);
                    refresh_tab_summaries(service, session.as_ref(), &renderer.pinned, style);
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
                        renderer.notice(format!("{verb} `{tool}` {scope}"));
                        // The rule just changed, so the row that counts the
                        // rules is stale until it is rebuilt.
                        refresh_tab_summaries(service, session.as_ref(), &renderer.pinned, style);
                    }
                    Err(error) => {
                        renderer.failure(format!("could not set the rule: {error}"));
                    }
                },
                ChatCommand::Reasoning { scope, reasoning } => {
                    if let Some(current) = session.clone() {
                        run_reasoning(service, current, scope, reasoning, renderer).await;
                    } else {
                        renderer.notice("there is no session yet; send a message first");
                    }
                }
                // `/tab` is the way to change tabs that does not need a
                // modified key: `Ctrl-T` cycles, and this one travels where a
                // terminal will not send the key. Bare, it lists them.
                ChatCommand::Tab { target } => {
                    let wanted = match target.as_deref() {
                        None => {
                            // Which tab is showing is state, and the list of
                            // them is a report: it is read and dismissed.
                            let lines = vec![format!(
                                "tabs: {}",
                                SessionTab::ALL
                                    .iter()
                                    .map(|tab| format!(
                                        "{}{}",
                                        if *tab == renderer.pinned.tab() {
                                            "*"
                                        } else {
                                            " "
                                        },
                                        tab.name()
                                    ))
                                    .collect::<Vec<_>>()
                                    .join("  ")
                            )];
                            renderer.open_modal("tabs", lines);
                            continue;
                        }
                        Some(target) => target
                            .parse::<usize>()
                            .ok()
                            .and_then(SessionTab::numbered)
                            .or_else(|| SessionTab::named(target)),
                    };
                    match wanted {
                        Some(tab) => renderer.pinned.set_tab(tab),
                        None => renderer.notice(format!(
                            "no tab `{target:?}`; use {}",
                            SessionTab::ALL
                                .iter()
                                .map(|tab| tab.name())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )),
                    }
                }
                ChatCommand::Model { model } => {
                    if model.is_empty() {
                        // List available models for the current provider. It is
                        // the session's own provider, read from its spec: the
                        // renderer's name is the model in flight, and guessing
                        // `openai` when there is no session listed the wrong
                        // catalogue under the right heading.
                        let provider = session
                            .as_ref()
                            .and_then(|current| provider_of(service, current));
                        let Some(provider) = provider else {
                            renderer.notice("there is no session yet; send a message first");
                            continue;
                        };
                        let catalog = service.catalog();
                        // The model list is a report: it is read and dismissed,
                        // and as lines of the conversation a catalogue of
                        // hundreds would bury the work.
                        let mut lines = vec![format!("Models for {provider}:")];
                        if let Some(p) = catalog.provider(&provider) {
                            let mut models: Vec<String> = p.models.keys().cloned().collect();
                            models.sort();
                            for m in &models {
                                lines.push(format!("  {m}"));
                            }
                            if models.is_empty() {
                                lines.push("  (none in catalogue)".to_owned());
                            }
                        } else {
                            lines.push(
                                "  (provider not in catalogue; any model name accepted)".to_owned(),
                            );
                        }
                        renderer.open_modal(format!("models of {provider}"), lines);
                    } else if let Some(current) = session.clone() {
                        // If model doesn't contain '/', prepend the provider from
                        // the renderer's current model name.
                        // A model named with its provider is sent as it was
                        // written, because the client does not take a reference
                        // apart and a model id may carry a slash of its own. A
                        // bare name is completed with the session's provider,
                        // which is read from the spec rather than guessed.
                        let full_model = if model.contains('/') {
                            model
                        } else {
                            let provider = provider_of(service, &current);
                            let Some(provider) = provider else {
                                renderer.notice("the session has no provider yet");
                                continue;
                            };
                            format!("{provider}/{model}")
                        };
                        renderer.model_name = Some(full_model.clone());
                        run_model(service, current, full_model, renderer).await;
                    } else {
                        renderer.notice("there is no session yet; send a message first");
                    }
                }
                ChatCommand::Provider { provider } => {
                    if provider.is_empty() {
                        // List available providers
                        let catalog = service.catalog();
                        let config = service.config();
                        let mut lines = vec!["Providers:".to_owned()];
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
                            lines.push(format!("  {p} ({tag})"));
                        }
                        renderer.open_modal("providers", lines);
                    } else if let Some(current) = session.clone() {
                        // Switch provider while keeping the current model. The
                        // model is the session's own, read from its spec: the
                        // renderer's name is the one in flight, and taking the
                        // old reference apart to find the model would break on a
                        // model id that contains a slash.
                        let model_name = model_name_of(service, &current)
                            .unwrap_or_else(|| "default".to_owned());
                        let full = format!("{provider}/{model_name}");
                        renderer.model_name = Some(full.clone());
                        run_model(service, current, full, renderer).await;
                    } else {
                        renderer.notice("there is no session yet; send a message first");
                    }
                }
                ChatCommand::Revoke { grant } => {
                    if let Some(current) = session.clone() {
                        run_revoke(service, current, grant, renderer).await;
                    } else {
                        renderer.notice("there is no session yet; send a message first");
                    }
                }
                ChatCommand::Jobs { action, id } => match session.clone() {
                    Some(current) => {
                        report_jobs(service, current, action, id, renderer, style).await;
                    }
                    None => renderer.notice("there is no session yet; send a message first"),
                },
                // A restart drops the session and starts a fresh one. The
                // service creates a session on the first message, so clearing
                // this is the whole of it, and the old session stays saved and
                // resumable. Only between turns: during one, the service is
                // still writing to the session being dropped.
                ChatCommand::Restart => {
                    session = None;
                    renderer.notice(
                        "starting a fresh session; the previous one is saved and can be resumed",
                    );
                }
            }
            continue;
        }
        // The message is the conversation, so it goes to the transcript and the
        // frame draws it where the conversation is. Printed onto the terminal,
        // it landed on top of the input.
        renderer.said(&text);
        renderer.mark_working();
        // The first message of a session that has no title is what the title is
        // made from, and it is asked for now so the request runs beside the turn
        // instead of in front of it. `renderer.title.is_none()` is the whole
        // condition: it is set the moment one arrives, so a session is asked
        // about once and a resumed one that already has a name is not asked
        // about at all.
        let titling = match (session.as_ref(), renderer.title.is_none(), text.as_str()) {
            (Some(key), true, first) => Titling::start(service, key.clone(), first.to_owned()),
            _ => Titling::none(),
        };
        let stream = service
            .submit(ClientRequest {
                request_id,
                kind: ClientRequestKind::SendMessage {
                    session: session.clone(),
                    text,
                    attachments: attachments
                        .iter()
                        .map(|attachment| ProtocolAttachment {
                            path: attachment.path.clone(),
                            mime: attachment.mime.clone(),
                            name: attachment.name.clone(),
                        })
                        .collect(),
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
        let outcome =
            drive_turns_titled(service, stream, renderer, style, Some(&mut *input), titling)
                .await?;
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
    renderer: &mut Renderer,
) {
    let kind = ClientRequestKind::SetReasoning {
        session,
        scope,
        reasoning: reasoning.clone(),
    };
    match drain_request(service, kind).await {
        Ok(events) => {
            if let Some(message) = stream_error(&events) {
                renderer.failure(format!("could not set reasoning: {message}"));
            } else {
                let label = match scope {
                    ReasoningScope::Session => "for this session",
                    ReasoningScope::Message => "for the next message",
                };
                renderer.notice(format!(
                    "reasoning set to {} {label}",
                    reasoning_setting_label(&reasoning)
                ));
                // The footer carries the effort, so setting it and not showing
                // it would leave the one place it appears saying the old value.
                if scope == ReasoningScope::Session {
                    renderer.set_reasoning(reasoning_setting_label(&reasoning));
                }
            }
        }
        Err(error) => renderer.failure(format!("could not set reasoning: {error}")),
    }
}

/// The model a session is on, without its provider.
///
/// A provider switch keeps the model and replaces the provider, so the name has
/// to come from the session's own spec rather than from the renderer: during a
/// turn the renderer is mid-answer and its name is the one already in flight.
///
/// The provider is not read here and the label is not split to find it: the spec
/// carries the two apart, and a model id may contain a slash of its own, which is
/// why a model is never taken apart to find its provider.
fn model_name_of(service: &Arc<Service>, session: &SessionKey) -> Option<String> {
    service.session_spec(session).map(|spec| spec.model)
}

/// The provider a session is on.
///
/// The other half of the same fact, read the same way: from the spec's own field.
fn provider_of(service: &Arc<Service>, session: &SessionKey) -> Option<String> {
    service.session_spec(session).map(|spec| spec.provider)
}

/// Steers a message into the running turn and names it, so the client can
/// tell it apart when the engine reports it applied.
async fn steer_text(
    service: &Arc<Service>,
    session: SessionKey,
    text: String,
    renderer: &mut Renderer,
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
                renderer.failure(format!("could not steer: {message}"));
                return None;
            }
            // The message waits above the input until the engine reports it
            // applied, so this line says it left and no more than that: a
            // second notice for a message the person just typed is noise.
            renderer.notice("steered into the running turn");
            Some(id)
        }
        Err(error) => {
            renderer.failure(format!("could not steer: {error}"));
            None
        }
    }
}

/// The tab a `/tab` argument names, or the tab a bare one would list.
///
/// `None` is a name no tab has. It is shared by the two paths that run a
/// command — the repl between turns and the live handler during one — because
/// they have to accept the same words: `/tab 2` and `/tab todo` mean the same
/// thing whichever path reads them, and one of them used to answer that tabs
/// needed a terminal it was standing in front of.
fn tab_target(target: Option<&str>, renderer: &Renderer) -> Result<SessionTab, String> {
    match target {
        // Bare during a turn: the tab after the current one, which is what
        // `Ctrl-T` does. The repl path lists the tabs instead, because it has
        // the room and a person between turns is asking what there is.
        None => Ok(renderer.pinned.tab().next()),
        Some(target) => target
            .parse::<usize>()
            .ok()
            .and_then(SessionTab::numbered)
            .or_else(|| SessionTab::named(target))
            .ok_or_else(|| {
                format!(
                    "no tab `{target}`; use {}",
                    SessionTab::ALL
                        .iter()
                        .map(|tab| tab.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
    }
}

#[allow(clippy::too_many_lines)]
async fn handle_live_command(
    service: &Arc<Service>,
    command: ChatCommand,
    current: Option<&SessionKey>,
    renderer: &mut Renderer,
    style: Style,
) {
    match command {
        // The tab changes while the turn runs. There is a terminal -- this is
        // the interactive turn loop, and the footer is on screen -- so the old
        // message claiming otherwise was simply wrong, and the command is the
        // one that helps most here: Ctrl-T is a keystroke a person reaches for
        // while the model is working.
        ChatCommand::Tab { target } => match tab_target(target.as_deref(), renderer) {
            Ok(tab) => renderer.pinned.set_tab(tab),
            Err(message) => renderer.notice(message),
        },
        ChatCommand::Exit => renderer.notice("wait until the current turn finishes, then /exit"),
        ChatCommand::Help => print_chat_help(renderer, style),
        // The report needs no turn boundary: it reads state that is already
        // settled, so it is the one command that works mid-turn.
        ChatCommand::Debug => report_debug(service, current, renderer, style),
        ChatCommand::Tools => report_tools_now(service, current, renderer, style),
        // A command from a plugin: the plugin answers with the lines
        // of a panel, and the frame draws it like any other. The client
        // does not know what the command is for and does not need to:
        // what comes back is text.
        ChatCommand::Plugin(invocation) => {
            let plugins = crate::plugins::all();
            match plugins.run(&invocation) {
                Some(lines) => renderer.open_modal(invocation.command.clone(), lines),
                None => renderer.failure(format!(
                    "`{}` was declared by a plugin and nothing answered it",
                    invocation.command
                )),
            }
        }

        ChatCommand::Compact => {
            renderer.notice("wait until the current turn finishes, then /compact");
        }
        ChatCommand::Refresh => report_refresh(service, renderer).await,
        ChatCommand::Mcp { action, server } => {
            report_mcp(service, action, server.as_deref(), renderer, style);
            refresh_tab_summaries(service, current, &renderer.pinned, style);
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
                renderer.notice(format!("{verb} `{tool}` {scope}"));
            }
            Err(error) => renderer.failure(format!("could not set the rule: {error}")),
        },
        ChatCommand::Reasoning { scope, reasoning } => {
            if let Some(session) = current {
                run_reasoning(service, session.clone(), scope, reasoning, renderer).await;
            } else {
                renderer.notice("the session is not ready yet; try again");
            }
        }
        ChatCommand::Model { model } => {
            if let Some(session) = current {
                run_model(service, session.clone(), model, renderer).await;
            } else {
                renderer.notice("the session is not ready yet; try again");
            }
        }
        // A provider switch is a model switch: the same request, with the
        // provider replaced and the model name kept. During a turn it queues
        // exactly as `/model` does — the refusal here was the odd one out, and
        // it meant a person who wanted the next turn on another provider had to
        // wait for the current one to end and then remember to ask again.
        ChatCommand::Provider { provider } => {
            if provider.is_empty() {
                renderer.notice("wait until the current turn finishes, then /provider");
            } else if let Some(session) = current {
                let model_name =
                    model_name_of(service, session).unwrap_or_else(|| "default".to_owned());
                run_model(
                    service,
                    session.clone(),
                    format!("{provider}/{model_name}"),
                    renderer,
                )
                .await;
            } else {
                renderer.notice("the session is not ready yet; try again");
            }
        }
        ChatCommand::Revoke { grant } => {
            if let Some(session) = current {
                run_revoke(service, session.clone(), grant, renderer).await;
            } else {
                renderer.notice("the session is not ready yet; try again");
            }
        }
        ChatCommand::Jobs { action, id } => match current {
            Some(session) => {
                report_jobs(service, session.clone(), action, id, renderer, style).await;
            }
            None => renderer.notice("no session yet; send a message first"),
        },
        // A restart ends the session, so it cannot wait for the turn; the
        // turn loop does it, where the boundary is known.
        ChatCommand::Rename { title } => match current {
            Some(current) => rename_session(service, current.clone(), title, renderer).await,
            None => renderer.notice("no session yet; send a message first"),
        },
        // A restart ends the session, so it cannot wait for the turn; the
        // turn loop does it, where the boundary is known.
        ChatCommand::Restart => {
            renderer.notice("wait until the current turn finishes, then /restart");
        }
    }
}

/// Names the session, from the text given or from the model.
///
/// `/rename fix the footer` writes what was typed and does not call anything:
/// a person who has already decided what the session is called should not wait
/// for a model to agree. A bare `/rename` asks the model, from the first message
/// the session holds, which is the path a session that predates titling takes.
async fn rename_session(
    service: &Arc<Service>,
    session: SessionKey,
    title: Option<String>,
    renderer: &mut Renderer,
) {
    // Nothing typed, so the model picks the name. It is the same request the
    // first message triggers, asked late: the session already has the history
    // to read from, and the answer is worth waiting for here.
    let Some(title) = title else {
        renderer.notice("asking the model for a name…");
        match service.retitle_session(&session).await {
            Ok(title) => {
                renderer.set_title(title.clone());
                renderer.notice(format!("named `{title}`"));
            }
            Err(error) => renderer.failure(format!("could not name the session: {error}")),
        }
        return;
    };
    // What was typed. It goes through the rename request, so the session's own
    // rules judge it and the event that comes back is the one the footer
    // listens for.
    let title = title.trim().to_owned();
    if title.is_empty() {
        renderer.notice("usage: /rename <title>  or  /rename");
        return;
    }
    let request = ClientRequestKind::RenameSession {
        session: session.clone(),
        title: title.clone(),
    };
    match drain_request(service, request).await {
        Ok(events) => {
            if let Some(error) = stream_error(&events) {
                renderer.failure(format!("could not rename: {error}"));
            } else {
                renderer.set_title(title.clone());
                renderer.notice(format!("named `{title}`"));
            }
        }
        Err(error) => renderer.failure(format!("could not rename: {error}")),
    }
}

/// Prints what the process knows about itself.
///
/// Built when it is asked for: the tools, the model and the servers all change
/// while the process runs, so a report captured at startup would describe the
/// wrong moment — the tools the model could reach before an MCP server dropped,
/// the model before a `/model`. Everything is read here and printed to stderr,
/// where the rest of the diagnostics live, so it does not land in the answer.
/// The tools and how a call to each is decided, for `/tools`.
///
/// It goes to stderr with the other diagnostics, not into the answer: it is
/// what the person asked the client, not what the model said.
fn report_tools_now(
    service: &Arc<Service>,
    session: Option<&SessionKey>,
    renderer: &mut Renderer,
    style: Style,
) {
    // A report is a panel and not a print: the frame owns the terminal, and
    // lines written onto it land on top of the conversation.
    let lines = report_tool_rules(style, service, session);
    renderer.open_modal("tools", lines);
}

/// Rebuilds the one-row summaries the tools and MCP tabs show.
///
/// Called at every moment either can change: when the session opens, when a
/// turn ends (a tool an MCP server published may have come or gone), and after
/// a command that writes a rule. The summaries arrive as text, so the footer
/// draws them without knowing what a rule is.
fn refresh_tab_summaries(
    service: &Arc<Service>,
    session: Option<&SessionKey>,
    pinned: &crate::input::Pinned,
    style: Style,
) {
    pinned.set_tool_summary(tools_summary(style, service, session));
    pinned.set_mcp_summary(mcp_summary(style, service));
}

fn report_debug(
    service: &Arc<Service>,
    session: Option<&SessionKey>,
    renderer: &mut Renderer,
    style: Style,
) {
    let config = service.config();
    let catalog = service.catalog();
    // The session's own spec once it has one: after a `/model` the model in play
    // is the session's, not the one the process started with. Before the first
    // message there is no session, so the default is what the next turn opens on.
    let spec = session
        .and_then(|session| service.session_spec(session))
        .unwrap_or_else(|| service.default_spec());
    let host = renderer.host_facts.as_ref();

    let mut lines = report_start(
        style,
        &spec,
        &config,
        service.config_path().as_deref(),
        session.map(SessionKey::as_str),
        &catalog,
        host,
    );
    lines.extend(report_tools(style, service));
    lines.extend(render_mcp(style, service));
    if let Some(host) = host {
        lines.extend(report_prompt(style, service, host.prompt_source));
    }
    // The same value the footer shows, and for the same reason: it is the
    // setting the next turn will send, already resolved once. Reading it here
    // instead of resolving it again is what keeps the report and the footer from
    // disagreeing.
    let effort = renderer
        .reasoning
        .clone()
        .unwrap_or_else(|| "(not resolved yet)".to_owned());
    lines.extend(report_reasoning(style, &effort));
    // The whole report is one panel: it is what the process knows about itself,
    // asked for and dismissed, and it does not belong in the conversation.
    renderer.open_modal("debug", lines);
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
    renderer: &mut Renderer,
    prompt: BudgetPrompt,
) -> Result<Option<f64>, String> {
    renderer.notice(format!(
        "budget exhausted for the {} ({} of {} {}); enter a new limit, or leave it empty to stop",
        scope_name(prompt.scope),
        format_measure(prompt.unit, prompt.spent),
        format_measure(prompt.unit, prompt.limit),
        unit_name(prompt.unit)
    ));
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
                        renderer.notice(error);
                        renderer.notice("that is not a limit; it is steered into the session");
                        steer_text(service, session.clone(), line.to_owned(), renderer).await;
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

/// Recomputes the effort the footer shows from the session's own state.
///
/// A resumed session carries its own override, and that is what the next turn
/// sends; a footer showing the config's default instead would name an effort
/// the session is not on. The setting is resolved once, here, and the footer
/// and `/debug` both read that: computing it twice is how they came to
/// disagree.
pub(crate) fn refresh_effort(
    service: &Arc<Service>,
    session: &SessionKey,
    renderer: &mut Renderer,
) {
    let label = service.session_spec(session).and_then(|spec| {
        let override_reasoning = service.session_reasoning(session);
        norte_code_agent::effective_reasoning(
            override_reasoning.as_ref(),
            &service.config().reasoning.default,
            &spec.capabilities.reasoning,
        )
        .ok()
        .map(|effective| reasoning_setting_label(&effective))
    });
    if let Some(label) = label {
        renderer.set_reasoning(label);
    }
}

async fn run_model(
    service: &Arc<Service>,
    session: SessionKey,
    model: String,
    renderer: &mut Renderer,
) {
    // Cloned: the refresh below needs the key after the request consumed it.
    let key = session.clone();
    match drain_request(service, ClientRequestKind::SetModel { session, model }).await {
        Ok(events) => {
            let mut reported = false;
            for event in events {
                match event.kind {
                    ServerEventKind::ModelChanged { model } => {
                        reported = true;
                        renderer.notice(format!("model set to {model}"));
                        // A new model can refuse the effort in force — the
                        // service downgrades it to one the model declares — so
                        // the value the footer shows is recomputed rather than
                        // left describing the old model.
                        refresh_effort(service, &key, renderer);
                    }
                    ServerEventKind::ModelQueued { model } => {
                        reported = true;
                        renderer.notice(format!(
                            "model switch to {model} queued for the next turn boundary"
                        ));
                    }
                    ServerEventKind::Error { message, .. } => {
                        reported = true;
                        renderer.failure(format!("could not switch model: {message}"));
                    }
                    _ => {}
                }
            }
            if !reported {
                renderer.notice("the model switch was not acknowledged");
            }
        }
        Err(error) => renderer.failure(format!("could not switch model: {error}")),
    }
}

async fn run_revoke(
    service: &Arc<Service>,
    session: SessionKey,
    grant: Option<String>,
    renderer: &mut Renderer,
) {
    let grants = match fetch_grants(service, session.clone()).await {
        Ok(grants) => grants,
        Err(error) => {
            renderer.failure(format!("could not list grants: {error}"));
            return;
        }
    };
    let Some(choice) = grant else {
        if grants.is_empty() {
            renderer.notice("no approval grants");
            return;
        }
        // The listing is a report: it is read and dismissed, and as lines of
        // the conversation it would push the work away.
        let mut lines = vec!["approval grants".to_owned()];
        for grant in grants {
            lines.push(format!(
                "  {}{}  tool {}  {}  approval {}  issued {}",
                crate::text::terminal(&grant.grant),
                if grant.revoked { " (revoked)" } else { "" },
                crate::text::terminal(&grant.tool),
                grant_scope_label(grant.scope),
                crate::text::terminal(&grant.approval),
                crate::text::terminal(&grant.issued_at),
            ));
        }
        renderer.open_modal("approval grants", lines);
        return;
    };
    let matching: Vec<_> = grants
        .iter()
        .filter(|grant| !grant.revoked && (grant.grant == choice || grant.tool == choice))
        .collect();
    let found = match matching.as_slice() {
        [grant] => *grant,
        [] => {
            renderer.notice(format!(
                "no active grant matches `{choice}`; use /revoke to list grants"
            ));
            return;
        }
        _ => {
            renderer.notice(format!(
                "`{choice}` matches more than one grant; use the full grant id"
            ));
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
            Some(message) => renderer.failure(format!("could not revoke: {message}")),
            None => renderer.notice(format!("grant {grant} revoked")),
        },
        Err(error) => renderer.failure(format!("could not revoke: {error}")),
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
    renderer: &mut Renderer,
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
                        Some(0) => renderer.notice(match &id {
                            Some(id) => format!("no running job `{id}`"),
                            None => "no background job was running".to_owned(),
                        }),
                        Some(count) => {
                            renderer.notice(format!("stopped {count} background job(s)"));
                        }
                        None => renderer.notice("the jobs could not be stopped"),
                    }
                }
                Err(error) => renderer.notice(format!("warning: could not stop the jobs: {error}")),
            }
        }
        JobListAction::List | JobListAction::Check => {
            let jobs = match fetch_jobs(service, session).await {
                Ok(jobs) => jobs,
                Err(error) => {
                    renderer.failure(format!("could not list the jobs: {error}"));
                    return;
                }
            };
            if let (JobListAction::Check, Some(id)) = (&action, &id) {
                match jobs.iter().find(|job| &job.id == id) {
                    Some(job) => print_job(job, renderer, style),
                    None => {
                        renderer.notice(format!("no job `{id}`; /jobs lists the ones there are"));
                    }
                }
            } else {
                if jobs.is_empty() {
                    renderer.notice("no background jobs in this session");
                    return;
                }
                // The listing is a report: it is read and dismissed.
                let mut lines = vec![format!("{} background job(s)", jobs.len())];
                for job in &jobs {
                    lines.push(format!(
                        "  {} {} {}",
                        crate::text::terminal(&job.id),
                        job_state_label(&job.state, crate::style::Style::plain()),
                        job.script
                    ));
                }
                renderer.open_modal("background jobs", lines);
            }
        }
    }
}

/// One job, with what it has written.
fn print_job(job: &JobInfo, renderer: &mut Renderer, style: Style) {
    let _ = style;
    let mut lines = vec![format!(
        "{} {}{} ({} ms)",
        crate::text::terminal(&job.id),
        job_state_label(&job.state, crate::style::Style::plain()),
        match job.exit_code {
            Some(code) => format!(", exit {code}"),
            None => String::new(),
        },
        job.elapsed_ms
    )];
    lines.push(format!("  {}", job.script));
    if job.timed_out {
        lines.push("  it reached its time bound".to_owned());
    }
    if job.dropped {
        lines.push("  older output was dropped to stay within the buffer".to_owned());
    }
    if job.output.is_empty() {
        lines.push("  (no output yet)".to_owned());
        return;
    }
    lines.push(String::new());
    for line in crate::text::terminal(&job.output).lines() {
        lines.push(line.to_owned());
    }
    renderer.open_modal(format!("job {}", job.id), lines);
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

/// A title request that is on its way back.
///
/// The title is asked for at the moment the first message is sent and runs
/// beside the turn, not before it: waiting for a name would make every session
/// pay a second round trip before its first token, and the name is not what the
/// person is waiting for. The request goes out on a task of its own, so the
/// answer arrives wherever the loop happens to be — which is why it is a
/// channel, and why the reader selects over it.
pub(crate) struct Titling {
    /// `None` when no title was asked for, so nothing ever arrives.
    receiver: Option<mpsc::UnboundedReceiver<Result<(SessionKey, String), String>>>,
}

impl Titling {
    /// Nothing asked for: a session that already has a title keeps it, and one
    /// whose titling is switched off is never named.
    pub(crate) fn none() -> Self {
        Self { receiver: None }
    }

    /// Asks for a title, on a task of its own.
    ///
    /// The request is started, not awaited: it is deliberately the cheapest the
    /// service makes, and it still has no business holding up the turn it names.
    pub(crate) fn start(
        service: &Arc<Service>,
        session: SessionKey,
        first_message: String,
    ) -> Self {
        if !service.config().titling.enabled {
            return Self::none();
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        let service = Arc::clone(service);
        tokio::spawn(async move {
            let title = service
                .title_session(&session, &first_message)
                .await
                .map(|title| (session, title))
                .map_err(|error| error.to_string());
            // A receiver that is gone means the session ended before the title
            // did, which is nobody left to name and not a failure to report.
            let _ = sender.send(title);
        });
        Self {
            receiver: Some(receiver),
        }
    }

    /// The next title, for as long as one is on its way.
    ///
    /// With nothing asked for this never resolves, and it has to be that rather
    /// than returning at once: a select arm that is always ready spins the loop
    /// it sits in — the outer `loop` re-enters the select, and with `biased` the
    /// arm is taken again before anything else is polled, so the task burns a
    /// core and never yields.
    pub(crate) async fn next(&mut self) -> Option<Result<(SessionKey, String), String>> {
        let answer = match self.receiver.as_mut() {
            Some(receiver) => receiver.recv().await,
            None => std::future::pending().await,
        };
        // One request, one answer. Dropping the receiver is what keeps the arm
        // from being selected again: a channel whose sender is gone answers
        // `None` for ever, which is the same spin by a slower road.
        self.receiver = None;
        answer
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

/// Re-reads everything that can be re-read while the process runs.
///
/// The configuration, the skill roots and the MCP servers that failed to start
/// are the three things that can change underneath a running agent, so one
/// command covers them: what a person means by "refresh" is not one of the
/// three, and asking them to remember which is which was the mistake.
async fn report_refresh(service: &Arc<Service>, renderer: &mut Renderer) {
    report_reload(service, renderer);
    let count = service.refresh_skills();
    renderer.notice(format!("skills refreshed: {count}"));
    let recovered = service.retry_mcp().await;
    match recovered {
        0 => {
            let failed = service.mcp_failures();
            if !failed.is_empty() {
                renderer.notice(format!("{} mcp server(s) still unreachable", failed.len()));
            }
        }
        count => renderer.notice(format!("mcp server(s) connected: {count}")),
    }
}

/// Reports the MCP servers, or switches one off or on.
///
/// A server's tools ride in every prompt, so the count is worth seeing; a server
/// that is off keeps its process, which is what makes turning it back on
/// instant.
fn report_mcp(
    service: &Arc<Service>,
    action: McpAction,
    server: Option<&str>,
    renderer: &mut Renderer,
    style: Style,
) {
    if let Some(name) = server {
        let active = action == McpAction::Enable;
        match service.set_mcp_active(name, active) {
            Some(state) => {
                let word = if state { "enabled" } else { "disabled" };
                // Turning a server on or off is something that happened, so it
                // is a line of the conversation and not a panel: the person
                // wants to see it was done and carry on.
                renderer.notice(format!("mcp server `{name}` {word}"));
                if !state {
                    renderer.notice("its tools leave the next turn; the process stays up");
                }
            }
            None => renderer.failure(format!("no mcp server named `{name}` connected")),
        }
        return;
    }

    // The listing is a report: it is read and dismissed, and as lines of the
    // conversation it would push the work away.
    let lines = render_mcp(style, service);
    if lines.is_empty() {
        renderer.notice("no MCP server connected");
        return;
    }
    renderer.open_modal("mcp servers", lines);
}

/// Re-reads the configuration and reports what happened.
///
/// The per-session rules and the model a session already resolved are session
/// state and are not rebuilt, so the notice names what needs a new session.
fn report_reload(service: &Arc<Service>, renderer: &mut Renderer) {
    match service.reload_config() {
        Ok(reloaded) => {
            renderer.notice(format!(
                "configuration reloaded ({} providers)",
                reloaded.config.providers.len()
            ));
            for warning in &reloaded.warnings {
                renderer.notice(format!(
                    "warning: unknown config key `{}`",
                    crate::text::terminal(warning)
                ));
            }
            renderer.notice(
                "the open session keeps its model and its rules; a new session uses the new default",
            );
        }
        Err(error) => {
            renderer.failure(format!("could not reload the configuration: {error}"));
        }
    }
}

/// Prints the command list.
///
/// Every line comes from the registry, so a command that is declared is a
/// command that is listed and the two cannot drift. The usage column is
/// measured from the lines themselves rather than being a number kept beside
/// them: a command that grows an argument would push the summaries out of line.
pub(crate) fn print_chat_help(renderer: &mut Renderer, style: Style) {
    let registry = crate::commands_registry::shared();
    let mut lines = vec![style.bold("chat commands")];
    // The width the usage lines share, so the summaries line up and the list
    // reads down rather than across. It is measured from the lines themselves
    // instead of being a number kept beside them: a command that grows an
    // argument would otherwise push its summary out of line.
    let usages: Vec<String> = registry
        .commands()
        .iter()
        .map(norte_code_commands::CommandSpec::usage)
        .collect();
    let width = usages.iter().map(String::len).max().unwrap_or(0);
    for (spec, usage) in registry.commands().iter().zip(&usages) {
        lines.push(format!("{usage:<width$}  {}", spec.summary));
        // An alias is a name a person may type, so it is named where the
        // command it belongs to is named.
        if !spec.aliases.is_empty() {
            lines.push(format!("{:width$}  (also {})", "", spec.aliases.join(", ")));
        }
    }
    lines.push(String::new());
    lines.push("text typed while a turn runs is steered into that turn".to_owned());
    lines.push("an approval scope defaults to session; global writes the user config".to_owned());
    renderer.open_modal("help", lines);
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
    renderer: &mut Renderer,
    style: Style,
) -> Result<Option<(String, Vec<Attachment>)>, String> {
    let _ = style;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(READER_TICK_MS));
    loop {
        let event = tokio::select! {
            biased;
            // A signal can still arrive from outside the terminal, so it keeps
            // its old meaning: leave the loop.
            _ = tokio::signal::ctrl_c() => {
                return Ok(None);
            }
            event = input.recv() => event,
            // The tick is what keeps a typed character on screen: a keystroke
            // is not a server event, so without a clock the draft would appear
            // only when something else happened to arrive.
            _ = tick.tick() => {
                renderer.draw()?;
                continue;
            }
        };
        // A keystroke changed the draft, and the draft is what the frame shows.
        renderer.draw()?;
        let (line, attachments) = match event {
            Some(EditorEvent::Line(line, attachments)) => (line, attachments),
            Some(EditorEvent::End) | None => {
                return Ok(None);
            }
            Some(EditorEvent::Action(EditorAction::CancelJobs)) => {
                let Some(session) = session.filter(|session| service.has_running_jobs(session))
                else {
                    // No job to stop: the key means what it always meant at the
                    // prompt, which is to leave.
                    return Ok(None);
                };
                stop_jobs(service, session.clone(), renderer).await;
                continue;
            }
            // Nothing is running at the prompt, so nothing to cancel or detach.
            // No question reaches this loop: one is answered inside the turn
            // that asked it, and an action at the prompt has no question to
            // belong to.
            Some(EditorEvent::Question(_) | EditorEvent::Action(_)) => continue,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line, "/exit" | "/quit" | "exit" | "quit") {
            return Ok(None);
        }
        return Ok(Some((line.to_owned(), attachments)));
    }
}

/// Stops every background job of the session, and reports what it reached.
pub(crate) async fn stop_jobs(
    service: &Arc<Service>,
    session: SessionKey,
    renderer: &mut Renderer,
) {
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
                Some(0) => renderer.notice("no background job was running"),
                Some(count) => renderer.notice(format!("stopped {count} background job(s)")),
                None => renderer.notice("the jobs could not be stopped"),
            }
        }
        Err(error) => renderer.notice(format!("warning: could not stop the jobs: {error}")),
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
    /// The clock, and nothing else: a frame is due.
    ///
    /// It is what a running call's counter and the spinner are drawn from. A
    /// turn that only redrew on an event would show a call stuck at `0s` while
    /// the provider took a minute to answer.
    Tick,
    /// Ctrl-C or another interrupt.
    Interrupt,
    /// Escape, Ctrl-C or Ctrl-G reached the editor and asked for
    /// something that is not a line.
    Action(EditorAction),
    /// A line completed in the input box, with the files its draft attached.
    Line(String, Vec<Attachment>),
    /// End of input.
    Eof,
    /// The next server event.
    Event(Box<Option<ServerEvent>>),
    /// A title arrived, or the request for one failed.
    Title(Result<(SessionKey, String), String>),
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
    input: Option<&mut crate::input::InteractiveInput>,
) -> Result<DriveOutcome, String> {
    drive_turns_titled(service, primary, renderer, style, input, Titling::none()).await
}

/// The same loop, with a title request running beside it.
///
/// It is a second entry point rather than a parameter everywhere: a turn that
/// names nothing — every turn after the first — has no channel to select over,
/// and asking its callers to say so would be asking them to write `Titling::none`
/// at a dozen sites to say nothing at all.
#[allow(clippy::too_many_lines)]
pub(crate) async fn drive_turns_titled(
    service: &Arc<Service>,
    primary: EventStream,
    renderer: &mut Renderer,
    style: Style,
    mut input: Option<&mut crate::input::InteractiveInput>,
    mut titling: Titling,
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
    let mut turn_tick = tokio::time::interval(std::time::Duration::from_millis(TURN_TICK_MS));
    loop {
        while let Some(mut stream) = streams.pop_front() {
            loop {
                // The clock, so a call's row counts and the spinner turns
                // while the turn is waiting for the provider: an event is not
                // what time passing looks like.
                let _ = &mut turn_tick;
                let wake = if let Some(receiver) = input.as_deref_mut() {
                    tokio::select! {
                        biased;
                        Some(()) = interrupt_rx.recv() => Wake::Interrupt,
                        event = receiver.recv() => match event {
                            Some(EditorEvent::Line(line, attachments)) => {
                                Wake::Line(line, attachments)
                            }
                            Some(EditorEvent::Action(action)) => Wake::Action(action),
                            // The answer itself is read by the turn that asked
                            // the question, which is parked on it: this loop is
                            // driving a turn, and a question is not a line for
                            // it to steer.
                            Some(EditorEvent::Question(_)) => continue,
                            Some(EditorEvent::End) | None => Wake::Eof,
                        },
                        next = stream.next() => Wake::Event(Box::new(next)),
                        _ = turn_tick.tick() => Wake::Tick,
                        title = titling.next() => match title {
                            Some(title) => Wake::Title(title),
                            // The channel closed with nothing in it, which is a
                            // request that was never made. There is no title
                            // coming, so the arm stops being selected.
                            None => continue,
                        },
                    }
                } else {
                    tokio::select! {
                        biased;
                        Some(()) = interrupt_rx.recv() => Wake::Interrupt,
                        event = stream.next() => Wake::Event(Box::new(event)),
                        _ = turn_tick.tick() => Wake::Tick,
                        title = titling.next() => match title {
                            Some(title) => Wake::Title(title),
                            None => continue,
                        },
                    }
                };
                let event = match wake {
                    // The clock: a frame, and nothing was decided. It is what
                    // keeps a running call's counter moving and the spinner
                    // turning while the provider is silent.
                    Wake::Tick => {
                        renderer.draw()?;
                        continue;
                    }
                    // A keystroke the editor could not act on itself: it knows
                    // which key was pressed, not what a turn is. All three
                    // decisions are made here, where that is known.
                    Wake::Action(action) => {
                        match action {
                            EditorAction::CancelTurn => {
                                if let Some(session) = current.clone() {
                                    renderer.mark_cancelled();
                                    renderer.notice("cancelling the running turn…");
                                    let request = ClientRequest {
                                        request_id: 0,
                                        kind: ClientRequestKind::CancelTurn { session },
                                    };
                                    if let Err(error) = service.submit(request).await {
                                        renderer.notice(format!(
                                            "warning: could not cancel the turn: {error}"
                                        ));
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
                                    renderer.notice("no background job is running");
                                    continue;
                                }
                                stop_jobs(service, session, renderer).await;
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
                                    renderer.notice(format!(
                                        "warning: could not detach the turn: {error}"
                                    ));
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
                            return Err("interrupted".to_owned());
                        }
                        cancel_requested = true;
                        renderer.mark_cancelled();
                        renderer.notice("cancelling the running turn…");
                        renderer.notice(
                            "the running tool is being stopped; press Ctrl-C again to force quit",
                        );
                        let request = ClientRequest {
                            request_id: 0,
                            kind: ClientRequestKind::CancelTurn { session },
                        };
                        if let Err(error) = service.submit(request).await {
                            renderer.notice(format!("warning: could not cancel the turn: {error}"));
                        }
                        continue;
                    }
                    Wake::Line(line, attachments) => {
                        let text = line.trim().to_owned();
                        if text.is_empty() {
                            continue;
                        }
                        // Steering reaches the request in flight, and a request
                        // already built has no place for a part that was not in
                        // it. An attachment typed mid-turn is said rather than
                        // dropped: the next message can carry it.
                        if !attachments.is_empty() {
                            renderer.notice(
                                "an attached file cannot be steered into a running turn; \
                                 send it with the next message",
                            );
                        }
                        match parse_chat_command(&text) {
                            Some(Ok(command)) => {
                                handle_live_command(
                                    service,
                                    command,
                                    current.as_ref(),
                                    renderer,
                                    style,
                                )
                                .await;
                            }
                            Some(Err(message)) => renderer.notice(message),
                            None => {
                                let Some(session) = current.clone() else {
                                    renderer.notice("the session is not ready yet; try again");
                                    continue;
                                };
                                // The message is not in the history until the
                                // engine applies it, so it is not printed as if
                                // it were: it waits above the input, where the
                                // human can still see what they sent, and moves
                                // into the transcript when the engine reports
                                // that it landed.
                                if let Some(id) =
                                    steer_text(service, session, text.clone(), renderer).await
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
                    // The title was asked for beside this turn and has nothing
                    // to do with it: the footer gains a name, and a request that
                    // failed is said once at `high` and then forgotten. A
                    // session that could not be named keeps its key, which is
                    // what it had, and the turn it is running is undisturbed.
                    Wake::Title(Ok((_, title))) => {
                        renderer.set_title(title);
                        continue;
                    }
                    Wake::Title(Err(error)) => {
                        if renderer.verbosity.at_least(Verbosity::High) {
                            renderer.notice(format!("could not name the session: {error}"));
                        }
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
                        renderer.notice(
                            "approval left pending; the session is saved and can be resumed",
                        );
                        return Ok(DriveOutcome {
                            session: current,
                            parked: true,
                        });
                    }
                }
                if answer_if_asked(
                    service,
                    &renderer.pinned,
                    &event,
                    input.as_deref_mut(),
                    &mut interrupt_rx,
                )
                .await?
                {
                    renderer.mark_cancelled();
                    renderer
                        .notice("question left pending; the session is saved and can be resumed");
                    return Ok(DriveOutcome {
                        session: current,
                        parked: true,
                    });
                }
                renderer.event(&event)?;
                // One frame per event, and never on a timer: the frame is a
                // function of the state, and the state only moves when
                // something arrives.
                renderer.draw()?;
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
                                    renderer,
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
                                            Err(error) => renderer.notice(format!(
                                                "warning: budget raised but the session was not resumed: {error}"
                                            )),
                                        }
                                        }
                                        Err(error) => renderer.notice(format!(
                                            "warning: could not raise the budget: {error}"
                                        )),
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
                    renderer.notice(format!(
                        "warning: could not resolve approval {}: {error}",
                        short(&approval)
                    ));
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
                renderer.notice(format!("warning: could not apply the steer: {error}"));
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
    // A turn can move both lists: an approval can write a session rule, and a
    // server that dropped took its tools with it. The rows are rebuilt once
    // here rather than after every event that might have moved either.
    refresh_tab_summaries(service, current.as_ref(), &renderer.pinned, style);
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
        BudgetUnit, ChatCommand, JobListAction, McpAction, Reasoning, ReasoningScope, SignalGuard,
        command_suggestion, parse_budget_limit, parse_chat_command,
    };

    /// The usage line the registry writes for a command.
    fn usage_of(name: &str) -> Option<String> {
        crate::commands_registry::shared()
            .get(name)
            .map(norte_code_commands::CommandSpec::usage)
    }

    /// Whether the completion offers a name.
    ///
    /// These tests were written against a `const` list of names, and the bug
    /// they exist for was a command that was parsed and missing from it. Asking
    /// the registry is the same question and cannot go stale.
    fn offered(name: &str) -> bool {
        crate::commands_registry::shared()
            .commands()
            .iter()
            .any(|spec| spec.spellings().contains(&name))
    }

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
    fn tab_is_resolved_the_same_way_wherever_the_command_runs() {
        // `/tab` used to answer "tabs need an interactive terminal" during a
        // turn, which was false: the live handler runs inside the editor, with
        // the footer on screen. What it means is the same on both paths.
        assert_eq!(
            parse_chat_command("/tab todo"),
            Some(Ok(ChatCommand::Tab {
                target: Some("todo".to_owned())
            }))
        );
        assert_eq!(
            parse_chat_command("/tab 2"),
            Some(Ok(ChatCommand::Tab {
                target: Some("2".to_owned())
            }))
        );
        assert_eq!(
            parse_chat_command("/tab"),
            Some(Ok(ChatCommand::Tab { target: None }))
        );
    }

    #[test]
    fn rename_takes_the_whole_line_and_a_bare_one_asks_the_model() {
        // A name is not a word: everything after the command is the title, so
        // a name with spaces in it is the name and not a usage error.
        assert_eq!(
            parse_chat_command("/rename fix the footer overflow"),
            Some(Ok(ChatCommand::Rename {
                title: Some("fix the footer overflow".to_owned())
            }))
        );
        assert_eq!(
            parse_chat_command("/rename   spaced   out  "),
            Some(Ok(ChatCommand::Rename {
                title: Some("spaced   out".to_owned())
            }))
        );
        // Bare, it is the model that picks, which is the path a session that
        // predates titling takes.
        assert_eq!(
            parse_chat_command("/rename"),
            Some(Ok(ChatCommand::Rename { title: None }))
        );
        assert!(offered("/rename"));
        assert_eq!(usage_of("/rename"), Some("/rename [title]".to_owned()));
        // A word that merely starts with it is still a message.
        assert_eq!(parse_chat_command("/renamed"), None);
    }

    #[test]
    fn debug_is_a_command_and_takes_no_argument() {
        assert_eq!(parse_chat_command("/debug"), Some(Ok(ChatCommand::Debug)));
        // The command list and the completion both know it, so it is reachable
        // by typing `/deb` and pressing Tab.
        assert!(offered("/debug"));
        assert_eq!(usage_of("/debug"), Some("/debug".to_owned()));
        // A word after it is not an argument it takes, and it is refused
        // rather than sent: a command with text the client does not understand
        // is a mistake the person wants told about, not a message for the
        // model that happens to start with a slash.
        assert!(
            parse_chat_command("/debug now")
                .expect("a command")
                .is_err()
        );
        // A name that is not the command at all is still a message.
        assert_eq!(parse_chat_command("/debugs"), None);
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
        assert!(offered("/mcp"), "the list offers it");
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
        assert!(usage_of("/refresh").is_some());
        assert!(usage_of("/reload").is_some());
        assert!(offered("/refresh"), "the list offers it");
        assert!(offered("/reload"), "and its alias");
    }

    #[test]
    fn typing_part_of_an_effort_completes_it() {
        // The case the registry was built for: nobody should have to remember
        // that it is `xhigh` and not `x-high`, and Tab has to finish the word
        // rather than refuse it for being partial.
        let typed = command_suggestion("/reasoning xh", "/reasoning xh".len())
            .expect("the effort is offered");
        assert_eq!(typed.completion, "igh", "Tab finishes `xh` into `xhigh`");
        assert!(
            typed.hint.contains("xhigh"),
            "and the row says what it was: {typed:?}"
        );

        // A single command with nothing typed past the slash still lists them.
        let listed =
            command_suggestion("/reasoning ", "/reasoning ".len()).expect("the values are offered");
        assert!(listed.hint.contains("budget:<tokens>"), "{listed:?}");
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
            offered("/jobs") && offered("/restart"),
            "the list must offer both"
        );
        assert_eq!(
            usage_of("/jobs"),
            Some("/jobs [list|check|stop] [job-id]".to_owned())
        );
        assert_eq!(usage_of("/restart"), Some("/restart".to_owned()));
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
