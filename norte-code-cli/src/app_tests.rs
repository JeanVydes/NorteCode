use super::*;
use crate::input::Pinned;
use crate::transcript::Entry;

#[test]
fn a_long_line_is_wrapped_to_the_width_it_is_given() {
    // The width belongs to the terminal at the moment of the frame, so the
    // wrap is here and not in the data: a wrap kept in the entry would be a
    // wrap for a width the terminal may no longer have.
    let rows = wrap("one two three four", 9);
    assert_eq!(rows, ["one two", "three", "four"]);
}

#[test]
fn indentation_survives_the_wrap() {
    // A diff's shape is its indent: the line that left and the line that
    // arrived are read by how far in they start. Dropping the leading spaces
    // was silent — the row simply came back flush against the margin — and
    // it made every wrapped diff and every nested line of output lie.
    assert_eq!(wrap("    let x = 1;", 40), ["    let x = 1;"]);
    // A wrapped continuation stays indented under the line it came from, so
    // a wrapped paragraph still reads as one paragraph.
    assert_eq!(
        wrap("    one two three", 10),
        ["    one", "    two", "    three"],
        "the continuation is indented too"
    );
    // And a run of spaces inside the line is not collapsed into one.
    assert_eq!(wrap("a  b", 20), ["a  b"]);
}

#[test]
fn a_word_wider_than_the_row_is_cut_rather_than_moved() {
    // Moving it would leave the row empty and the word still too long, so
    // it fills the row and continues on the next one.
    assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
}

#[test]
fn an_empty_line_is_one_empty_row() {
    // A blank line in the conversation is a line, and dropping it would
    // join two paragraphs that were never joined.
    assert_eq!(wrap("", 10), [""]);
}

#[test]
fn a_cut_entry_says_how_much_it_left_out() {
    use crate::transcript::MAX_ENTRY_LINES;
    let mut transcript = Transcript::new();
    let mut entry = Entry::new(Kind::ToolOutput);
    for index in 0..MAX_ENTRY_LINES + 3 {
        entry.push_line(format!("line {index}"));
    }
    transcript.push(entry);
    let lines = transcript_lines(&transcript, 80);
    let last = &lines[lines.len() - 1].text;
    assert!(last.contains('3'), "the count is on screen: {last}");
    assert!(last.contains("left out"), "{last}");
}

#[test]
fn the_conversation_is_read_newest_last() {
    // The view takes from the end, so the order in the data is the order on
    // screen and the last line is the one being read.
    let mut transcript = Transcript::new();
    transcript.push(Entry::line(Kind::Said, "hello"));
    transcript.push(Entry::line(Kind::Answer, "hi"));
    let lines = transcript_lines(&transcript, 80);
    // The two are a said line and an answer, in that order, with the padding
    // row between them: the order is what this pins, not the row count.
    let said = lines
        .iter()
        .position(|row| row.text == "hello")
        .expect("the said line");
    let answer = lines
        .iter()
        .position(|row| row.text == "hi")
        .expect("the answer");
    assert_eq!(lines[said].kind, Kind::Said);
    assert_eq!(lines[answer].kind, Kind::Answer);
    assert!(said < answer, "the order is the order it happened in");
}

#[test]
fn a_blank_row_separates_two_entries_and_not_a_call() {
    // Two messages are two things and a blank row says so; a tool
    // announcement and its output are one call and stay together.
    let mut transcript = Transcript::new();
    transcript.push(Entry::line(Kind::Said, "hello"));
    transcript.push(Entry::line(Kind::Answer, "hi"));
    let rows = transcript_lines(&transcript, 80);
    let said = rows
        .iter()
        .position(|row| row.text == "hello")
        .expect("said");
    let answer = rows
        .iter()
        .position(|row| row.text == "hi")
        .expect("answer");
    assert_eq!(answer - said, 2, "a blank row between them: {rows:?}");
    assert!(rows[said + 1].text.is_empty(), "{rows:?}");

    // One call: the announcement and the output share a call id.
    let mut transcript = Transcript::new();
    let mut started = Entry::line(Kind::ToolStarted, "→ shell");
    started.call_id = Some("c1".to_owned());
    let mut output = Entry::line(Kind::ToolOutput, "done");
    output.call_id = Some("c1".to_owned());
    transcript.push(started);
    transcript.push(output);
    let rows = transcript_lines(&transcript, 80);
    let first = rows
        .iter()
        .position(|row| row.text.contains("shell"))
        .expect("shell");
    let second = rows
        .iter()
        .position(|row| row.text == "done")
        .expect("done");
    assert_eq!(second - first, 1, "no blank row inside a call: {rows:?}");
}

