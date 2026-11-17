use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Composer, INLINE_LIMIT};
use crate::chips::Chip;
use crate::components::ComponentAction;
use crate::ui_model::{SessionAttachmentState, UiModel};

fn model() -> UiModel {
    UiModel {
        active_session: Some(norte_code_wire::SessionKey::new("session-1")),
        attachment_state: SessionAttachmentState::Attached,
        ..UiModel::default()
    }
}

fn press(composer: &mut Composer, code: KeyCode) -> Option<ComponentAction> {
    composer.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &model())
}

#[test]
fn a_long_text_becomes_a_chip_instead_of_filling_the_composer() {
    let mut composer = Composer::default();
    let long = "x".repeat(INLINE_LIMIT + 10);

    composer.paste(&long);

    assert_eq!(composer.text, "");
    assert_eq!(composer.all_chips().len(), 1);
    assert_eq!(
        composer.all_chips()[0].to_string(),
        format!("[Copied: {}...]", "x".repeat(48))
    );
}

#[test]
fn a_short_text_stays_in_the_composer() {
    let mut composer = Composer::default();

    composer.paste("hola");

    assert_eq!(composer.text, "hola");
    assert_eq!(composer.all_chips().len(), 0);
}

#[test]
fn a_chip_is_deleted_as_a_whole() {
    let mut composer = Composer::default();
    composer.paste(&"x".repeat(INLINE_LIMIT + 1));

    press(&mut composer, KeyCode::Backspace);

    assert_eq!(composer.all_chips().len(), 0);
    assert_eq!(composer.text, "");
}

#[test]
fn the_text_is_deleted_character_by_character() {
    let mut composer = Composer::default();
    composer.paste("hola");

    press(&mut composer, KeyCode::Backspace);

    assert_eq!(composer.text, "hol");
}

#[test]
fn the_chips_that_travel_together_are_deleted_together() {
    let mut composer = Composer::default();
    composer.add_chip(Chip::text("one".to_owned()));
    composer.add_chip(Chip::text("two".to_owned()));

    press(&mut composer, KeyCode::Backspace);

    assert_eq!(composer.all_chips().len(), 0);
}

#[test]
fn sending_carries_the_text_and_the_chips() {
    let mut composer = Composer::default();
    composer.paste("hola");
    composer.add_chip(Chip::text("un texto largo".to_owned()));

    let action = press(&mut composer, KeyCode::Enter);

    let Some(ComponentAction::SendMessage { text, chips }) = action else {
        panic!("the message was not sent");
    };
    assert_eq!(text, "hola");
    assert_eq!(chips.len(), 1);
}

#[test]
fn the_composer_asks_for_a_file_when_the_reader_asks_to_attach_one() {
    let mut composer = Composer::default();

    composer.handle_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        &model(),
    );

    assert_eq!(composer.attach.as_deref(), Some(""));
}

#[test]
fn the_path_of_the_file_is_typed_and_sent() {
    let mut composer = Composer::default();
    composer.handle_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        &model(),
    );
    for character in "photo.png".chars() {
        press(&mut composer, KeyCode::Char(character));
    }

    let action = press(&mut composer, KeyCode::Enter);

    assert!(matches!(action, Some(ComponentAction::AttachFile(path)) if path == "photo.png"));
    assert!(composer.attach.is_none());
}

#[test]
fn the_attach_prompt_can_be_left_without_attaching_anything() {
    let mut composer = Composer::default();
    composer.handle_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        &model(),
    );

    let action = press(&mut composer, KeyCode::Esc);

    assert!(action.is_none());
    assert!(composer.attach.is_none());
}

#[test]
fn shift_enter_inserts_a_newline_instead_of_sending() {
    let mut composer = Composer::default();
    composer.paste("first line");

    let action = composer.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), &model());

    assert!(action.is_none());
    assert_eq!(composer.text, "first line\n");
}

#[test]
fn the_composer_reserves_a_row_for_every_chip_it_draws() {
    let mut composer = Composer::default();
    composer.paste("hola");
    composer.add_chip(Chip::text("a".to_owned()));
    composer.add_chip(Chip::text("b".to_owned()));

    assert_eq!(
        composer.chip_lines(crate::ui_model::ThemeMode::Dark).len(),
        2
    );
    assert_eq!(composer.height(false), 6);
}

#[test]
fn the_composer_keeps_only_five_visible_lines() {
    let mut composer = Composer::default();
    composer.paste("one\ntwo\nthree\nfour\nfive\nsix\nseven");

    assert_eq!(composer.height(false), 8);
    assert_eq!(composer.height(true), 9);
    assert_eq!(composer.visible_lines(&model()).len(), 5);
    assert_eq!(line_content(&composer.visible_lines(&model())[0]), "three");
    assert_eq!(
        line_content(composer.visible_lines(&model()).last().expect("a line")),
        "seven ",
        "the caret sits at the end of the line as one cell"
    );
}

