//! Questions and numbered human input.

use std::sync::Arc;

use super::{
    ClientRequest, ClientRequestKind, ServerEvent, ServerEventKind, Service, SessionKey, StreamExt,
    mpsc,
};

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
    input: Option<&mut crate::input::InteractiveInput>,
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
    eprintln!("? {}", crate::text::terminal(prompt));
    let Some(input) = input else {
        eprintln!("(no interactive client; answering with a note)");
        let answer =
            "(no interactive client is attached; continue with your best judgment)".to_owned();
        return submit_answer(service, session, question, answer).await;
    };
    let answer = if options.is_empty() {
        match prompt_input("answer> ", input, interrupts).await? {
            PromptInput::Line(line) => line,
            PromptInput::Eof => {
                "(no interactive client is attached; continue with your best judgment)".to_owned()
            }
            PromptInput::Cancelled => return Ok(true),
        }
    } else {
        for (index, option) in options.iter().enumerate() {
            eprintln!("  {}. {}", index + 1, crate::text::terminal(option));
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
            let line = match prompt_input("choice> ", input, interrupts).await? {
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
                match prompt_input("something else> ", input, interrupts).await? {
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
///
/// The actions a keystroke asks for — Escape, Ctrl-C, Ctrl-Shift-B — have no
/// meaning while a prompt owns the line, so they are skipped rather than
/// mistaken for input: a prompt wants a line, and taking one is the answer.
pub(crate) async fn prompt_input(
    prompt: &str,
    input: &mut crate::input::InteractiveInput,
    interrupts: &mut mpsc::Receiver<()>,
) -> Result<PromptInput, String> {
    if !prompt.is_empty() {
        eprintln!("{}", crate::text::terminal(prompt));
    }
    let line = tokio::select! {
        biased;
        Some(()) = interrupts.recv() => return Ok(PromptInput::Cancelled),
        line = input.recv_line() => line,
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
