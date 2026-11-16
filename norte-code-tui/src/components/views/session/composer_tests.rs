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
        composer.all_chips()[0].detail,
        format!("{} chars", long.len())
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

    assert_eq!(composer.chip_lines().len(), 2);
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
        "seven"
    );
}

fn line_content(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}
