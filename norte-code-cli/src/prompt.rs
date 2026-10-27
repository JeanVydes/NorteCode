//! Questions and numbered human input.

use std::sync::Arc;

use super::{
    ClientRequest, ClientRequestKind, ServerEvent, ServerEventKind, Service, SessionKey, StreamExt,
    mpsc,
};

/// Answers a `QuestionAsked` event, walking the choices with the arrow keys.
///
/// With closed `options` the choices are drawn as a list the cursor walks, and
/// the last row is "something else": choosing it opens the input box, which is
/// why nothing is prompted for until it is chosen. Before that there is no box
/// to type in — a prompt that asks for a line and then asks for a line again is
/// asking one question twice, and the second one has no question in it.
///
/// A multi-select answer takes every marked choice, in the order they were
/// marked, and composes them the way a single answer composes one.
pub(crate) async fn answer_if_asked(
    service: &Arc<Service>,
    pinned: &Arc<crate::input::Pinned>,
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
    let Some(input) = input else {
        eprintln!("? {}", crate::text::terminal(prompt));
        eprintln!("(no interactive client; answering with a note)");
        let answer =
            "(no interactive client is attached; continue with your best judgment)".to_owned();
        return submit_answer(service, session, question, answer).await;
    };
    // No choices: there is nothing to walk, so the box is the whole answer and
    // it opens at once.
    if options.is_empty() {
        eprintln!("? {}", crate::text::terminal(prompt));
        let answer = match prompt_input("answer> ", input, interrupts).await? {
            PromptInput::Line(line) => line,
            PromptInput::Eof => {
                "(no interactive client is attached; continue with your best judgment)".to_owned()
            }
            PromptInput::Cancelled => return Ok(true),
        };
        return submit_answer(service, session, question, answer).await;
    }

    // The question is drawn where the draft is drawn: the editor owns those rows
    // and is the only thing that can read a key while a turn runs.
    pinned.ask(
        crate::text::terminal(prompt).into_owned(),
        options.clone(),
        *multiple,
    );
    let answer = loop {
        let next = tokio::select! {
            biased;
            Some(()) = interrupts.recv() => break None,
            event = input.recv() => event,
        };
        // The editor reports what the person did as a question event: it is the
        // only thing that reads a key, so it is the only thing that knows which
        // one was meant. Nothing here reads the question's state instead — doing
        // that answered with the first choice the moment any event arrived, on a
        // question nobody had answered.
        match next {
            Some(crate::input::EditorEvent::Question(answer)) => break Some(answer),
            // A line is not an answer to a question: the box sends its text as a
            // `Question` like every other way of answering.
            Some(crate::input::EditorEvent::Line(..) | crate::input::EditorEvent::Action(_)) => {}
            Some(crate::input::EditorEvent::End) | None => break None,
        }
    };
    // The block goes whatever happened: an answer was sent, or there is nobody
    // left to answer and a question on screen would be one that cannot be.
    pinned.retire();
    let answer = match answer {
        Some(crate::input::Answer::Taken(taken)) => compose_selection(options, &taken, None),
        Some(crate::input::Answer::Typed(text)) => text,
        Some(crate::input::Answer::Left) | None => {
            eprintln!("(the question was left unanswered)");
            "(the question was left unanswered; continue with your best judgment)".to_owned()
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
/// The actions a keystroke asks for — Escape, Ctrl-C, Ctrl-G — have no
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