fn line_content(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn the_caret_moves_left_and_right() {
    let mut composer = Composer::default();
    composer.paste("abc");

    composer.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &model());
    composer.handle_key(
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        &model(),
    );

    assert_eq!(composer.text, "abxc");
}

#[test]
fn the_caret_moves_between_the_lines() {
    let mut composer = Composer::default();
    composer.paste("one\nthree");

    composer.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &model());
    composer.handle_key(
        KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
        &model(),
    );

    assert_eq!(composer.text, "oneX\nthree");
}

#[test]
fn the_caret_clamps_to_the_end_of_a_shorter_line() {
    let mut composer = Composer::default();
    composer.paste("abc\nlonger line");

    composer.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &model());
    composer.handle_key(
        KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE),
        &model(),
    );

    assert_eq!(composer.text, "abcZ\nlonger line");
}

#[test]
fn the_caret_returns_to_its_column_after_a_short_line() {
    let mut composer = Composer::default();
    composer.paste("abc\nlonger line");

    composer.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &model());
    composer.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &model());
    composer.handle_key(
        KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE),
        &model(),
    );

    assert_eq!(composer.text, "abc\nlonger lineZ");
}

#[test]
fn erase_removes_the_character_before_the_caret() {
    let mut composer = Composer::default();
    composer.paste("abc");

    composer.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &model());
    composer.handle_key(
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        &model(),
    );

    assert_eq!(composer.text, "ac");
}

#[test]
fn erase_does_nothing_at_the_start_of_the_text() {
    let mut composer = Composer::default();
    composer.paste("abc");

    composer.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &model());
    composer.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &model());
    composer.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &model());
    composer.handle_key(
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        &model(),
    );

    assert_eq!(composer.text, "abc");
}

#[test]
fn home_and_end_move_within_the_line() {
    let mut composer = Composer::default();
    composer.paste("abc\nxyz");

    press(&mut composer, KeyCode::Home);
    press(&mut composer, KeyCode::Char('Q'));
    assert_eq!(composer.text, "abc\nQxyz");

    press(&mut composer, KeyCode::End);
    press(&mut composer, KeyCode::Char('!'));
    assert_eq!(composer.text, "abc\nQxyz!");
}

#[test]
fn home_from_the_end_of_the_first_line_reaches_its_start() {
    let mut composer = Composer::default();
    composer.paste("abc\nxyz");

    press(&mut composer, KeyCode::Up);
    press(&mut composer, KeyCode::Home);
    press(&mut composer, KeyCode::Char('Q'));

    assert_eq!(composer.text, "Qabc\nxyz");
}

#[test]
fn delete_erases_the_character_at_the_caret() {
    let mut composer = Composer::default();
    composer.paste("abc");

    press(&mut composer, KeyCode::Home);
    press(&mut composer, KeyCode::Delete);
    assert_eq!(composer.text, "bc");
    press(&mut composer, KeyCode::Delete);
    assert_eq!(composer.text, "c");
}

#[test]
fn delete_at_the_end_of_the_text_does_nothing() {
    let mut composer = Composer::default();
    composer.paste("abc");

    press(&mut composer, KeyCode::Delete);

    assert_eq!(composer.text, "abc");
}

#[test]
fn delete_at_the_end_of_a_line_joins_it_with_the_next() {
    let mut composer = Composer::default();
    composer.paste("ab\ncd");

    press(&mut composer, KeyCode::Up);
    press(&mut composer, KeyCode::Delete);
    assert_eq!(composer.text, "abcd");
}

#[test]
fn a_newline_at_the_end_opens_a_line_for_the_caret() {
    let mut composer = Composer::default();
    composer.paste("first line");

    composer.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), &model());

    let lines = composer.visible_lines(&model());
    assert_eq!(lines.len(), 2);
    assert_eq!(line_content(&lines[1]), " ");
    assert_eq!(composer.height(false), 5);
}

#[test]
fn the_visible_lines_follow_the_caret() {
    let mut composer = Composer::default();
    composer.paste("one\ntwo\nthree\nfour\nfive\nsix\nseven");

    for _ in 0..6 {
        composer.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &model());
    }

    let lines = composer.visible_lines(&model());
    assert_eq!(lines.len(), 5);
    assert_eq!(line_content(&lines[0]), "one ");
}

#[test]
fn sending_puts_the_caret_back_at_the_start() {
    let mut composer = Composer::default();
    composer.paste("hola");
    press(&mut composer, KeyCode::Enter);

    composer.paste("ab");
    composer.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &model());
    composer.handle_key(
        KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
        &model(),
    );

    assert_eq!(composer.text, "aXb");
}