#[test]
fn a_boxed_entry_uses_a_background_without_ascii_rules() {
    let mut transcript = Transcript::new();
    let mut entry = Entry::bordered(Kind::ToolOutput, "shell #abc");
    entry.push_line("one");
    entry.push_line("two");
    transcript.push(entry);
    let rows = transcript_lines(&transcript, 80);
    assert_eq!(rows[0].frame, Some(Frame::Inside));
    assert_eq!(rows[0].text, "shell #abc");
    assert_eq!(rows[1].frame, Some(Frame::Inside));
    assert_eq!(rows[1].text, "one");
    assert_eq!(rows[2].frame, Some(Frame::Inside));
    assert_eq!(rows[2].text, "two");
    for row in &rows[..] {
        assert!(
            !row.text
                .chars()
                .any(|character| matches!(character, '│' | '─' | '╭' | '╮' | '╰' | '╯')),
            "the content carries no ASCII-style frame: {:?}",
            row.text
        );
    }
}

#[test]
fn no_row_the_view_draws_carries_an_escape_sequence() {
    // The bug this design exists to prevent: the rows were built by the
    // string painters, which wrap text in raw SGR codes, and the frame put
    // the codes on screen as the characters they are. A row is text and an
    // ink, and the ink is what the widget turns into colour.
    let mut transcript = Transcript::new();
    let mut diff = Entry::bordered(Kind::ToolOutput, "edit 1/1 · src/main.rs");
    diff.push_inked(Ink::Red, "- old");
    diff.push_inked(Ink::Green, "+ new");
    transcript.push(diff);
    for row in transcript_lines(&transcript, 80) {
        assert!(
            !row.text.contains('\u{1b}'),
            "a row is text and an ink, never an escape: {:?}",
            row.text
        );
    }
}

#[test]
fn the_input_grows_with_what_is_in_it_and_stops() {
    // A block that reserved its maximum would take the conversation's rows
    // for a draft of one line.
    let mut state = State::new();
    assert_eq!(state.heights(80).input, 1, "an empty draft is one row");
    state.input = "one\ntwo\nthree".to_owned();
    assert_eq!(state.heights(80).input, 3);
    // And it stops: a draft of a thousand lines is not a reason to lose the
    // conversation.
    state.input = "x\n".repeat(1_000);
    assert_eq!(state.heights(80).input, INPUT_ROWS);
}

#[test]
fn the_completion_is_its_own_band_so_a_tall_draft_cannot_hide_it() {
    let mut state = State::new();
    state.input = "x\n".repeat(100);
    state.completion = Some(Completion {
        text: "alpha".to_owned(),
        rows: vec!["alpha".to_owned(), "beta".to_owned()],
        cursor: 0,
    });
    let heights = state.heights(80);
    assert_eq!(heights.input, INPUT_ROWS, "the draft keeps its own rows");
    assert_eq!(heights.completion, 2, "the menu keeps its own rows");
    assert!(
        heights.input + heights.completion
            <= INPUT_ROWS + u16::try_from(COMPLETION_ROWS).unwrap_or(0),
        "the two are measured separately, not out of one budget"
    );
}

#[test]
fn the_input_wraps_at_the_body_width_and_shows_every_row() {
    // The conversation owns the whole width now, so the draft wraps at the
    // terminal's width. The block's height is measured there too: counting
    // it narrower than the text gave it one row fewer than it needed and cut
    // the last row off — the words being typed.
    let mut state = State::new();
    state.input = "x".repeat(70);
    let rows_of_x: Vec<usize> = screen_of(&state, 80, 24)
        .iter()
        .map(|row| row.chars().filter(|c| *c == 'x').count())
        .filter(|count| *count > 0)
        .collect();
    assert_eq!(
        rows_of_x.len(),
        1,
        "the draft fits one row at the full width: {rows_of_x:?}"
    );
    assert_eq!(
        rows_of_x.iter().sum::<usize>(),
        70,
        "and no character was lost: {rows_of_x:?}"
    );
}

/// A completion menu with `rows` rows and its cursor at `cursor`.
fn menu(rows: usize, cursor: usize) -> Completion {
    Completion {
        text: String::new(),
        rows: (0..rows).map(|n| format!("model-{n}")).collect(),
        cursor,
    }
}

