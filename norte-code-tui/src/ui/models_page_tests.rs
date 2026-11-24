//! The models page: removing a provider key only after confirmation.

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::layout::Rect;
use terminalui::{InputEvent, Theme, Tree, node};

use super::{Mode, ModelsAction, ModelsData, ModelsPage};

fn model(provider: &str, id: &str) -> norte_code_wire::ModelInfo {
    norte_code_wire::ModelInfo {
        provider: provider.to_owned(),
        model: id.to_owned(),
        name: id.to_owned(),
        description: None,
        max_context: None,
        max_output: None,
        tools: false,
        input: Vec::new(),
        output: Vec::new(),
        reasoning: norte_code_wire::ReasoningInfo::default(),
        deprecated: false,
    }
}

fn page_with_keys() -> ModelsPage {
    let mut page = ModelsPage::new();
    page.set_data(ModelsData {
        models: vec![model("openrouter", "one"), model("anthropic", "two")],
        key_status: HashMap::from([
            ("openrouter".to_owned(), true),
            ("anthropic".to_owned(), true),
        ]),
        active: None,
    });
    page
}

fn key_event(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    }
}

fn input(code: KeyCode) -> InputEvent {
    InputEvent::from(Event::Key(key_event(code)))
}

#[test]
fn deleting_a_key_requires_confirmation_and_carries_the_provider() {
    let mut tree = Tree::new(node("models", page_with_keys()));
    tree.focus("models");
    tree.handle_event(&input(KeyCode::Delete));

    assert!(matches!(
        tree.find::<ModelsPage>("models").map(|page| page.mode.clone()),
        Some(Mode::ConfirmDelete { provider }) if provider == "openrouter"
    ));
    assert!(
        tree.find_mut::<ModelsPage>("models")
            .expect("page")
            .take_pending()
            .is_empty()
    );

    assert!(tree.focus("delete_key_confirm"));
    tree.handle_event(&input(KeyCode::Enter));
    let actions = tree
        .find_mut::<ModelsPage>("models")
        .expect("page")
        .take_pending();
    assert!(
        matches!(&actions[..], [ModelsAction::DeleteKey { provider }] if provider == "openrouter")
    );
}

#[test]
fn cancelling_the_key_dialog_leaves_the_key_alone() {
    let mut tree = Tree::new(node("models", page_with_keys()));
    tree.focus("models");
    tree.handle_event(&input(KeyCode::Delete));
    assert!(tree.focus("delete_key_cancel"));
    tree.handle_event(&input(KeyCode::Enter));

    let actions = tree
        .find_mut::<ModelsPage>("models")
        .expect("page")
        .take_pending();
    assert!(actions.is_empty());
    assert!(matches!(
        tree.find::<ModelsPage>("models")
            .map(|page| page.mode.clone()),
        Some(Mode::Listing)
    ));
}

#[test]
fn the_confirmation_names_the_provider() {
    let mut tree = Tree::new(node("models", page_with_keys()));
    tree.focus("models");
    tree.handle_event(&input(KeyCode::Delete));

    let area = Rect::new(0, 0, 80, 24);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    tree.render(area, &mut buf, &Theme::default());
    let sheet = &buf;
    let painted: String = (0..area.height)
        .flat_map(|y| (0..area.width).map(move |x| sheet[(x, y)].symbol().to_string()))
        .collect();
    assert!(
        painted.contains("Delete the saved key for openrouter?"),
        "{painted}"
    );
}

#[test]
fn typing_d_still_filters_the_catalogue() {
    let mut page = page_with_keys();
    page.handle_key(&key_event(KeyCode::Char('d')));
    assert_eq!(page.filter, "d");
}
