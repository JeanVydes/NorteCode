//! The prompt is an application-level modal: it shows what the reader types,
//! and it is visible from any tab.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

use super::{App, Overlay};
use crate::components::ComponentId;
use crate::components::prompt::Prompt;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A shell of an app: the prompt and the menu need nothing from the host.
fn app_shell(tab: ComponentId) -> App {
    App::for_test(tab)
}

fn drawn(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..30 {
        for x in 0..100 {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[test]
fn the_rename_prompt_shows_the_typed_title() {
    let mut app = app_shell(ComponentId::Session);
    app.open_prompt(Prompt::Rename);
    for character in "my title".chars() {
        app.prompt_key(key(KeyCode::Char(character))).expect("key");
    }

    let text = drawn(&mut app);

    assert!(text.contains("rename session"), "drawn:\n{text}");
    assert!(text.contains("my title"), "drawn:\n{text}");
}

#[test]
fn the_prompt_is_visible_from_another_tab() {
    let mut app = app_shell(ComponentId::Telemetry);
    app.open_prompt(Prompt::Budget);

    let text = drawn(&mut app);

    assert!(text.contains("session budget"), "drawn:\n{text}");
}

#[test]
fn the_budget_prompt_only_takes_numbers() {
    let mut app = app_shell(ComponentId::Session);
    app.open_prompt(Prompt::Budget);
    for character in "a1.5x".chars() {
        app.prompt_key(key(KeyCode::Char(character))).expect("key");
    }

    let text = drawn(&mut app);

    assert!(text.contains("1.5"), "drawn:\n{text}");
    assert!(!text.contains("a1.5x"));
}

#[test]
fn escape_closes_the_prompt() {
    let mut app = app_shell(ComponentId::Session);
    app.open_prompt(Prompt::Rename);

    app.prompt_key(key(KeyCode::Esc)).expect("key");

    assert!(!app.prompt_open());
    assert_eq!(app.overlay_for_test(), Overlay::Closed);
}