#[test]
fn the_completion_menu_follows_its_cursor_instead_of_its_head() {
    // The menu drew the first four rows whatever the cursor did, so a list
    // of ten walked from the fifth row on showed nothing: the arrows moved a
    // choice that was below the window the whole way. The window follows the
    // cursor now, and the row under it is the row Enter would take.
    let mut state = State::new();
    state.completion = Some(menu(10, 7));
    let screen = screen_of(&state, 80, 24).join("\n");
    assert!(
        screen.contains("model-7"),
        "the row under the cursor is on screen: {screen}"
    );
    assert!(
        !screen.contains("model-0"),
        "and the head of the list is not: the window moved: {screen}"
    );

    // It stops at the end rather than scrolling past it: the last row is on
    // screen and the window is full, so the list never shows blank rows.
    state.completion = Some(menu(10, 9));
    let end = screen_of(&state, 80, 24).join("\n");
    assert!(end.contains("model-9"), "the last row is shown: {end}");
    assert!(end.contains("model-6"), "and the window is full: {end}");
    assert!(
        !end.contains("model-5"),
        "starting at the list's end: {end}"
    );
}

#[test]
fn a_menu_shorter_than_the_window_starts_at_its_first_row() {
    // The window is `COMPLETION_ROWS` tall and a list of three fills three
    // rows from the top: there is nowhere to scroll and the arithmetic that
    // moves it must not cut the head off a list that fits.
    let mut state = State::new();
    state.completion = Some(menu(3, 2));
    let screen = screen_of(&state, 80, 24).join("\n");
    for row in ["model-0", "model-1", "model-2"] {
        assert!(screen.contains(row), "`{row}` is shown: {screen}");
    }
}

