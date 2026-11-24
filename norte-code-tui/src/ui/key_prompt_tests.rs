//! The masked key prompt collects a secret without persisting it.

use std::path::Path;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::components::input::Input;
use terminalui::{InputEvent, Theme, Tree, node};

use super::key_prompt::{KeyPrompt, KeyPromptAction};
use super::key_status::{cache_key, provider_has_key};

fn key(code: KeyCode, modifiers: KeyModifiers) -> InputEvent {
    InputEvent::from(Event::Key(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    }))
}

fn char_key(character: char) -> InputEvent {
    key(KeyCode::Char(character), KeyModifiers::NONE)
}

fn enter_key() -> InputEvent {
    key(KeyCode::Enter, KeyModifiers::NONE)
}

fn open_prompt() -> Tree {
    let mut tree = Tree::new(node("prompt", KeyPrompt::new()));
    tree.find_mut::<KeyPrompt>("prompt")
        .expect("key prompt")
        .open("anthropic", &["ANTHROPIC_API_KEY".to_owned()]);
    assert!(tree.focus("key:input"));
    tree
}

#[test]
fn cache_keys_separate_providers_and_secret_files() {
    let first = cache_key(Path::new("/tmp/one/secrets.json"), "anthropic");
    let other_provider = cache_key(Path::new("/tmp/one/secrets.json"), "openai");
    let other_file = cache_key(Path::new("/tmp/two/secrets.json"), "anthropic");
    assert_ne!(first, other_provider);
    assert_ne!(first, other_file);
}

#[test]
fn a_provider_without_environment_keys_needs_no_lookup() {
    assert!(provider_has_key("anything", &[]));
}

#[test]
fn submitting_reports_the_key_to_the_host() {
    let mut tree = open_prompt();
    for character in "secret".chars() {
        tree.handle_event(&char_key(character));
    }
    tree.handle_event(&enter_key());

    tree.find_mut::<KeyPrompt>("prompt")
        .expect("key prompt")
        .drain();
    let action = tree
        .find_mut::<KeyPrompt>("prompt")
        .and_then(KeyPrompt::take_action);
    assert_eq!(
        action,
        Some(KeyPromptAction::Submit {
            provider: "anthropic".to_owned(),
            key: "secret".to_owned(),
        })
    );
}

#[test]
fn the_rendered_value_is_masked() {
    let mut tree = open_prompt();
    for character in "secret".chars() {
        tree.handle_event(&char_key(character));
    }
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
    tree.render(buffer.area, &mut buffer, &Theme::default());

    let area = tree
        .find::<Input>("key:input")
        .expect("key input")
        .state
        .area;
    let visible: String = (area.x..area.x + area.width)
        .map(|x| {
            buffer
                .cell((x, area.y + area.height / 2))
                .map_or(" ", ratatui::buffer::Cell::symbol)
        })
        .collect();
    assert!(visible.contains('*'));
    assert!(!visible.contains('s'));
}

#[test]
fn the_rendered_prompt_shows_full_button_labels() {
    let mut tree = open_prompt();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
    tree.render(buffer.area, &mut buffer, &Theme::default());

    let rendered: String = buffer
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();
    assert!(
        rendered.contains("cancel"),
        "the cancel button was clipped: {rendered:?}"
    );
    assert!(
        rendered.contains("save"),
        "the save button was clipped: {rendered:?}"
    );
}

#[test]
fn escape_reports_cancellation() {
    let mut tree = open_prompt();
    tree.handle_event(&key(KeyCode::Esc, KeyModifiers::NONE));
    tree.find_mut::<KeyPrompt>("prompt")
        .expect("key prompt")
        .drain();

    let action = tree
        .find_mut::<KeyPrompt>("prompt")
        .and_then(KeyPrompt::take_action);
    assert_eq!(action, Some(KeyPromptAction::Cancel));
}
