//! The interactive turn loop: input, steering, cancellation and the
//! handoff of a turn's decisions.

use super::{
    Arc, Budget, BudgetAction, BudgetUnit, ClientRequest, ClientRequestKind, EventStream, JobInfo,
    Reasoning, ReasoningScope, Renderer, ServerEvent, ServerEventKind, Service, SessionKey,
    StreamExt, Style, VecDeque, mpsc,
};
use crate::input::{Attachment, CompletionNames, EditorAction, EditorEvent, InteractiveInput};

use crate::prompt::{PromptInput, answer_if_asked, prompt_input};
use crate::render::{
    BudgetPrompt, Verbosity, format_measure, mcp_panel, parse_reasoning_setting,
    reasoning_setting_label, report_mcp as render_mcp, report_prompt, report_reasoning,
    report_start, report_tools, scope_name, tools_panel, unit_name,
};
use norte_code_protocol::commands::Invocation;
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
use norte_code_protocol::JobState;

/// A validated interactive command.
///
/// Commands with arguments parse into closed variants. Anything that is not a
/// known command stays a normal message, so a prompt may still start with `/`.
/// What `/mcp` was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum McpAction {
    /// List the configured servers and their tools.
    List,
    /// Add a server, or replace one of the same name.
    Add {
        /// The name the server is known by.
        name: String,
        /// The program to run.
        command: String,
        /// Its arguments, in order.
        args: Vec<String>,
    },
    /// Turn a server on.
    Enable {
        /// The name of the server.
        name: String,
    },
    /// Turn a server off, keeping its entry.
    Disable {
        /// The name of the server.
        name: String,
    },
    /// Remove a server.
    Remove {
        /// The name of the server.
        name: String,
    },
}

#[derive(Debug, PartialEq)]
pub(crate) enum ChatCommand {
    /// Show the command list.
    Help,
    /// Summarise the session on demand.
    Compact,
    /// Re-read everything that can be re-read while the process runs.
    Refresh,
    /// List, add, remove or switch an MCP server.
    Mcp { action: McpAction },
    /// The telemetry panel: this session's ledger, and every provider's.
    Telemetry,
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
    /// Go back to choosing a provider and a model: the screen the process opens
    /// on, reachable from inside a session.
    Menu,
    /// Delete this session and leave the chat.
    Delete,