#[test]
fn a_draft_taller_than_the_block_shows_the_line_the_cursor_is_on() {
    // The block is bounded, and a draft past the bound scrolls inside it. A
    // block pinned to its first row scrolls the cursor off the bottom, and
    // the line being written is the one thing the block exists to show.
    let mut state = State::new();
    state.input = (0..30)
        .map(|n| format!("row {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    state.cursor = state.input.chars().count();
    let screen = screen_of(&state, 80, 24).join("\n");
    assert!(
        screen.contains("row 29"),
        "the last row, where the cursor is, is shown: {screen}"
    );
}

#[test]
fn the_selection_is_drawn_as_an_inverted_run() {
    // A selection in the draft is the same reverse video the mouse selection
    // uses: the frame inverts the range and the cursor is one cell of it.
    let mut state = State::new();
    state.input = "hello world".to_owned();
    state.cursor = 5;
    state.select = Some(0);
    let reversed = reversed_rows(&state, 40, 12);
    let row = reversed
        .iter()
        .position(|count| *count > 0)
        .expect("the draft's row");
    // Five selected characters and the cursor's own cell on the space after
    // `hello`: the run is the selection, and the cursor sits at its end.
    assert!(
        reversed[row] >= 5,
        "the selected run is inverted: {reversed:?}"
    );

    // With no selection only the cursor is inverted, which is one cell.
    let mut plain = State::new();
    plain.input = "hello world".to_owned();
    plain.cursor = 5;
    let reversed = reversed_rows(&plain, 40, 12);
    let row = reversed
        .iter()
        .position(|count| *count > 0)
        .expect("the draft's row");
    assert_eq!(
        reversed[row], 1,
        "with no selection one cell is the cursor: {reversed:?}"
    );
}

#[test]
fn the_cursor_is_a_box_on_one_row_and_not_on_every_line() {
    // Reading the clamp as "the cursor sits at the end of every line" drew a
    // box after each one. There is one cursor, and it sits on one row.
    let mut state = State::new();
    state.input = "alpha\nbeta\ngamma".to_owned();
    state.cursor = state.input.chars().count();
    // The cursor cells are the reversed ones: count them per screen row.
    let reversed = reversed_rows(&state, 40, 12);
    let with_cursor: Vec<usize> = reversed
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(row, _)| row)
        .collect();
    assert_eq!(
        with_cursor.len(),
        1,
        "there is one cursor row, not one per line: {reversed:?}"
    );
    // And the two lines above it carry no cursor at all.
    assert!(
        with_cursor[0] >= 2,
        "the cursor is on the last line, not the first: {with_cursor:?}"
    );
}

#[test]
fn a_line_at_the_end_does_not_move_what_is_being_read() {
    // While an answer streams in, a person reading history stays on the line
    // they were on. The offset counts rows back from the end, so a line
    // arriving at the end used to shift the window up by a row and the text
    // under the eyes walked away; the frame folds that growth into the offset
    // now, which is what holds the view still.
    let mut transcript = Transcript::new();
    for index in 0..40 {
        transcript.push(Entry::line(Kind::Answer, format!("line {index}")));
    }
    let width = 80;
    let pinned = Pinned::new();
    // Three rows back from the tail, which is where a scroll leaves it.
    pinned.scroll_transcript(-3);

    let before = transcript_lines(&transcript, width);
    pinned.transcript_grew(before.len());
    let (from, take) = viewport(before.len(), 20, pinned.transcript_scroll());
    let seen: Vec<&str> = before[from..from + take]
        .iter()
        .map(|row| row.text.as_str())
        .collect();

    // A chunk of the streaming answer lands as two more rows.
    transcript.push(Entry::line(Kind::Answer, "one more".to_owned()));
    let after = transcript_lines(&transcript, width);
    let added = after.len().saturating_sub(before.len());
    assert!(added >= 1, "the new entry added rows: {added}");
    pinned.transcript_grew(after.len());
    let (from, take) = viewport(after.len(), 20, pinned.transcript_scroll());
    let now: Vec<&str> = after[from..from + take]
        .iter()
        .map(|row| row.text.as_str())
        .collect();

    assert_eq!(
        now, seen,
        "the rows under the eye did not move as the answer grew"
    );
}

#[test]
fn the_view_follows_the_tail_while_it_is_at_the_tail() {
    // At the tail the offset is zero and nothing is folded: the newest line
    // is the one to show, so a chunk arriving has to push the window forward
    // with it.
    let mut transcript = Transcript::new();
    for index in 0..40 {
        transcript.push(Entry::line(Kind::Answer, format!("line {index}")));
    }
    let width = 80;
    let pinned = Pinned::new();
    let before = transcript_lines(&transcript, width);
    pinned.transcript_grew(before.len());
    transcript.push(Entry::line(Kind::Answer, "one more".to_owned()));
    let after = transcript_lines(&transcript, width);
    pinned.transcript_grew(after.len());
    assert_eq!(pinned.transcript_scroll(), 0, "the tail stays the tail");
    let (from, take) = viewport(after.len(), 20, pinned.transcript_scroll());
    assert_eq!(from + take, after.len(), "the last row is on screen");
}

/// The frame a state draws, as lines of text.
///
/// It is the whole interface at a size the test chose, which is the one
/// thing the unit tests above cannot see: they check what the rows *are*
/// and not what the screen *shows*. Every layout bug this file has had —
/// the rule overwritten by the status, the panel that took the wrong
/// columns, the conversation painted over its own tail — was invisible to
/// them and obvious here.
fn frame_of(state: &State, width: u16, height: u16) -> Vec<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let state = state.clone();
    terminal
        .draw(|frame| {
            let area = frame.area();
            let body = Rect::new(area.x, area.y, area.width, area.height);
            draw_transcript(frame, body, &state, &Pinned::new(), Style::plain());
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// Draws twice into the same terminal and reports the second frame.
///
/// It is the one thing a single-frame test cannot see: a frame is diffed
/// against the last one, so a row that used to hold more than it does now
/// keeps the tail of the old text unless the frame writes over it.
fn second_frame(first: &State, then: &State, width: u16, height: u16) -> Vec<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    for state in [first, then] {
        let state = state.clone();
        terminal
            .draw(|frame| {
                let area = frame.area();
                draw_transcript(frame, area, &state, &Pinned::new(), Style::plain());
            })
            .expect("draw");
    }
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The whole interface drawn twice, as the screen the second frame shows.
fn whole_frames(build: impl Fn(&mut State), then: impl Fn(&mut State)) -> Vec<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let (w, h) = (100u16, 32u16);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("terminal");
    let pinned = Pinned::new();
    for step in [&build as &dyn Fn(&mut State), &then] {
        let mut state = State::new();
        step(&mut state);
        terminal
            .draw(|frame| state.paint(frame, Style::plain(), &pinned))
            .expect("draw");
    }
    let buffer = terminal.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn a_frame_that_shrinks_leaves_nothing_of_the_one_before_it() {
    // What this is for: on a real terminal a long answer followed by a
    // shorter one left the tail of the first sitting beside the second, and
    // a box rule that moved up left its dashes behind on the old row. A
    // frame is diffed against the last one, so a cell the new frame does
    // not write keeps what the old frame put there.
    let long = |state: &mut State| {
        state.transcript.push(Entry::line(
            Kind::Answer,
            "a very long line that fills the row and then keeps going past its end",
        ));
        for n in 0..12 {
            state.transcript.push(Entry::line(
                Kind::Answer,
                format!("line {n} of a longer answer"),
            ));
        }
    };
    let short = |state: &mut State| {
        state.transcript.push(Entry::line(Kind::Answer, "short"));
    };
    let rows = whole_frames(long, short);
    // Every row that is not the answer is blank: no dash, no word of the
    // answer that was there before.
    // The chrome is written every frame and belongs where it is: the input
    // prompt, and the rule that closes the frame under it.
    let chrome = |row: &str| row.trim_start().starts_with('›') || row.trim_start().starts_with('─');
    for (at, row) in rows.iter().enumerate() {
        let trimmed = row.trim();
        if trimmed.is_empty() || chrome(row) {
            continue;
        }
        assert!(
            trimmed.contains("short"),
            "row {at} holds something the last frame left: {row:?}"
        );
    }
}

#[test]
fn a_shorter_line_does_not_leave_the_tail_of_the_longer_one() {
    // The bug this exists for: a row that held a long line keeps whatever
    // the new, shorter line did not reach. On a real terminal it came out as
    // stray dashes from a box rule and the tail of an older answer sitting
    // beside the new text.
    let mut first = State::new();
    first.transcript.push(Entry::line(
        Kind::Answer,
        "a line long enough to reach the right edge of the view",
    ));
    let mut then = State::new();
    then.transcript.push(Entry::line(Kind::Answer, "short"));
    let rows = second_frame(&first, &then, 40, 4);
    let painted = rows
        .iter()
        .find(|row| !row.trim().is_empty())
        .expect("a row");
    assert!(
        painted.trim_end().ends_with("short"),
        "the tail of the long line is still there: {rows:?}"
    );
}

#[test]
fn a_scrolled_view_shows_a_way_back_and_the_tail_does_not() {
    // Reading history with no sign that it is history is how a person loses
    // the newest line: the bar is the sign and the button, and it is gone at
    // the tail, where there is nothing to go back to.
    let mut state = State::new();
    for index in 0..40 {
        state
            .transcript
            .push(Entry::line(Kind::Answer, format!("line {index}")));
    }
    // At the tail there is no bar.
    let at_tail = painted_state(&state, 60, 8);
    assert!(at_tail.tail_bar().is_none(), "no bar is drawn at the tail");
    // Scrolled back, the bar is drawn on the bottom row and is clickable.
    let scrolled = Pinned::new();
    scrolled.scroll_transcript(-5);
    paint_into(&scrolled, &state, 60, 8);
    let (left, row, width) = scrolled.tail_bar().expect("a bar when scrolled");
    assert!(width > 0 && left < 60, "the bar spans into the frame");
    assert!(row < 8, "the bar is inside an 8-row frame: {row}");
    assert!(
        scrolled.click_tail_bar(left, row),
        "a click on the bar is caught"
    );
    assert_eq!(scrolled.transcript_scroll(), 0, "and goes to the tail");
}

#[test]
fn a_click_away_from_the_bar_does_not_jump() {
    // The bar is a button, not a hot zone over the conversation: a click on
    // the text is a selection, which is what reading and copying need.
    let pinned = Pinned::new();
    pinned.scroll_transcript(-5);
    assert!(
        !pinned.click_tail_bar(0, 0),
        "nothing is drawn there, so nothing was caught"
    );
    assert_eq!(pinned.transcript_scroll(), 5, "the view did not move");
}

/// Paints a frame and hands back the pinned handle it wrote to.
///
/// The selection is read from what the frame painted, so the only way to
/// test it is to paint one: a test that built the rows by hand would be
/// testing `text_in` against rows nothing ever drew.
fn paint_into(pinned: &Pinned, state: &State, width: u16, height: u16) {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let state = state.clone();
    terminal
        .draw(|frame| state.paint(frame, Style::plain(), pinned))
        .expect("draw");
}

/// A frame painted into a fresh handle, for a test that wants both.
fn painted_state(state: &State, width: u16, height: u16) -> std::sync::Arc<Pinned> {
    let pinned = Pinned::new();
    paint_into(&pinned, state, width, height);
    pinned
}

/// The whole interface painted once, as the rows the screen shows.
fn screen_of(state: &State, width: u16, height: u16) -> Vec<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let pinned = Pinned::new();
    let state = state.clone();
    terminal
        .draw(|frame| state.paint(frame, Style::plain(), &pinned))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The whole interface painted once, as the reversed (cursor) cells per row.
///
/// It is what tells a real cursor from a stray one: the text alone cannot
/// say which cells were inverted, and the cursor is the inversion.
fn reversed_rows(state: &State, width: u16, height: u16) -> Vec<usize> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let pinned = Pinned::new();
    let state = state.clone();
    terminal
        .draw(|frame| state.paint(frame, Style::plain(), &pinned))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .filter(|x| buffer[(*x, y)].modifier.contains(Modifier::REVERSED))
                .count()
        })
        .collect()
}

