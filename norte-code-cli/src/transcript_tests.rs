use super::*;

/// The text of an entry's rows, which is what most of these are about.
fn texts(entry: &Entry) -> Vec<String> {
    entry.lines.iter().map(Line::text).collect()
}

#[test]
fn a_streamed_answer_is_one_entry_and_not_one_per_token() {
    // The whole reason `append` exists: a token at a time would otherwise
    // be a line at a time, and an answer of five hundred tokens would be
    // five hundred entries to scroll through.
    let mut transcript = Transcript::new();
    for token in ["Hel", "lo", " world"] {
        transcript.append(Kind::Answer, token);
    }
    assert_eq!(transcript.len(), 1, "{transcript:?}");
    assert_eq!(texts(&transcript.entries()[0]), ["Hello world"]);
}

#[test]
fn a_newline_in_a_token_starts_a_line_and_not_an_entry() {
    let mut transcript = Transcript::new();
    transcript.append(Kind::Answer, "one\ntwo");
    transcript.append(Kind::Answer, "\nthree");
    assert_eq!(transcript.len(), 1, "{transcript:?}");
    assert_eq!(texts(&transcript.entries()[0]), ["one", "two", "three"]);
}

#[test]
fn a_different_kind_starts_its_own_entry() {
    // An answer that follows a tool's output is a new thing said, and
    // joining them would make the transcript read as if it were not.
    let mut transcript = Transcript::new();
    transcript.append(Kind::Reasoning, "thinking");
    transcript.append(Kind::Answer, "the answer");
    assert_eq!(transcript.len(), 2, "{transcript:?}");
    assert_eq!(transcript.entries()[0].kind, Kind::Reasoning);
    assert_eq!(transcript.entries()[1].kind, Kind::Answer);
}

#[test]
fn closing_ends_the_entry_so_the_next_token_starts_another() {
    let mut transcript = Transcript::new();
    transcript.append(Kind::Answer, "first");
    transcript.close();
    transcript.append(Kind::Answer, "second");
    assert_eq!(transcript.len(), 2, "{transcript:?}");
}

#[test]
fn a_line_past_the_bound_is_cut_and_says_so_in_the_text() {
    // A line is cut visibly rather than counted: the number would take more
    // room than the line it belongs to.
    let mut entry = Entry::new(Kind::ToolOutput);
    entry.push_line("x".repeat(MAX_LINE_CHARS + 100));
    let line = entry.lines[0].text();
    assert_eq!(line.chars().count(), MAX_LINE_CHARS);
    assert!(line.ends_with('…'), "the cut is visible");
}

#[test]
fn an_entry_past_the_bound_drops_the_oldest_and_counts_them() {
    // A tool that prints a thousand lines is one entry, and the newest are
    // the ones worth keeping: output is read from its end.
    let mut entry = Entry::new(Kind::ToolOutput);
    for index in 0..MAX_ENTRY_LINES + 10 {
        entry.push_line(format!("line {index}"));
    }
    assert_eq!(entry.lines.len(), MAX_ENTRY_LINES);
    assert_eq!(entry.dropped, 10);
    assert_eq!(
        entry.lines[0].text(),
        format!("line {}", 10),
        "the oldest went first"
    );
}

#[test]
fn a_tab_is_measured_as_the_spaces_it_becomes() {
    // A tab's width depends on the column it starts at, and this does not
    // know the column: it is written as spaces so the count is honest.
    let mut entry = Entry::new(Kind::Answer);
    entry.push_line("a\tb");
    assert_eq!(entry.lines[0].text(), "a    b");
}

#[test]
fn a_running_entry_counts_and_a_finished_one_stops() {
    // The header of a tool call shows the call running, so its clock has to
    // be there and has to stop: a count that kept going beside a finished
    // call is a number that lies.
    let mut transcript = Transcript::new();
    let mut entry = Entry::line(Kind::ToolStarted, "→ shell #abc");
    entry.started = Some(std::time::Instant::now());
    transcript.push(entry);
    assert!(
        transcript.running_for().is_some(),
        "a call that is out is counting"
    );
    let measured = transcript.stop_timer();
    assert!(measured.is_some(), "and it reports what it measured");
    assert!(
        transcript.running_for().is_none(),
        "and it stops counting when it ends"
    );
}

