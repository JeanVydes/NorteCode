//! The interactive turn loop: input, queued messages, approval handoff and
//! cancellation.

use super::{
    Arc, Budget, ClientRequest, ClientRequestKind, EventStream, Phase, Reasoning, Renderer,
    ServerEvent, ServerEventKind, Service, SessionKey, StreamExt, Style, VecDeque, mpsc, short,
};
use crate::approval::{ApprovalChoice, ApprovalOutcome, approval_request, ask_approval};
use crate::prompt::answer_if_asked;

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
        let (next, from_pending) = match pending.take() {
            Some(text) => (Some(text), true),
            None => (read_repl_line(input, style).await?, false),
        };
        let Some(text) = next else {
            break;
        };
        if from_pending {
            renderer.clear_queued();
        }
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
            renderer.mark_working();
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
    if outcome.parked || renderer.budget_stopped {
        // These lines are not going to be sent; drop the pinned preview.
        renderer.clear_queued();
        return;
    }
    if outcome.queued.is_empty() {
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
                            style.yellow(format!("warning: could not cancel the turn: {error}"))
                        );
                    }
                    continue;
                }
                Wake::Line(line) => {
                    let line = line.trim();
                    if !line.is_empty() {
                        renderer.push_queued(line.to_owned());
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
                    return Ok(DriveOutcome {
                        session: current,
                        parked: true,
                        queued: renderer.queued(),
                    });
                }
            }
            if answer_if_asked(service, &event, input.as_deref_mut(), &mut interrupt_rx).await? {
                renderer.mark_cancelled();
                eprintln!(
                    "{}",
                    style.yellow("question left pending; the session is saved and can be resumed")
                );
                return Ok(DriveOutcome {
                    session: current,
                    parked: true,
                    queued: renderer.queued(),
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
    Ok(DriveOutcome {
        session: current,
        parked,
        queued: renderer.queued(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::SignalGuard;

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