#[test]
fn a_question_shows_its_prompt_and_every_choice() {
    // The bug this pins: the block counted a question as one row, so the
    // frame cut the list to the prompt and the choices never reached the
    // screen. Every choice has to be there, with the row that opens the box
    // under them.
    let mut state = State::new();
    state.question = Some(Question {
        prompt: "which one?".to_owned(),
        choices: vec!["alpha".to_owned(), "beta".to_owned()],
        cursor: 0,
        multiple: false,
        taken: Vec::new(),
        typing: false,
        masked: false,
    });
    let screen = screen_of(&state, 60, 20).join("\n");
    assert!(screen.contains("which one?"), "{screen}");
    assert!(screen.contains("alpha"), "the first choice: {screen}");
    assert!(screen.contains("beta"), "the second choice: {screen}");
    assert!(screen.contains("something else"), "the box row: {screen}");
}

#[test]
fn a_question_with_no_choices_still_shows_the_box_row() {
    // A model may ask without offering anything: the list is then the one row
    // that opens the box, and it has to be drawn or there is nothing to
    // choose at all.
    let mut state = State::new();
    state.question = Some(Question {
        prompt: "what now?".to_owned(),
        choices: Vec::new(),
        cursor: 0,
        multiple: false,
        taken: Vec::new(),
        typing: false,
        masked: false,
    });
    let screen = screen_of(&state, 60, 20).join("\n");
    assert!(screen.contains("what now?"), "{screen}");
    assert!(screen.contains("something else"), "{screen}");
}