    /// Report what the process knows about itself: the model, its
    /// capabilities, the tools, the servers, the paths. Built when it is asked
    /// for, not captured at startup, so the tools are the ones registered now.
    Debug,
    /// A command a plugin declared.
    ///
    /// It arrives parsed and not as text: the registry that completed it is the
    /// same one that parsed it, so a plugin's command is a command like any
    /// other and the plugin is handed what it declared.
    Plugin(norte_code_protocol::commands::Invocation),
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
        "/menu" => ChatCommand::Menu,
        "/delete" => ChatCommand::Delete,
        "/debug" => ChatCommand::Debug,
        "/telemetry" => ChatCommand::Telemetry,
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
        "/model" => ChatCommand::Model {
            model: invocation.arg_or_empty("model").to_owned(),
        },
        "/provider" => ChatCommand::Provider {
            provider: invocation.arg_or_empty("provider").to_owned(),
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
    // The action is a closed choice, so a word that is not one falls through to
    // the trailing arguments. An action nobody gave with text after the command
    // is therefore an action nobody wrote: the first word is the mistake, and
    // saying so is better than listing for a person who typed `frobnicate`.
    let named = invocation.arg("action");
    let rest = invocation.arg("args").unwrap_or("");
    let action = match named {
        None if !rest.trim().is_empty() => {
            let first = rest.split_whitespace().next().unwrap_or("");
            return Err(format!(
                "unknown /mcp action `{first}`; use list, add, enable, disable or remove"
            ));
        }
        None => "list",
        Some(action) => action,
    };
    let args: Vec<String> = rest.split_whitespace().map(str::to_owned).collect();
    let action = match action {
        "list" => McpAction::List,
        "add" => {
            // `add <name> <command> [args…]`: the name and the command are the
            // two things a server cannot be without, and the rest is whatever
            // the program takes.
            let mut parts = args.into_iter();
            let (Some(name), Some(command)) = (parts.next(), parts.next()) else {
                return Err("usage: /mcp add <name> <command> [args…]".to_owned());
            };
            McpAction::Add {
                name,
                command,
                args: parts.collect(),
            }
        }
        "enable" | "disable" | "remove" => {
            let mut parts = args.into_iter();
            let Some(name) = parts.next() else {
                return Err(format!("usage: /mcp {action} <name>"));
            };
            match action {
                "enable" => McpAction::Enable { name },
                "disable" => McpAction::Disable { name },
                _ => McpAction::Remove { name },
            }
        }
        other => {
            return Err(format!(
                "unknown /mcp action `{other}`; use list, add, enable, disable or remove"
            ));
        }
    };
    Ok(ChatCommand::Mcp { action })
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
pub(crate) fn completion_rows(text: &str, cursor: usize, names: &CompletionNames) -> usize {
    candidates(text, cursor, names).len()
}

/// What Tab inserts for the row at `pick`.
///
/// `pick` is clamped to the rows that exist, because the keyboard sends it and
/// the line it was chosen for may be gone: completing to a row that is no
/// longer offered is worse than completing to the first one.
#[must_use]
pub(crate) fn completion_text(
    text: &str,
    cursor: usize,
    pick: usize,
    names: &CompletionNames,
) -> Option<String> {
    let rows = candidates(text, cursor, names);
    rows.get(pick.min(rows.len().saturating_sub(1)))
        .map(|row| row.completion.clone())
        .filter(|completion| !completion.is_empty())
}

/// Every candidate for the line: the registry's, and the catalogue's.
///
/// The registry answers for its own arguments — the efforts, the scopes, the
/// actions. `/provider` and `/model` take a name only the catalogue holds, and
/// the registry cannot carry it: it is built once at startup while the
/// catalogue is read again by `/refresh`, and one does not know about the
/// other. Those two are answered from the names the CLI pushed into
/// [`crate::input::Pinned`], and everything else is the registry's answer.
pub(crate) fn candidates(
    text: &str,
    cursor: usize,
    names: &CompletionNames,
) -> Vec<norte_code_protocol::commands::Candidate> {
    let rows = crate::commands_registry::shared().candidates(text, cursor);
    if !rows.is_empty() {
        return rows;
    }
    catalogue_candidates(text, cursor, names)
}

/// The names the two commands that take one from the catalogue offer.
///
/// The line is a command and one word being typed, which is the shape both
/// take: `/provider ri` offers the provider ids and `/model deep` the models of
/// the provider in play. The models are the bare ids, because `/model` takes a
/// model and the provider comes from the session rather than from the line.
fn catalogue_candidates(
    text: &str,
    cursor: usize,
    names: &CompletionNames,
) -> Vec<norte_code_protocol::commands::Candidate> {
    if cursor != text.len() {
        return Vec::new();
    }
    // A command with an argument already finished is not a line to complete:
    // there is no second provider and no second model, and the registry says
    // the same about its own arguments.
    let Some(space) = text.find(char::is_whitespace) else {
        return Vec::new();
    };
    let partial = text[space..].trim_start();
    if partial.contains(char::is_whitespace) {
        return Vec::new();
    }
    let (values, note) = match &text[..space] {
        "/provider" => (&names.providers, "provider"),
        "/model" => (&names.models, "model"),
        _ => return Vec::new(),
    };
    let prefix = partial.to_lowercase();
    values
        .iter()
        .filter(|value| value.to_lowercase().starts_with(&prefix))
        .map(|value| norte_code_protocol::commands::Candidate {
            text: value.clone(),
            // Skipping the characters already typed rather than slicing at a
            // byte offset: a name is not guaranteed to be ASCII, and a byte
            // offset in the middle of a character would panic on a keypress.
            completion: value.chars().skip(prefix.chars().count()).collect(),
            note: note.to_owned(),
        })
        .collect()
}

/// Records the names the completion offers for the provider in play.
///
/// The catalogue belongs to the service and the completion is answered on the
/// threads that read keys and draw the frame, neither of which can reach it, so
/// the names travel through the pinned handle. They are refreshed whenever the
/// answer changes: at startup, after `/refresh` re-read the catalogue, and after
/// a `/provider` or a `/model` moved the session.
pub(crate) fn refresh_completions(
    service: &Arc<Service>,
    session: Option<&SessionKey>,
    renderer: &mut Renderer,
) {
    let catalog = service.catalog();
    let mut providers: Vec<String> = catalog.providers().keys().cloned().collect();
    providers.sort();
    // The provider the next `/model` would land on, which is the one a `/model`
    // line completes from: the held one when `/provider` just moved and the
    // model did not come with it, and the session's own otherwise.
    let provider = renderer
        .pending_provider
        .clone()
        .or_else(|| session.and_then(|key| provider_of(service, key)))
        // A process with no session yet completes from the model it would open
        // on, which is the service's default: `/model` before the first message
        // lists that provider's models rather than none.
        .or_else(|| Some(service.default_spec().provider));
    let mut models: Vec<String> = provider
        .as_deref()
        .map(|provider| {
            catalog
                .models_of(provider)
                .map(|model| model.id.clone())
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    renderer.set_completion_names(CompletionNames { providers, models });
}

/// The one dimmed row shown above the input, for the line as it stands.
///
/// It is [`candidates`] reduced to what the editor draws: the first row's
/// completion, for Tab, and a hint naming what the candidates are. One row is a
/// hint and several are a list, which is why the hint is one name or many.
pub(crate) fn command_suggestion(
    text: &str,
    cursor: usize,
    names: &CompletionNames,
) -> Option<CommandSuggestion> {
    let rows = candidates(text, cursor, names);
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
                ChatCommand::Telemetry => {
                    report_telemetry(session.as_ref(), renderer).await;
                }
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
                ChatCommand::Refresh => report_refresh(service, session.as_ref(), renderer).await,
                ChatCommand::Rename { title } => match session.clone() {
                    Some(current) => {
                        rename_session(service, current, title, renderer).await;
                    }
                    None => renderer.notice("there is no session yet; send a message first"),
                },
                ChatCommand::Mcp { action } => {
                    report_mcp(service, action, renderer).await;
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
                ChatCommand::Model { model } => {
                    if model.is_empty() {
                        // List available models for the provider in play. It is
                        // the held provider when `/provider` just moved to one the
                        // current model cannot survive — that is the whole reason
                        // the list is being asked for — and the session's own
                        // provider otherwise. The renderer's name is the model in
                        // flight, and guessing `openai` when there is no session
                        // listed the wrong catalogue under the right heading.
                        let provider = renderer.pending_provider.clone().or_else(|| {
                            session
                                .as_ref()
                                .and_then(|current| provider_of(service, current))
                        });
                        let Some(provider) = provider else {
                            renderer.notice("there is no session yet; send a message first");
                            continue;
                        };
                        let catalog = service.catalog();
                        // The model list is a report: it is read and dismissed,
                        // and as lines of the conversation a catalogue of
                        // hundreds would bury the work.
                        let mut lines = vec![format!("Models for {provider}:")];
                        if catalog.provider(&provider).is_some() {
                            let mut models: Vec<String> = catalog
                                .models_of(&provider)
                                .map(|entry| entry.id.clone())
                                .collect();
                            models.sort();
                            for m in &models {
                                lines.push(format!("  {m}"));
                            }
                            if models.is_empty() {
                                // A provider with no models is one somebody added
                                // to `providers.json` and not yet to
                                // `models.json`, which is the state a plugin
                                // building one leaves the other in for a moment.
                                lines.push("  (no models; add one to models.json)".to_owned());
                            }
                        } else {
                            lines.push(format!(
                                "  (no provider named `{provider}`; see providers.json)"
                            ));
                        }
                        renderer.open_modal(format!("models of {provider}"), lines);
                    } else if let Some(current) = session.clone() {
                        // The model is a string and nothing else: it may carry
                        // slashes of its own — `nvidia/nemotron` is a model of
                        // the `nvidia` provider — so `pick_model` never takes it
                        // apart to find a provider. The provider is the
                        // session's, or the one `/provider` just moved to when
                        // that switch left the model unset.
                        let full_model = pick_model(service, &current, &model, renderer);
                        let Some(full_model) = full_model else {
                            continue;
                        };
                        renderer.model_name = Some(full_model.clone());
                        run_model(service, current, full_model, renderer).await;
                    } else {
                        renderer.notice("there is no session yet; send a message first");
                    }
                }
                ChatCommand::Provider { provider } => {
                    if provider.is_empty() {
                        // Every provider the catalogue carries, with the wires
                        // it serves and how many models it holds. There is no
                        // configured provider to merge in any more: the
                        // catalogue is the only place a provider comes from.
                        let catalog = service.catalog();
                        let mut lines = vec!["Providers:".to_owned()];
                        let mut providers: Vec<String> =
                            catalog.providers().keys().cloned().collect();
                        providers.sort();
                        for id in &providers {
                            let Some(entry) = catalog.provider(id) else {
                                continue;
                            };
                            let wires = entry.offered();
                            let models = catalog.models_of(id).count();
                            let plural = if models == 1 { "model" } else { "models" };
                            lines.push(format!(
                                "  {id} ({} · {models} {plural})",
                                crate::text::terminal(&wires)
                            ));
                        }
                        renderer.open_modal("providers", lines);
                    } else if let Some(current) = session.clone() {
                        // The same carrying-out the live turn uses: a provider
                        // switch the current model cannot survive is held and the
                        // model is left to the next `/model`, instead of sending a
                        // pair no provider lists. Left separate once, which is why
                        // the held state never appeared at a turn boundary and
                        // neither half could be moved.
                        switch_provider(service, &current, &provider, renderer).await;
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
                // Going back to the menu is the startup screen reached from
                // inside a session: it lists the providers and the models the
                // way the window before the session does, and the choice is
                // made with `/provider` and `/model`. It does not drop the
                // session: a screen that threw the conversation away to show a
                // list would be a worse answer than the list on its own, and
                // `/restart` is the command that starts a fresh one.
                ChatCommand::Menu => {
                    let catalog = service.catalog();
                    let current = session
                        .as_ref()
                        .and_then(|current| provider_of(service, current));
                    let mut lines = vec!["Choose a provider and a model:".to_owned()];
                    let mut providers: Vec<String> = catalog.providers().keys().cloned().collect();
                    providers.sort();
                    for provider in &providers {
                        let marked = if current.as_deref() == Some(provider.as_str()) {
                            "*"
                        } else {
                            " "
                        };
                        let mut models: Vec<String> = catalog
                            .models_of(provider)
                            .map(|entry| entry.id.clone())
                            .collect();
                        models.sort();
                        if models.is_empty() {
                            lines.push(format!("{marked} {provider} (no models listed)"));
                            continue;
                        }
                        lines.push(format!("{marked} {provider}"));
                        for model in &models {
                            lines.push(format!("    {model}"));
                        }
                    }
                    lines.push(String::new());
                    lines.push("pick one with /provider <id> and /model <id>".to_owned());
                    renderer.open_modal("menu", lines);
                }
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
                // The session goes and the chat goes with it. Nothing is left
                // to continue and nothing above it in the history names it any
                // more, so staying would leave a client on a session that no
                // longer exists. It is done here, at the turn boundary, and not
                // while a turn runs: the service is still writing to the
                // session being dropped.
                ChatCommand::Delete => {
                    let Some(key) = session.clone() else {
                        renderer.notice("there is no session to delete yet; send a message first");
                        continue;
                    };
                    match delete_current(service, &key).await {
                        Ok(()) => {
                            // Cleared before the break, so the caller cannot
                            // write the session back: a session deleted here
                            // and saved on the way out would come back, and the
                            // next start would offer a conversation the person
                            // asked to be rid of.
                            session = None;
                            renderer.notice("session deleted; leaving");
                            break;
                        }
                        Err(error) => {
                            renderer.failure(format!("could not delete the session: {error}"));
                            continue;
                        }
                    }
                }
            }
            continue;
        }
        // A provider was switched and no model has been chosen for it yet. There
        // is nothing to send the message to — the session is still on its old
        // model, and the new provider does not serve it — so the message is
        // refused here rather than sent to a provider that would reject it.
        if let Some(provider) = renderer.pending_provider.clone() {
            renderer.said(&text);
            renderer.failure(format!(
                "no model set for `{provider}`; pick one with /model"
            ));
            continue;
        }
        // The message is the conversation, so it goes to the transcript and the
        // frame draws it where the conversation is. Printed onto the terminal,
        // it landed on top of the input.
        renderer.said(&text);
        renderer.mark_working(); // The first message of a session that has no title is what the title is
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
        ChatCommand::Exit => renderer.notice("wait until the current turn finishes, then /exit"),
        ChatCommand::Help => print_chat_help(renderer, style),
        // The report needs no turn boundary: it reads state that is already
        // settled, so it is the one command that works mid-turn.
        ChatCommand::Debug => report_debug(service, current, renderer, style),
        ChatCommand::Tools => report_tools_now(service, current, renderer, style),
        ChatCommand::Telemetry => {
            report_telemetry(current, renderer).await;
        }
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
        ChatCommand::Refresh => report_refresh(service, current, renderer).await,
        ChatCommand::Mcp { action } => {
            report_mcp(service, action, renderer).await;
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
                let full_model = pick_model(service, session, &model, renderer);
                if let Some(full_model) = full_model {
                    renderer.model_name = Some(full_model.clone());
                    run_model(service, session.clone(), full_model, renderer).await;
                }
            } else {
                renderer.notice("the session is not ready yet; try again");
            }
        }
        ChatCommand::Provider { provider } => {
            if provider.is_empty() {
                renderer.notice("wait until the current turn finishes, then /provider");
            } else if let Some(session) = current {
                switch_provider(service, session, &provider, renderer).await;
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
        // A delete ends the session, so it cannot wait for the turn either: the
        // service is writing to the session being deleted.
        ChatCommand::Delete => {
            renderer.notice("wait until the current turn finishes, then /delete");
        }
        // `/menu` reads the catalogue and the session's provider, and both are
        // settled between turns: mid-turn the list would be the one the running
        // turn is already answering from, which is the list that helps least.
        ChatCommand::Menu => {
            renderer.notice("wait until the current turn finishes, then /menu");
        }
    }
}

/// Deletes a session through the service, so every row goes with it.
///
/// The request is the wire's own `DeleteSession`, which is the path a delete
/// elsewhere takes: the service drops the session from the directory it holds
/// and the store clears the logical row, the Norte session, the node chain and
/// the ledgers keyed by either id. The tables carry no foreign keys, so a delete
/// that cleared one row alone is what left requests pointing at a session the
/// list no longer had.
async fn delete_current(service: &Arc<Service>, session: &SessionKey) -> Result<(), String> {
    let request = ClientRequestKind::DeleteSession {
        session: session.clone(),
    };
    let events = drain_request(service, request).await?;
    if let Some(error) = stream_error(&events) {
        return Err(error);
    }
    Ok(())
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
    // lines written onto it land on top of the conversation. This one is also
    // where a rule is set, which is what `tools_panel` builds the rows for: a
    // person reading the list is one key from changing what it says, instead of
    // typing `/allow shell global` from memory while looking at the list that
    // names the tool.
    let rows = tools_panel(style, service, session);
    renderer.open_editable("tools", rows);
}

/// Every MCP server the sidebar names: the ones switched on.
///
/// A server configured off is left out, not drawn as a dead row. It is off on
/// purpose and a row that kept saying so took a line of the column for the life
/// of the process, next to the ones that are actually up; the `/mcp` panel is
/// where a person goes to see and switch every entry, including the off ones.
/// A server that failed to start stays: it is configured on and not running,
/// which is exactly what the column should warn about.
pub(crate) fn mcp_rows(service: &norte_code_agent::Service) -> Vec<crate::input::McpRow> {
    use crate::input::{McpRow, McpState};
    let status = service.mcp_status();
    let failures = service.mcp_failures();
    service
        .config()
        .mcp_servers
        .iter()
        .filter(|(_, config)| config.enabled)
        .map(|(name, _)| {
            let running = status.iter().find(|server| &server.name == name);
            let state = if let Some(server) = running {
                if server.active {
                    McpState::On
                } else {
                    McpState::Off
                }
            } else if let Some((_, reason)) = failures.iter().find(|(failed, _)| failed == name) {
                McpState::Failed(reason.clone())
            } else {
                // Configured on and neither running nor failed: it is trying, or
                // it was never reached. Off is the honest word for that.
                McpState::Off
            };
            McpRow {
                name: name.clone(),
                tools: running.map_or(0, |server| server.tools.len()),
                state,
            }
        })
        .collect()
}

/// Opens the telemetry panel: this session's ledger and every provider's.
///
/// The rows come from the same store `norte-code telemetry` reads, opened here
/// as a reader: a client that is mid-turn has a writer of its own on the same
/// file, and WAL lets the two share it. A store that cannot be opened is not a
/// failure of the client — a machine that has never run a request has no
/// database — so it opens an empty panel and says so.
async fn report_telemetry(session: Option<&SessionKey>, renderer: &mut Renderer) {
    let records = match read_telemetry().await {
        Ok(records) => records,
        Err(error) => {
            renderer.failure(format!("could not read the request ledger: {error}"));
            return;
        }
    };
    let session_key = session.map(ToString::to_string);
    renderer.open_telemetry(crate::telemetry_panel::Panel::new(records, session_key));
}

/// Every recorded request row, from the store the client already uses.
async fn read_telemetry() -> Result<Vec<norte_code_protocol::sqlite::RequestRecord>, String> {
    use norte_code_protocol::sqlite::SessionDirectory as _;
    let path = crate::commands::database_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let store = norte_code_protocol::sqlite::SqliteStore::open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    store
        .requests(None)
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))
}

/// Carries out what the tools panel decided.
///
/// The panel produces the intent and this is where it is carried out, because
/// the service is here and the panel has no idea one exists. The panel is
/// rebuilt after every decision and opened again, so the row a person just
/// changed says what it became — a rule that changed with the list still saying
/// the old word is a list nobody can trust.
async fn apply_tool_action(
    service: &Arc<Service>,
    session: Option<&SessionKey>,
    renderer: &mut Renderer,
    style: Style,
    action: crate::modal::Action,
    scope: crate::modal::Scope,
) {
    let global = scope == crate::modal::Scope::Global;
    // A rule is set or forgotten, and which of the two the key asks for is the
    // action the row carried. The two are one call here because the panel shows
    // one row per tool: setting an allow and setting a deny are the same write
    // to a different list, and forgetting removes the tool from both.
    let (tool, rule) = match &action {
        crate::modal::Action::Allow(tool) => (tool.clone(), Some(true)),
        crate::modal::Action::Deny(tool) => (tool.clone(), Some(false)),
        crate::modal::Action::Forget(tool) => (tool.clone(), None),
        // The MCP panel's own action: a server is switched and not a rule set,
        // so it takes its own path.
        crate::modal::Action::Toggle(name) => {
            apply_mcp_action(
                service,
                renderer,
                &crate::modal::Action::Toggle(name.clone()),
            );
            return;
        }
    };
    let outcome = match rule {
        Some(allow) => {
            service
                .set_approval_rule(session, &tool, allow, global)
                .await
        }
        None => service.forget_approval_rule(session, &tool, global).await,
    };
    match outcome {
        Ok(()) => {
            let reach = if global {
                "globally"
            } else {
                "for this session"
            };
            match rule {
                Some(true) => renderer.notice(format!("allowed `{tool}` {reach}")),
                Some(false) => renderer.notice(format!("denied `{tool}` {reach}")),
                None => renderer.notice(format!("`{tool}` asks again {reach}")),
            }
            // The list is rebuilt from the rules, so the row says what the rule
            // now is: a panel that kept its old text after a change is a panel
            // that lies about what a key did.
            let rows = tools_panel(style, service, session);
            if rows.is_empty() {
                renderer.close_modal();
            } else {
                renderer.open_editable("tools", rows);
            }
        }
        Err(error) => {
            renderer.failure(format!("could not change the rule for `{tool}`: {error}"));
        }
    }
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
    lines.extend(report_tools(style, service, session));
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

/// The `provider/model` a `/model` names, given the half the person did not type.
///
/// The model is a string and nothing else: it may carry slashes of its own —
/// `nvidia/nemotron` is a model of the `nvidia` provider — so this never takes
/// the typed name apart to find a provider in it. The provider is the session's,
/// or the one `/provider` just moved to when that switch left the model unset,
/// which is what makes `/provider exa` then `/model x` land on `exa/x`.
///
/// Returns `None` when there is nowhere for the name to land: a session with no
/// provider yet, which is the one case a person has to answer with `/provider`
/// first.
fn pick_model(
    service: &Arc<Service>,
    session: &SessionKey,
    model: &str,
    renderer: &mut Renderer,
) -> Option<String> {
    let provider = renderer
        .pending_provider
        .clone()
        .or_else(|| provider_of(service, session));
    let Some(provider) = provider else {
        renderer.notice("the session has no provider yet; pick one with /provider");
        return None;
    };
    Some(format!("{provider}/{model}"))
}

/// What a `/provider` asks for, once the catalogue has been asked about it.
///
/// The decision is a value and not a set of branches inside the command, so it
/// can be checked without a service: the four outcomes are the whole of what a
/// provider switch can be, and the command below is only the carrying-out.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ProviderSwitch {
    /// The new provider serves the model already in play: switch to it.
    Move(String),
    /// The new provider does not serve it. The provider is held and the model
    /// is left for the next `/model`.
    Hold,
    /// The session has no model to carry.
    NoModel,
    /// The catalogue does not list the provider.
    Unknown,
}

/// What `/provider` should do, from the three facts the catalogue answers.
///
/// It takes booleans rather than the catalogue so the four cases are enumerated
/// in one place instead of inferred from control flow at the call site.
fn plan_provider_switch(known: bool, model: Option<&str>, serves: bool) -> ProviderSwitch {
    if !known {
        return ProviderSwitch::Unknown;
    }
    let Some(model) = model else {
        return ProviderSwitch::NoModel;
    };
    if serves {
        ProviderSwitch::Move(model.to_owned())
    } else {
        ProviderSwitch::Hold
    }
}

/// Moves the session to `provider`, or leaves its model to be chosen.
///
/// The switch is one request when the model survives it — the same model id is
/// served by the new provider too — and nothing is sent when it does not: a
/// provider does not serve another's model, and `exa/river-model` is a request
/// no provider would answer. Instead the provider is remembered and the model
/// is left unset, so the next `/model` names one for it and a message sent in
/// the meantime is refused rather than refused by the vendor.
///
/// That unset state is the client's own and never reaches the service: a
/// session is not committed without a model, and closing here reopens on the
/// model the session already had.
async fn switch_provider(
    service: &Arc<Service>,
    session: &SessionKey,
    provider: &str,
    renderer: &mut Renderer,
) {
    let model_name = model_name_of(service, session);
    let catalog = service.catalog();
    let known = catalog.provider(provider).is_some();
    let serves = model_name
        .as_deref()
        .is_some_and(|model| catalog.find(provider, model).is_some());
    match plan_provider_switch(known, model_name.as_deref(), serves) {
        ProviderSwitch::Unknown => {
            let mut names: Vec<String> = catalog.providers().keys().cloned().collect();
            names.sort();
            renderer.failure(format!(
                "unknown provider `{provider}`; pick one of: {}",
                names.join(", ")
            ));
        }
        ProviderSwitch::NoModel => {
            renderer.notice("the session has no model yet; pick one with /model");
        }
        // A model the new provider does not serve is the case this exists for:
        // the switch is held and the model is left to the next `/model`.
        ProviderSwitch::Hold => {
            renderer.pending_provider = Some(provider.to_owned());
            renderer.model_name = Some(provider.to_owned());
            renderer.notice(format!(
                "`{provider}` does not serve `{}`; pick one with /model",
                model_name.unwrap_or_default()
            ));
        }
        ProviderSwitch::Move(model) => {
            let full = format!("{provider}/{model}");
            renderer.pending_provider = None;
            renderer.model_name = Some(full.clone());
            run_model(service, session.clone(), full, renderer).await;
        }
    }
    // The completion follows the session: a `/model` line now offers the models
    // of the provider just moved to, which is the list the message above told
    // the person to pick from.
    refresh_completions(service, Some(session), renderer);
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
                        // The provider the switch was waiting on is settled once
                        // the model is: from here a message has somewhere to go.
                        renderer.pending_provider = None;
                        renderer.notice(format!("model set to {model}"));
                        // A new model can refuse the effort in force — the
                        // service downgrades it to one the model declares — so
                        // the value the footer shows is recomputed rather than
                        // left describing the old model.
                        refresh_effort(service, &key, renderer);
                        // The completion follows too: the model just changed, so a
                        // `/model` line offers the models of the provider it is on.
                        refresh_completions(service, Some(&key), renderer);
                    }
                    ServerEventKind::ModelQueued { model } => {
                        reported = true;
                        renderer.pending_provider = None;
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
                        job_state_label(job.state, crate::style::Style::plain()),
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
        job_state_label(job.state, crate::style::Style::plain()),
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
///
/// The match is over the enum and has no catch-all arm: a state the protocol
/// grows is a compile error here, which is what a color chosen for it needs to
/// be. Matching on the string meant a new state fell into `_` and was drawn as
/// a finished job.
fn job_state_label(state: JobState, style: Style) -> String {
    let word = state.label();
    match state {
        JobState::Running => style.cyan(word.to_owned()),
        JobState::Finished => style.green(word.to_owned()),
        JobState::Cancelled => style.yellow(word.to_owned()),
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
async fn report_refresh(
    service: &Arc<Service>,
    session: Option<&SessionKey>,
    renderer: &mut Renderer,
) {
    report_reload(service, renderer);
    let count = service.refresh_skills();
    renderer.notice(format!("skills refreshed: {count}"));
    // The connections are rebuilt, not merely retried: a server added, removed
    // or disabled in the file takes effect here, and one that is still failing
    // comes back in the failures list. A retry alone could only revive what was
    // already configured when the process started, which is why a new server
    // needed a restart.
    let up = service.reload_mcp().await;
    let failed = service.mcp_failures();
    renderer.notice(format!("mcp servers connected: {up}"));
    if !failed.is_empty() {
        renderer.notice(format!("{} mcp server(s) unreachable", failed.len()));
    }
    // The sidebar follows: it is a column of state, and the state just changed.
    renderer.set_mcp(mcp_rows(service));
    // And so does the completion: `/refresh` is what re-reads the catalogue, so
    // a model added to `models.json` is one the next `/model` offers.
    refresh_completions(service, session, renderer);
}

/// Rebuilds the MCP connections and reports how many are up.
///
/// It is what every write to the MCP configuration ends with: a switch or an
/// entry that is written but not connected is a change the person cannot see
/// working, and the reload is what makes the file and the process agree.
async fn reload_mcp(service: &Arc<Service>, renderer: &mut Renderer) {
    let up = service.reload_mcp().await;
    let failed = service.mcp_failures();
    renderer.notice(format!("mcp servers connected: {up}"));
    for (name, reason) in failed {
        renderer.notice(format!("`{name}` did not start: {reason}"));
    }
    // The sidebar follows: it is a column of state, and the state just changed.
    renderer.set_mcp(mcp_rows(service));
}

/// Lists, adds, removes or switches an MCP server.
///
/// `enable`/`disable` write the flag into the config file and reload the
/// connections, so the choice outlives the process: a server switched off here
/// is off in the next run too. `add` and `remove` write the entry and reload, so
/// what a person typed is what connects. `list` opens the panel, where the same
/// switch is one key.
async fn report_mcp(service: &Arc<Service>, action: McpAction, renderer: &mut Renderer) {
    let path = service
        .config_path()
        .or_else(norte_code_protocol::config::user_config_path);
    match &action {
        McpAction::List => {}
        McpAction::Add {
            name,
            command,
            args,
        } => {
            let Some(path) = path else {
                renderer.failure("no user config path; pass --config");
                return;
            };
            match norte_code_protocol::config::add_user_mcp_server(&path, name, command, args) {
                Ok(()) => {
                    renderer.notice(format!("added mcp server `{name}`"));
                    reload_mcp(service, renderer).await;
                }
                Err(error) => renderer.failure(format!("could not add `{name}`: {error}")),
            }
            return;
        }
        McpAction::Remove { name } => {
            let Some(path) = path else {
                renderer.failure("no user config path; pass --config");
                return;
            };
            match norte_code_protocol::config::remove_user_mcp_server(&path, name) {
                Ok(()) => {
                    renderer.notice(format!("removed mcp server `{name}`"));
                    reload_mcp(service, renderer).await;
                }
                Err(error) => renderer.failure(format!("could not remove `{name}`: {error}")),
            }
            return;
        }
        McpAction::Enable { name } | McpAction::Disable { name } => {
            let enabled = matches!(action, McpAction::Enable { .. });
            // The switch is written first: a file that cannot be written is what
            // the person needs to know, and switching the process alone would
            // say it worked when the next run would undo it.
            if let Some(path) = path {
                if let Err(error) =
                    norte_code_protocol::config::set_user_mcp_enabled(&path, name, enabled)
                {
                    renderer.failure(format!("could not change `{name}`: {error}"));
                    return;
                }
            }
            let word = if enabled { "enabled" } else { "disabled" };
            renderer.notice(format!("mcp server `{name}` {word}"));
            if !enabled {
                renderer.notice("its tools leave the next turn; the process stays up");
            }
            // The file changed, so the configuration in memory is stale: a
            // reload reads it back before the connections are rebuilt. Without
            // this the sidebar was filtered from the old `enabled` flag and a
            // server switched off here kept its row until a `/refresh`.
            if let Err(error) = service.reload_config() {
                renderer.notice(format!("warning: could not reload the config: {error}"));
            }
            reload_mcp(service, renderer).await;
            return;
        }
    }

    // The listing is a panel and not a print, and it is one a person can act
    // on: reading the list that names a server is the moment they know which
    // one to switch off, and typing `/mcp disable <name>` from memory while
    // looking at it is a command line doing a panel's job.
    let rows = mcp_panel(service);
    if rows.is_empty() {
        renderer.notice("no MCP server connected");
        return;
    }
    renderer.open_editable("mcp servers", rows);
}

/// Carries out what the MCP panel decided.
///
/// The panel produces the intent and this is where it is carried out. The panel
/// is rebuilt after every switch and opened again, so the row says what the
/// server became: a list that kept the word it had is a list that lies about
/// what a key did.
fn apply_mcp_action(
    service: &Arc<Service>,
    renderer: &mut Renderer,
    action: &crate::modal::Action,
) {
    let crate::modal::Action::Toggle(name) = action else {
        return;
    };
    let active = service
        .mcp_status()
        .iter()
        .find(|server| server.name == *name)
        .is_some_and(|server| !server.active);
    // The switch is written to the config file, not only the process: a server
    // turned off in the panel is off in the next run too, which is what makes
    // the key mean something beyond this session.
    if let Some(path) = service
        .config_path()
        .or_else(norte_code_protocol::config::user_config_path)
    {
        if let Err(error) = norte_code_protocol::config::set_user_mcp_enabled(&path, name, active) {
            renderer.failure(format!("could not change `{name}`: {error}"));
            return;
        }
    }
    match service.set_mcp_active(name, active) {
        Some(state) => {
            let word = if state { "enabled" } else { "disabled" };
            renderer.notice(format!("mcp server `{name}` {word}"));
            if !state {
                renderer.notice("its tools leave the next turn; the process stays up");
            }
            renderer.set_mcp(mcp_rows(service));
            let rows = mcp_panel(service);
            if rows.is_empty() {
                renderer.close_modal();
            } else {
                renderer.open_editable("mcp servers", rows);
            }
        }
        None => renderer.failure(format!("no mcp server named `{name}` connected")),
    }
}

/// Re-reads the configuration and reports what happened.
///
/// The per-session rules and the model a session already resolved are session
/// state and are not rebuilt, so the notice names what needs a new session.
fn report_reload(service: &Arc<Service>, renderer: &mut Renderer) {
    match service.reload_config() {
        Ok(reloaded) => {
            // The dial in the status row is drawn from the threshold and the
            // window, and both are read from the configuration. They were set
            // once at startup, so a threshold written into the file and picked
            // up by `/refresh` changed how the session compacts without
            // changing the colour the bar warns in: the two disagreed, which is
            // the one thing a warning must not do.
            renderer.compaction_threshold = reloaded.config.compaction.threshold;
            // The window comes from the spec, which a `/model` may have moved:
            // a session switched to a model with a smaller window has a dial
            // that fills faster, and reading it from the startup spec would
            // leave the bar lying about how close the compaction is.
            renderer.max_context = service.default_spec().capabilities.max_context;
            // The dial's colour and the effort bar are the renderer's own state,
            // and no pinned text carries them: without this the new threshold
            // would redraw its colour only on the next keystroke, which is the
            // one moment a person who just ran `/refresh` is not typing.
            renderer.mark_dirty();
            // The catalogue was re-read from disk by reload_config: a person who
            // added a model to models.json and typed /reload expects the next
            // /model to find it, and the files are the only place a model comes
            // from. Warnings from a malformed catalogue file are already in
            // reloaded.warnings.
            let catalog_note = format!(", {} models", service.catalog().model_count());
            renderer.notice(format!("configuration reloaded{catalog_note}"));
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
        .map(norte_code_protocol::commands::CommandSpec::usage)
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
            // The clock is what keeps an animation moving — the spinner, and
            // the seconds a running call counts — and what puts a keystroke on
            // screen: a key is not a server event, so without a tick the draft
            // would appear only when something else happened to arrive.
            //
            // It draws only when there is a reason to. The frame used to be
            // written every 120ms whatever had happened, which is a cursor
            // hide/show pair sixteen times a second and a core burned to draw
            // the screen it had just drawn.
            _ = tick.tick() => {
                if renderer.animating() {
                    renderer.tick_modal();
                }
                if renderer.animating() || renderer.take_dirty() {
                    renderer.draw()?;
                }
                continue;
            }
        };
        // A keystroke changed the draft, and the draft is what the frame shows.
        renderer.draw()?;
        let (line, attachments) = match event {
            Some(EditorEvent::Line(line, attachments)) => (line, attachments),
            // A selection was copied: the terminal has it, and the conversation
            // says so, because a copy that reported nothing is one a person
            // cannot tell from a drag that missed.
            Some(EditorEvent::Copied(text)) => {
                copied(&text, renderer);
                continue;
            }
            Some(EditorEvent::End) | None => {
                return Ok(None);
            }
            // A panel key at the prompt: the panel decides what it means and
            // this carries the decision out. It is `async`, so it can: the
            // caller is the loop that owns the service, and asking it to make
            // the call would be a round trip for something already here.
            Some(EditorEvent::Action(EditorAction::Panel(press))) => {
                if let Some((decision, scope)) = renderer.modal_key(press) {
                    apply_tool_action(service, session, renderer, style, decision, scope).await;
                }
                renderer.draw()?;
                continue;
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

/// Puts a selection on the terminal's clipboard and says what went there.
///
/// OSC 52 is the write; the sentence is the receipt. A copy that said nothing
/// would be a copy a person cannot tell from a drag that missed, and the one
/// thing they need after selecting a long answer is to know it landed.
fn copied(text: &str, renderer: &mut Renderer) {
    match crate::terminal::copy(text) {
        Ok(()) => {
            let lines = text.lines().count();
            let chars = text.chars().count();
            renderer.notice(format!("copied {chars} character(s), {lines} line(s)"));
        }
        Err(error) => renderer.failure(format!("could not copy the selection: {error}")),
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
                            Some(EditorEvent::Copied(text)) => {
                                copied(&text, renderer);
                                continue;
                            }
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
                        if renderer.animating() || renderer.take_dirty() {
                            renderer.draw()?;
                        }
                        continue;
                    }
                    // A keystroke the editor could not act on itself: it knows
                    // which key was pressed, not what a turn is. All three
                    // decisions are made here, where that is known.
                    Wake::Action(action) => {
                        match action {
                            // A key the panel owns: the panel decides what it
                            // means and the loop carries the decision out,
                            // because the service is here and the panel has no
                            // idea one exists.
                            EditorAction::Panel(press) => {
                                if let Some((decision, scope)) = renderer.modal_key(press) {
                                    apply_tool_action(
                                        service,
                                        current.as_ref(),
                                        renderer,
                                        style,
                                        decision,
                                        scope,
                                    )
                                    .await;
                                }
                                renderer.draw()?;
                                continue;
                            }
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
                        committed: done, ..
                    } => {
                        parked = false;
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
                let pinned = Arc::clone(&renderer.pinned);
                if answer_if_asked(
                    service,
                    &pinned,
                    &event,
                    input.as_deref_mut(),
                    &mut interrupt_rx,
                    &mut || renderer.draw(),
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
    // A turn can move both lists: it can write a session rule, and a server
    // that dropped took its tools with it. The rows are rebuilt once
    // here rather than after every event that might have moved either.
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
        BudgetUnit, ChatCommand, JobListAction, McpAction, ProviderSwitch, Reasoning,
        ReasoningScope, SignalGuard, command_suggestion, interactive_loop, mpsc,
        parse_budget_limit, parse_chat_command, plan_provider_switch, refresh_completions,
        switch_provider,
    };
    use crate::input::{CompletionNames, EditorEvent, InteractiveInput, Pinned};
    use crate::render::{Format, Renderer, Verbosity};
    use crate::style::Style;
    use norte_code_protocol::ClientRequestKind;
    use norte_code_protocol::catalog::WireSchema;

    use super::drain_request;

    #[test]
    fn a_provider_switch_moves_only_when_the_model_survives_it() {
        // The three cases the command has, as values: the model is served (move),
        // it is not (hold the provider), and there is nothing to carry. Before
        // this, `/provider` kept the model and sent the pair whatever the
        // catalogue said, so a provider that does not serve another's model got
        // a request it would refuse — and there was no way out of the state,
        // because `/model` put the old provider back on.
        assert_eq!(
            plan_provider_switch(true, Some("shared-model"), true),
            ProviderSwitch::Move("shared-model".to_owned())
        );
        assert_eq!(
            plan_provider_switch(true, Some("other-providers-model"), false),
            ProviderSwitch::Hold
        );
        assert_eq!(
            plan_provider_switch(true, None, false),
            ProviderSwitch::NoModel
        );
    }

    #[test]
    fn an_unknown_provider_is_decided_before_the_model_is_looked_at() {
        // `serves` is a question about a provider that exists. Asking it about
        // one the catalogue does not list would answer "no" and hold a switch to
        // nowhere, which reads as the provider being real and the model wrong.
        assert_eq!(
            plan_provider_switch(false, Some("m"), false),
            ProviderSwitch::Unknown
        );
        assert_eq!(
            plan_provider_switch(false, None, false),
            ProviderSwitch::Unknown
        );
    }

    /// The names a test offers: none, so the registry alone answers.
    ///
    /// The catalogue's own names are what `the_catalogue_names_complete` adds:
    /// every other test is asking about the registry.
    fn no_names() -> CompletionNames {
        CompletionNames::default()
    }

    /// The names a test offers, as the CLI would push them: one provider, two
    /// models of it.
    fn catalogue() -> CompletionNames {
        CompletionNames {
            providers: vec!["exa".to_owned(), "river".to_owned()],
            models: vec![
                "deepseek-ai/DeepSeek-V4.1-Flash".to_owned(),
                "deepseek-v4.1-flash-uncensored".to_owned(),
            ],
        }
    }

    #[test]
    fn the_catalogue_names_complete_for_the_two_commands_that_take_one() {
        // `/provider` and `/model` take a name only the catalogue holds, and
        // neither offered anything: the registry is built at startup and cannot
        // read the catalogue, which `/refresh` re-reads. A person typed the id
        // from memory or not at all.
        let names = catalogue();
        let providers = command_suggestion("/provider ri", "/provider ri".len(), &names)
            .expect("the providers are offered");
        assert_eq!(
            providers.completion, "ver",
            "Tab finishes `ri` into `river`"
        );
        assert!(providers.hint.contains("river"), "{providers:?}");

        let models = command_suggestion("/model deep", "/model deep".len(), &names)
            .expect("the models are offered");
        assert!(
            models.hint.contains("deepseek-ai/DeepSeek-V4.1-Flash"),
            "a model id may carry slashes of its own: {models:?}"
        );
        // The completion appends only what was not typed, so a name with a
        // slash in it is completed as it is rather than cut at the slash.
        assert_eq!(models.completion, "seek-ai/DeepSeek-V4.1-Flash");
    }

    #[test]
    fn a_catalogue_name_is_not_offered_where_the_registry_answers() {
        // The catalogue is the fallback, not a second opinion: `/reasoning` and
        // every other command complete from the registry, and a provider id
        // typed after one is not a name it offers.
        let names = catalogue();
        let efforts = command_suggestion("/reasoning h", "/reasoning h".len(), &names)
            .expect("the registry answers");
        assert_eq!(efforts.completion, "igh");
        // And with no names pushed — the state before the first catalogue read —
        // the two commands offer nothing rather than an empty list.
        assert!(command_suggestion("/provider r", "/provider r".len(), &no_names()).is_none());
        assert!(command_suggestion("/model d", "/model d".len(), &no_names()).is_none());
    }

    #[test]
    fn a_command_with_its_argument_already_given_offers_nothing_more() {
        // There is one provider and one model on the line: offering a second
        // would complete a word the parser then refuses.
        let names = catalogue();
        for line in ["/provider river ", "/model deepseek-v4.1-flash "] {
            assert!(
                command_suggestion(line, line.len(), &names).is_none(),
                "`{line}` is finished"
            );
        }
    }

    /// The usage line the registry writes for a command.
    fn usage_of(name: &str) -> Option<String> {
        crate::commands_registry::shared()
            .get(name)
            .map(norte_code_protocol::commands::CommandSpec::usage)
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
    fn delete_is_a_command_and_takes_no_argument() {
        // `/delete` removes the session the chat is on and leaves. It is spelled
        // out because it is not undoable from here: everything else a person
        // types beginning with a slash is refused rather than sent as a message,
        // and a delete that reached the model as text would be a message about
        // deleting rather than the deletion.
        assert_eq!(parse_chat_command("/delete"), Some(Ok(ChatCommand::Delete)));
        // An argument is refused by the registry, which names the word it did
        // not expect and prints the usage: a delete is not something to guess
        // at, and a silent ignore would run it on the wrong reading of what
        // was typed.
        assert!(
            matches!(
                parse_chat_command("/delete now"),
                Some(Err(message)) if message.contains("no `now`") && message.contains("usage: /delete")
            ),
            "an argument is refused: {:?}",
            parse_chat_command("/delete now")
        );
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
                action: McpAction::List
            }))
        );
        // Each action, with the server it acts on.
        for (text, action) in [
            (
                "/mcp enable everything",
                McpAction::Enable {
                    name: "everything".to_owned(),
                },
            ),
            (
                "/mcp disable everything",
                McpAction::Disable {
                    name: "everything".to_owned(),
                },
            ),
            (
                "/mcp remove everything",
                McpAction::Remove {
                    name: "everything".to_owned(),
                },
            ),
        ] {
            assert_eq!(
                parse_chat_command(text),
                Some(Ok(ChatCommand::Mcp { action })),
                "{text}"
            );
        }
        // `add` takes a name, a command and its arguments.
        assert_eq!(
            parse_chat_command("/mcp add context7 npx -y @upstash/context7-mcp"),
            Some(Ok(ChatCommand::Mcp {
                action: McpAction::Add {
                    name: "context7".to_owned(),
                    command: "npx".to_owned(),
                    args: vec!["-y".to_owned(), "@upstash/context7-mcp".to_owned()],
                },
            }))
        );
        // A switch without a server, an `add` without a command, and an unknown
        // action are refused rather than sent to the model as a message.
        assert!(matches!(parse_chat_command("/mcp enable"), Some(Err(_))));
        assert!(matches!(
            parse_chat_command("/mcp add onlyname"),
            Some(Err(_))
        ));
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
        let typed = command_suggestion("/reasoning xh", "/reasoning xh".len(), &no_names())
            .expect("the effort is offered");
        assert_eq!(typed.completion, "igh", "Tab finishes `xh` into `xhigh`");
        assert!(
            typed.hint.contains("xhigh"),
            "and the row says what it was: {typed:?}"
        );

        // A single command with nothing typed past the slash still lists them.
        let listed = command_suggestion("/reasoning ", "/reasoning ".len(), &no_names())
            .expect("the values are offered");
        assert!(listed.hint.contains("budget:<tokens>"), "{listed:?}");
    }

    #[test]
    fn slash_commands_and_arguments_have_dimmed_suggestions() {
        let commands = command_suggestion("/", 1, &no_names()).expect("command list");
        assert_eq!(commands.completion, "help");
        assert!(commands.hint.contains("/reasoning"));

        let prefix = command_suggestion("/rea", 4, &no_names()).expect("command prefix");
        assert_eq!(prefix.completion, "soning");

        let settings =
            command_suggestion("/reasoning ", "/reasoning ".len(), &no_names()).expect("settings");
        assert_eq!(settings.completion, "auto");
        assert!(settings.hint.contains("high"));
        assert!(settings.hint.contains("budget:<tokens>"));

        let partial =
            command_suggestion("/reasoning h", "/reasoning h".len(), &no_names()).expect("effort");
        assert_eq!(partial.completion, "igh");

        let scope = command_suggestion("/reasoning high ", "/reasoning high ".len(), &no_names())
            .expect("scope");
        assert_eq!(scope.completion, "session");

        let tool_scope =
            command_suggestion("/allow bash ", "/allow bash ".len(), &no_names()).expect("scope");
        assert_eq!(tool_scope.completion, "session");

        assert!(command_suggestion("/steer", "/steer".len(), &no_names()).is_none());
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

    #[test]
    fn the_menu_is_reachable_under_the_name_and_the_alias() {
        // The startup screen is a list, and the command is the second way to
        // reach it. `/back` is the name a person reaches for after wandering
        // into something; both must land on the same thing, or the help would
        // offer one and the parser answer only the other.
        assert!(matches!(
            parse_chat_command("/menu"),
            Some(Ok(ChatCommand::Menu))
        ));
        assert!(matches!(
            parse_chat_command("/back"),
            Some(Ok(ChatCommand::Menu))
        ));
        assert!(offered("/menu"), "the list must offer it");
        assert_eq!(usage_of("/menu"), Some("/menu".to_owned()));
        // It takes no argument, so one is the mistake it is elsewhere: a word
        // after the command is a word the command does not use.
        assert!(matches!(parse_chat_command("/menu now"), Some(Err(_))));
    }

    /// The routing that was missing. `/provider` and `/model` reach the
    /// command through two handlers — one at a turn boundary and one mid-turn —
    /// and the held-provider state was only wired into the second. At a turn
    /// boundary (the idle case, which is nearly always) `/provider exa` kept the
    /// old model, built `exa/river-model`, and sent it: the service refused a
    /// pair no file lists, the switch never took, and `/model` put the old
    /// provider back. Neither half could be moved.
    ///
    /// This drives the real loop — the same `interactive_loop` a person types
    /// into — rather than the pure decision the unit test above already covers,
    /// so it is the wiring, and not the policy, that fails if it regresses.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn a_held_provider_switch_is_taken_by_the_turn_boundary_handler() {
        use std::collections::BTreeMap;

        use norte::harness::{InMemoryBlobStore, InMemoryStore};
        use norte::{Capabilities, Model, ModelId, ModelRef, ProviderId};
        use norte_code_agent::{ModelFactory, ModelSpec, Service, ServiceError};
        use norte_code_protocol::catalog::Catalog;
        use norte_code_protocol::config::{ApiKey, Config};
        use norte_testing::ScriptedModel;

        struct Factory(Arc<dyn Model>);
        impl ModelFactory for Factory {
            fn build(
                &self,
                _spec: &ModelSpec,
                _capabilities: &Capabilities,
            ) -> Result<Arc<dyn Model>, ServiceError> {
                Ok(Arc::clone(&self.0))
            }
        }

        let providers: norte_code_protocol::catalog::Providers = serde_json::from_str(
            r#"{"providers":{
                "first":{"name":"First","environment_keys":["PATH"],
                    "wires":[{"api":"https://example.invalid/v1","schema":"openai"}]},
                "second":{"name":"Second","environment_keys":["PATH"],
                    "wires":[{"api":"https://example.invalid/v1","schema":"openai"}]}}}"#,
        )
        .expect("providers");
        // Only `first` lists `shared`. `second` lists a model of its own, so a
        // switch to it cannot carry the model the session is on.
        let models: norte_code_protocol::catalog::Models = serde_json::from_str(
            r#"{"models":[
                {"id":"shared","provider_id":"first","name":"Shared",
                    "supported_params":["tools"],
                    "modalities":{"input":["text"],"output":["text"]},
                    "limit":{"context":1000,"output":100}},
                {"id":"other","provider_id":"second","name":"Other",
                    "supported_params":["tools"],
                    "modalities":{"input":["text"],"output":["text"]},
                    "limit":{"context":1000,"output":100}}]}"#,
        )
        .expect("models");
        let catalog = Arc::new(Catalog::new(providers, models).expect("catalogue"));
        let entry = catalog.find("first", "shared").expect("model");
        let spec = ModelSpec {
            provider: "first".to_owned(),
            model: "shared".to_owned(),
            endpoint: "https://example.invalid/v1".to_owned(),
            api_key: ApiKey::new("test"),
            schema: norte_code_protocol::catalog::WireSchema::OpenAI,
            auth: None,
            capabilities: norte_code_protocol::catalog::capabilities(entry, WireSchema::OpenAI),
            headers: BTreeMap::new(),
        };
        let reference = ModelRef::new(
            ProviderId::new("first").expect("provider"),
            ModelId::new("shared").expect("model"),
        );
        let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
        let service = Arc::new(
            Service::new(
                Config::default(),
                catalog,
                spec,
                Arc::new(Factory(model)),
                Arc::new(InMemoryStore::new()),
                Arc::new(InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );

        let pinned = Pinned::new();
        let mut renderer = Renderer::new(
            Format::Text,
            Verbosity::Low,
            false,
            Style::plain(),
            Arc::clone(&pinned),
        );

        let events = drain_request(&service, ClientRequestKind::CreateSession { title: None })
            .await
            .expect("create session");
        let session = events
            .iter()
            .find_map(|event| event.session.clone())
            .expect("session key");

        // Drive the loop the way a person does: `/provider`, then `/model`, then
        // `/exit`. There is no terminal in a test, so the lines come from a
        // channel instead of the keyboard — the same path the real handler
        // takes, which is the thing that was wired wrong.
        let (tx, rx) = mpsc::unbounded_channel();
        let mut input = InteractiveInput::from_events(rx);
        for line in ["/provider second", "/model other", "/exit"] {
            tx.send(EditorEvent::Line(line.to_owned(), Vec::new()))
                .expect("line");
        }
        drop(tx);

        interactive_loop(
            &service,
            Some(session.clone()),
            &mut renderer,
            Style::plain(),
            &mut input,
            None,
            None,
        )
        .await
        .expect("loop");

        assert_eq!(renderer.pending_provider, None, "the switch is settled");
        assert_eq!(
            service.session_spec(&session).map(|spec| spec.label()),
            Some("second/other".to_owned()),
            "both halves moved through the turn-boundary handler"
        );
    }

    /// The wiring the completion depends on, driven through the real loop.
    ///
    /// The names travel from the catalogue to the pinned handle, which is the
    /// only channel the keyboard's thread and the frame share: a completion that
    /// was answered from the catalogue but never pushed there would offer
    /// nothing, and that is a bug no pure test of the candidate list can see.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn the_catalogue_reaches_the_pinned_handle_the_frame_completes_from() {
        use std::collections::BTreeMap;

        use norte::harness::{InMemoryBlobStore, InMemoryStore};
        use norte::{Capabilities, Model, ModelId, ModelRef, ProviderId};
        use norte_code_agent::{ModelFactory, ModelSpec, Service, ServiceError};
        use norte_code_protocol::catalog::Catalog;
        use norte_code_protocol::config::{ApiKey, Config};
        use norte_testing::ScriptedModel;

        struct Factory(Arc<dyn Model>);
        impl ModelFactory for Factory {
            fn build(
                &self,
                _spec: &ModelSpec,
                _capabilities: &Capabilities,
            ) -> Result<Arc<dyn Model>, ServiceError> {
                Ok(Arc::clone(&self.0))
            }
        }

        let providers: norte_code_protocol::catalog::Providers = serde_json::from_str(
            r#"{"providers":{
                "exa":{"name":"Exa","environment_keys":["PATH"],
                    "wires":[{"api":"https://example.invalid/v1","schema":"openai"}]},
                "river":{"name":"River","environment_keys":["PATH"],
                    "wires":[{"api":"https://example.invalid/v1","schema":"openai"}]}}}"#,
        )
        .expect("providers");
        let models: norte_code_protocol::catalog::Models = serde_json::from_str(
            r#"{"models":[
                {"id":"deepseek-v4.1-flash-uncensored","provider_id":"exa","name":"U",
                    "supported_params":["tools"],
                    "modalities":{"input":["text"],"output":["text"]},
                    "limit":{"context":1000,"output":100}},
                {"id":"deepseek-ai/DeepSeek-V4.1-Flash","provider_id":"river","name":"F",
                    "supported_params":["tools"],
                    "modalities":{"input":["text"],"output":["text"]},
                    "limit":{"context":1000,"output":100}}]}"#,
        )
        .expect("models");
        let catalog = Arc::new(Catalog::new(providers, models).expect("catalogue"));
        let entry = catalog
            .find("exa", "deepseek-v4.1-flash-uncensored")
            .expect("model");
        let spec = ModelSpec {
            provider: "exa".to_owned(),
            model: "deepseek-v4.1-flash-uncensored".to_owned(),
            endpoint: "https://example.invalid/v1".to_owned(),
            api_key: ApiKey::new("test"),
            schema: norte_code_protocol::catalog::WireSchema::OpenAI,
            auth: None,
            capabilities: norte_code_protocol::catalog::capabilities(entry, WireSchema::OpenAI),
            headers: BTreeMap::new(),
        };
        let reference = ModelRef::new(
            ProviderId::new("exa").expect("provider"),
            ModelId::new("deepseek-v4.1-flash-uncensored").expect("model"),
        );
        let model = Arc::new(ScriptedModel::new(reference).turn_text("ok"));
        let service = Arc::new(
            Service::new(
                Config::default(),
                catalog,
                spec,
                Arc::new(Factory(model)),
                Arc::new(InMemoryStore::new()),
                Arc::new(InMemoryBlobStore::new()),
                "system",
            )
            .expect("service"),
        );

        let pinned = Pinned::new();
        let mut renderer = Renderer::new(
            Format::Text,
            Verbosity::Low,
            false,
            Style::plain(),
            Arc::clone(&pinned),
        );
        let events = drain_request(&service, ClientRequestKind::CreateSession { title: None })
            .await
            .expect("create session");
        let session = events
            .iter()
            .find_map(|event| event.session.clone())
            .expect("session key");

        // A session on `exa`: the models offered are `exa`'s.
        refresh_completions(&service, Some(&session), &mut renderer);
        let names = pinned.completion_names();
        assert_eq!(
            names.providers,
            ["exa", "river"],
            "every provider is offered"
        );
        assert_eq!(
            names.models,
            ["deepseek-v4.1-flash-uncensored"],
            "the session's own provider decides the models: {names:?}"
        );

        // `/provider river` holds the switch, because `river` does not serve the
        // model the session is on, and the completion follows the held provider:
        // the next `/model` line offers `river`'s models, which is the list the
        // notice just told the person to pick from.
        switch_provider(&service, &session, "river", &mut renderer).await;
        assert_eq!(renderer.pending_provider.as_deref(), Some("river"));
        let held = pinned.completion_names();
        assert_eq!(
            held.models,
            ["deepseek-ai/DeepSeek-V4.1-Flash"],
            "the held provider decides the models: {held:?}"
        );
        // And the frame, which is what draws the menu, offers it from the same
        // names: the row reaches the frame and not only the test.
        let suggestion = command_suggestion(
            "/model deep",
            "/model deep".len(),
            &pinned.completion_names(),
        )
        .expect("the model is offered");
        assert_eq!(suggestion.completion, "seek-ai/DeepSeek-V4.1-Flash");
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