#[test]
fn an_entry_that_counts_nothing_says_so() {
    // A message has no duration: a timer on every entry would put a number
    // beside things that never run.
    let mut transcript = Transcript::new();
    transcript.push(Entry::line(Kind::Said, "hello"));
    assert!(transcript.running_for().is_none());
}

#[test]
fn an_empty_entry_is_not_added() {
    // A notice that printed nothing is not a thing that happened, and an
    // entry with no lines would be a blank row with nothing to explain it.
    let mut transcript = Transcript::new();
    transcript.push(Entry::new(Kind::Notice));
    assert!(transcript.is_empty());
}

#[test]
fn several_lines_arrive_as_several_lines() {
    let mut entry = Entry::new(Kind::ToolOutput);
    entry.push_text("one\ntwo\nthree");
    assert_eq!(texts(&entry), ["one", "two", "three"]);
}

#[test]
fn an_entry_carries_the_ink_of_each_of_its_rows() {
    // A diff is the reason this exists: inside one tool result a removal and
    // an addition are two different things, and the entry's kind cannot say
    // which is which. The ink is data, not an escape sequence in the text:
    // a frame draws text, and a code kept in it would be drawn as characters.
    let mut entry = Entry::new(Kind::ToolOutput);
    entry.push_inked(Ink::Red, "- old line");
    entry.push_line("  context");
    entry.push_inked(Ink::Green, "+ new line");
    assert_eq!(entry.lines[0].ink(), Some(Ink::Red));
    assert_eq!(
        entry.lines[1].ink(),
        None,
        "plain rows keep the entry's ink"
    );
    assert_eq!(entry.lines[2].ink(), Some(Ink::Green));
    assert_eq!(entry.lines[2].text(), "+ new line");
}

#[test]
fn no_row_of_the_conversation_carries_an_escape_sequence() {
    // The bug this whole design exists to prevent: the rows were built with
    // the string painters, those wrap text in raw SGR codes, and the frame
    // put the codes on screen as the characters they are.
    let mut entry = Entry::new(Kind::ToolOutput);
    entry.push_inked(Ink::Red, "- old");
    entry.push_inked(Ink::Green, "+ new");
    for line in &entry.lines {
        assert!(
            !line.text().contains('\u{1b}'),
            "a row carries ink, never an escape: {:?}",
            line.text()
        );
    }
}

#[test]
fn an_entry_the_view_frames_is_kept_even_with_no_rows() {
    // A call that produced nothing still happened, and the frame is what
    // says so. `push` used to drop an entry with no lines, which would leave
    // the announcement with nothing under it.
    let mut transcript = Transcript::new();
    transcript.push(Entry::bordered(Kind::ToolOutput, "shell #abc"));
    assert_eq!(transcript.len(), 1);
    assert!(transcript.entries()[0].boxed);
    assert_eq!(transcript.entries()[0].title.as_deref(), Some("shell #abc"));
    // And a plain entry with no rows is still nothing to draw.
    transcript.push(Entry::new(Kind::Notice));
    assert_eq!(transcript.len(), 1);
}

#[test]
fn the_transcript_keeps_the_order_things_happened_in() {
    let mut transcript = Transcript::new();
    transcript.push(Entry::line(Kind::Said, "do the thing"));
    transcript.append(Kind::Answer, "doing it");
    transcript.close();
    transcript.push(Entry::line(Kind::ToolStarted, "read"));
    let kinds: Vec<Kind> = transcript
        .entries()
        .iter()
        .map(|entry| entry.kind)
        .collect();
    assert_eq!(
        kinds,
        [Kind::Said, Kind::Answer, Kind::ToolStarted],
        "the order is the conversation"
    );
}

#[test]
fn replayed_reasoning_keeps_the_full_history_bound() {
    let mut entry = Entry::new(Kind::Reasoning).replayed();
    for line in 0..=MAX_THOUGHT_LINES {
        entry.push_line(format!("thought {line}"));
    }
    assert_eq!(entry.lines.len(), MAX_THOUGHT_LINES + 1);
    assert_eq!(entry.dropped, 0);
}

#[test]
fn plain_text_drops_margin_and_padding() {
    let mut transcript = Transcript::new();
    transcript.push(Entry::line(Kind::Said, "alpha beta"));
    transcript.push(Entry::line(Kind::Answer, "gamma"));
    let text = transcript.plain_text(..);
    assert_eq!(text, "alpha beta\ngamma");
}