#[test]
fn a_masked_question_is_a_prompt_and_a_box_without_a_list() {
    let mut state = State::new();
    state.question = Some(Question {
        prompt: "API key for ollamacloud".to_owned(),
        choices: Vec::new(),
        cursor: 0,
        multiple: false,
        taken: Vec::new(),
        typing: true,
        masked: true,
    });
    state.input = "••••••".to_owned();
    let screen = screen_of(&state, 60, 20).join("\n");
    assert!(screen.contains("API key for ollamacloud"), "{screen}");
    assert!(screen.contains("••••••"), "the masked box: {screen}");
    assert!(!screen.contains("something else"), "no list: {screen}");
}

#[test]
fn a_free_text_question_opens_its_box_without_the_other_row() {
    let mut state = State::new();
    state.question = Some(Question {
        prompt: "what now?".to_owned(),
        choices: Vec::new(),
        cursor: 0,
        multiple: false,
        taken: Vec::new(),
        typing: true,
        masked: false,
    });
    state.input = "the answer".to_owned();
    let screen = screen_of(&state, 60, 20).join("\n");
    assert!(screen.contains("what now?"), "{screen}");
    assert!(screen.contains("the answer"), "the box: {screen}");
    assert!(
        !screen.contains("something else"),
        "nothing to choose: {screen}"
    );
}

#[test]
fn a_drag_copies_what_the_frame_painted_and_nothing_else() {
    // What the copy reads is the frame's own rows: the selection is in
    // cells of the screen, and only the frame knows where a line landed.
    // A blank row above the text is not part of a drag that starts on it.
    let mut state = State::new();
    state
        .transcript
        .push(Entry::line(Kind::Answer, "alpha beta gamma"));
    let pinned = painted_state(&state, 40, 6);

    // The answer is the last row, because the conversation sits against the
    // input. A drag across `beta` on it: columns 6..=9.
    //
    // Every drag repaints first: a copy takes the rows the frame recorded,
    // because what is on screen is what is being copied, and a second drag
    // without a frame would read the screen from before it.
    paint_into(&pinned, &state, 40, 6);
    let painted = pinned.debug_painted();
    // The row the answer landed on, found from the frame and not assumed:
    // the conversation sits against the input, so where it starts depends on
    // how much is in it and on the chrome below.
    let on_screen = painted
        .iter()
        .position(|line| line.contains("alpha beta gamma"))
        .expect("the answer on screen");
    let row = u16::try_from(on_screen).expect("a row of the screen");
    // The margin leads — a space and the bar — so the answer starts at the
    // column the frame reports and not at zero: `alpha` at 3, `beta` at 9.
    let line = &painted[on_screen];
    let start = u16::try_from(line.find("alpha").expect("the answer")).expect("a column");
    let word = u16::try_from(line.find("beta").expect("the word")).expect("a column");
    pinned.begin_selection(word, row);
    pinned.extend_selection(word + 3, row);
    assert_eq!(start, 3, "the margin is what the row leads with");
    let copied = pinned.finish_selection().expect("the selection");
    assert_eq!(copied, "beta", "the word that was under the drag");

    // A drag that starts on the blank rows above and ends on the text takes
    // the text and not the blanks it passed over.
    paint_into(&pinned, &state, 40, 6);
    let painted = pinned.debug_painted();
    let row = u16::try_from(
        painted
            .iter()
            .position(|line| line.contains("alpha beta gamma"))
            .expect("the answer on screen"),
    )
    .expect("a row of the screen");
    pinned.begin_selection(0, 0);
    pinned.extend_selection(4, row);
    let copied = pinned.finish_selection().expect("the selection");
    assert_eq!(copied, "al", "the drag's own text, without the margin");
    assert_eq!(
        copied.lines().count(),
        1,
        "the blank row above is not a line of the copy: {copied:?}"
    );

    // A drag over nothing is not a copy at all: an empty selection would
    // put a newline on the clipboard and lose whatever was there.
    paint_into(&pinned, &state, 40, 6);
    pinned.begin_selection(0, 0);
    pinned.extend_selection(3, 0);
    assert_eq!(
        pinned.finish_selection(),
        None,
        "a drag over nothing is not a copy"
    );
}

