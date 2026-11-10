use super::*;

/// A buffer holding `text`, with the cursor at its end unless moved.
fn draft(text: &str) -> LineBuffer {
    let mut buffer = LineBuffer::default();
    for character in text.chars() {
        buffer.insert(character);
    }
    buffer
}

#[test]
fn enter_takes_the_completion_the_menu_has_under_its_cursor() {
    // Enter sent the line as it stood, so `/provider ri` went off
    // half-typed and was refused: the key beside Tab is the one a person
    // reaches to accept a choice, and with a menu open there is a choice.
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let mut buffer = draft("/provider ri");
    let mut history = Vec::new();
    let mut position = None;
    let mut draft = String::new();
    let outcome = handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut buffer,
        &mut history,
        &mut position,
        &mut draft,
        &sender,
        Some("ver"),
    );
    assert!(matches!(outcome, KeyOutcome::Continue));
    // The line that left is the completion applied, and the draft is
    // empty again: the menu was accepted and the command sent.
    let sent = receiver.try_recv().expect("the line was sent");
    match sent {
        EditorEvent::Line(line, _) => assert_eq!(line, "/provider river"),
        other => panic!("the line was expected, got {other:?}"),
    }
    assert!(buffer.is_empty(), "the draft was consumed");
}

#[test]
fn enter_with_no_completion_sends_the_line_as_it_is() {
    // Nothing offered is not an error: the key still ends the line, and
    // a message is a message.
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let mut buffer = draft("hello");
    let mut history = Vec::new();
    let mut position = None;
    let mut draft = String::new();
    handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut buffer,
        &mut history,
        &mut position,
        &mut draft,
        &sender,
        None,
    );
    match receiver.try_recv().expect("the line was sent") {
        EditorEvent::Line(line, _) => assert_eq!(line, "hello"),
        other => panic!("the line was expected, got {other:?}"),
    }
}

#[test]
fn a_selection_replaces_what_is_typed_over_it() {
    // The point of a selection in the editor: it is a range the next
    // keystroke replaces, which is what makes typing over a word work.
    let mut buffer = draft("hello world");
    buffer.cursor = 0;
    buffer.start_selection();
    buffer.cursor = 5;
    assert_eq!(buffer.selection(), Some((0, 5)));
    buffer.insert('X');
    assert_eq!(buffer.display(), "X world");
    assert_eq!(buffer.selection(), None, "the selection left with the text");
}

#[test]
fn backspace_and_delete_take_the_selection_whole() {
    // A deletion key over a selection removes the selection and not one
    // character beside it, which is what a person expects from it. The
    // range here is `two` inside `one two three` — its own spaces stay,
    // because they were not selected.
    let mut buffer = draft("one two three");
    buffer.cursor = 4;
    buffer.start_selection();
    buffer.cursor = 7;
    assert!(buffer.backspace());
    assert_eq!(buffer.display(), "one  three");

    let mut buffer = draft("one two three");
    buffer.cursor = 4;
    buffer.start_selection();
    buffer.cursor = 7;
    assert!(buffer.delete());
    assert_eq!(buffer.display(), "one  three");
}

#[test]
fn select_all_takes_the_whole_draft_and_the_cursor_is_one_end() {
    let mut buffer = draft(
        "a
b
c",
    );
    buffer.select_all();
    assert_eq!(buffer.selection(), Some((0, 5)));
    assert_eq!(buffer.cursor, 5);
    buffer.delete_selection();
    assert!(
        buffer.is_empty(),
        "select-all then delete empties the draft"
    );
}

#[test]
fn a_plain_arrow_collapses_a_selection_to_its_edge() {
    // An arrow over a selection moves to the edge rather than by one
    // character, so a selection can be narrowed with the arrows alone.
    let mut buffer = draft("abcdef");
    buffer.cursor = 1;
    buffer.start_selection();
    buffer.cursor = 4;
    assert!(buffer.collapse_selection(true), "right goes to the end");
    assert_eq!(buffer.cursor, 4);
    assert_eq!(buffer.selection(), None);

    buffer.cursor = 1;
    buffer.start_selection();
    buffer.cursor = 4;
    assert!(buffer.collapse_selection(false), "left goes to the start");
    assert_eq!(buffer.cursor, 1);
}

#[test]
fn a_selection_over_a_chip_takes_the_chip_whole() {
    // Half a pasted paragraph is not a smaller paragraph, so a selection
    // that touches a chip removes the chip and not the part it covered.
    let mut buffer = LineBuffer::default();
    buffer.insert_paste(&"x".repeat(CHIP_MIN_CHARS + 8));
    assert_eq!(buffer.chips.len(), 1, "the paste became one chip");
    buffer.select_all();
    buffer.delete_selection();
    assert!(buffer.is_empty(), "the chip went with the selection");
    assert!(buffer.chips.is_empty(), "and its range went with it");
}