#[test]
fn a_running_call_draws_one_row_that_turns_and_a_finished_one_keeps_its_place() {
    // One call is one row, and the frame is what shows it: while the call
    // runs the row leads with a turning glyph and the time it has taken,
    // and when it ends the same row — the same screen row — carries the
    // outcome. A test of the data alone cannot see either, because the glyph
    // and the clock are drawn and never stored.
    let mut state = State::new();
    let mut running = Entry::line(Kind::ToolStarted, "read README.md");
    running.started = Some(std::time::Instant::now());
    running.call_id = Some("call-1".to_owned());
    state.transcript.push(running);

    let rows = frame_of(&state, 60, 6);
    let live = rows
        .iter()
        .find(|row| row.contains("read README.md"))
        .expect("the live row");
    assert!(
        live.chars()
            .any(|glyph| SPINNER.contains(&glyph.to_string().as_str())),
        "a glyph of the spinner leads it: {live:?}"
    );
    assert!(live.contains("0s"), "and the clock runs: {live:?}");

    // The ending replaces the row's text, on the row it already had: the
    // glyph and the clock are the frame's, so a finished call has neither.
    let mut ended = Entry::line(Kind::ToolStarted, "read README.md");
    ended.kind = Kind::ToolOutput;
    ended.lines = vec![crate::transcript::Line::inked(
        crate::transcript::Ink::Green,
        "\u{2713} read in 3 ms",
    )];
    let mut done = State::new();
    done.transcript.push(ended);
    let rows = frame_of(&done, 60, 6);
    let finished = rows
        .iter()
        .find(|row| row.contains("read in 3 ms"))
        .expect("the finished row");
    assert!(
        !finished
            .chars()
            .any(|glyph| SPINNER.contains(&glyph.to_string().as_str())),
        "no glyph on a call that ended: {finished:?}"
    );
    assert!(!finished.contains("0s"), "and no clock: {finished:?}");
}

#[test]
fn an_answer_is_drawn_as_markdown_and_a_notice_is_not() {
    // The data holds what arrived; the view is what knows it is markdown.
    // A tool's output is a command's bytes and a notice is a sentence, and
    // a `**` in either of those is two asterisks.
    let mut state = State::new();
    state.transcript.push(Entry::line(
        Kind::Answer,
        "## Title\n\n- one\n- two\n\nA **bold** word.",
    ));
    let rows = transcript_lines(&state.transcript, 60);
    let texts: Vec<&str> = rows.iter().map(|row| row.text.as_str()).collect();
    assert!(
        texts.iter().any(|line| line.contains("Title")),
        "the heading survives: {texts:?}"
    );
    assert!(
        !texts.iter().any(|line| line.contains('#')),
        "and its hashes do not: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|line| line.trim_start().starts_with("• one")),
        "a list is drawn with bullets: {texts:?}"
    );
    assert!(
        !texts.iter().any(|line| line.contains("**")),
        "and emphasis markers are gone: {texts:?}"
    );
    let bold = rows
        .iter()
        .flat_map(|row| row.runs.iter())
        .find(|run| run.text == "bold")
        .expect("the bold word is a run of its own");
    assert_eq!(bold.emphasis, crate::markdown::Emphasis::Bold);

    // The same text as a notice is left alone.
    let mut state = State::new();
    state
        .transcript
        .push(Entry::line(Kind::Notice, "## not a title and **not bold**"));
    let rows = transcript_lines(&state.transcript, 60);
    assert!(
        rows[0].text.contains("## not a title and **not bold**"),
        "a notice is text: {:?}",
        rows[0].text
    );
    assert!(rows[0].runs.is_empty(), "and it is one span");
}

#[test]
fn a_fenced_block_is_drawn_as_code_on_every_row_it_spans() {
    // A fence is state that spans lines, which is why the answer is
    // rendered whole and not a line at a time.
    let mut state = State::new();
    state
        .transcript
        .push(Entry::line(Kind::Answer, "```rust\nlet x = 1;\n```"));
    let rows = transcript_lines(&state.transcript, 60);
    let code = rows
        .iter()
        .find(|row| row.text.contains("let x = 1;"))
        .expect("the code row");
    assert!(
        code.runs
            .iter()
            .any(|run| run.emphasis == crate::markdown::Emphasis::Fence),
        "drawn as code and not as prose: {:?}",
        code.runs
    );
    assert!(
        rows.iter().any(|row| row.text.contains("rust")),
        "the language leads the block: {:?}",
        rows.iter().map(|row| row.text.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn the_conversation_paints_every_row_it_was_given() {
    // The area is fixed and the content is not. A short conversation left
    // the rows above it untouched, so whatever the previous frame had put
    // there stayed on screen: on a real terminal that was the tail of an
    // older answer interleaved with the new one.
    let mut state = State::new();
    state.transcript.push(Entry::line(Kind::Said, "hi"));
    let rows = frame_of(&state, 40, 6);
    assert_eq!(rows.len(), 6);
    // The line is the last row: the conversation sits against the input
    // and not against the top of an empty screen.
    assert!(
        rows[5].contains("hi"),
        "the newest line is at the bottom: {rows:?}"
    );
    // And every row above it was written as blank, not left as it was: an
    // untouched row keeps whatever the last frame put there.
    for row in &rows[..5] {
        assert!(
            row.trim().is_empty(),
            "an untouched row keeps old text: {row:?}"
        );
    }
}

#[test]
fn the_view_shows_the_tail_until_someone_scrolls_back() {
    // The offset counts from the end, and the frame clamps it: the reader
    // moves a number of lines without knowing the height, so a scroll past
    // the top has to land on the first row and not on a negative one.
    assert_eq!(viewport(100, 20, 0), (80, 20), "the tail by default");
    assert_eq!(viewport(100, 20, 5), (75, 20), "five rows back");
    assert_eq!(
        viewport(100, 20, 95),
        (0, 20),
        "the top: a full screen, and the first row is the first row"
    );
    assert_eq!(
        viewport(100, 20, 10_000),
        (0, 20),
        "a wild scroll lands on the first row and not past it"
    );
    // A conversation shorter than the screen shows what there is, and the
    // rows it does not have are not blank ones.
    assert_eq!(viewport(3, 20, 0), (0, 3));
    assert_eq!(viewport(0, 20, 0), (0, 0), "nothing to draw");
}

#[test]
fn the_margin_is_what_says_who_is_talking() {
    // Colour alone cannot carry it: on a terminal with no colour every ink
    // is the same ink, and the two sides of a conversation would read as
    // one voice. The bar is what survives that, which is why it is drawn
    // from the kind and not from the ink.
    let plain = Style::plain();
    assert_eq!(
        ink(Kind::Said, plain),
        ink(Kind::Answer, plain),
        "with no colour there is nothing to tell them apart"
    );
    assert_eq!(margin(Kind::Said).trim(), "▌");
    assert_eq!(margin(Kind::Reasoning).trim(), "│");
    assert_eq!(margin(Kind::Answer).trim(), "", "the model has no bar");
}

#[test]
fn every_kind_of_entry_has_its_own_ink() {
    // Colour is what tells a tool's output from the answer at a glance, and
    // two kinds sharing one would be two things made to look alike.
    let style = Style::for_tests(crate::style::Colors::True);
    // `Notice` and `ToolOutput` are both quiet on purpose: they are the
    // same kind of thing to a reader, which is not the case for the rest.
    let distinct: Vec<ratatui::style::Style> = vec![
        ink(Kind::Answer, style),
        ink(Kind::Reasoning, style),
        ink(Kind::ToolStarted, style),
        ink(Kind::Failure, style),
    ];
    for (index, one) in distinct.iter().enumerate() {
        for other in &distinct[index + 1..] {
            assert_ne!(one, other, "two kinds share an ink");
        }
    }
}
