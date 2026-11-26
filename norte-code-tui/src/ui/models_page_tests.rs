//! The models page: removing a provider key only after confirmation.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};
use terminalui::{InputEvent, Outcome, Tree, node};

use super::{Mode, ModelsAction, ModelsData, ModelsPage, ProviderKeyStatus};

fn file_key() -> ProviderKeyStatus {
    crate::test_support::provider_key_status(norte_code_wire::ProviderKeySource::File, &[])
}

fn page_with_keys() -> ModelsPage {
    let mut page = ModelsPage::new();
    page.set_data(ModelsData {
        models: vec![
            crate::test_support::model_info("openrouter", "one"),
            crate::test_support::model_info("anthropic", "two"),
        ],
        key_status: HashMap::from([
            ("openrouter".to_owned(), file_key()),
            ("anthropic".to_owned(), file_key()),
        ]),
        active: None,
    });
    page
}

fn click(column: u16, row: u16) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

#[test]
fn clicks_activate_the_rendered_row_and_ignore_headers_and_details() {
    let area = Rect::new(4, 3, 80, 12);
    let mut tree = Tree::new(node("models", page_with_keys()));
    tree.focus("models");
    tree.render(
        area,
        &mut Buffer::empty(area),
        &terminalui::Theme::default(),
    );

    for (column, row) in [(4, 3), (4, 4), (4, 5), (70, 6)] {
        tree.handle_event(&click(column, row));
        assert!(
            tree.find_mut::<ModelsPage>("models")
                .unwrap()
                .take_pending()
                .is_empty()
        );
    }
    tree.handle_event(&click(5, 6));
    let actions = tree
        .find_mut::<ModelsPage>("models")
        .unwrap()
        .take_pending();
    assert!(
        matches!(&actions[..], [ModelsAction::ToggleExpand { provider }] if provider == "openrouter")
    );

    tree.handle_event(&click(5, 7));
    let actions = tree
        .find_mut::<ModelsPage>("models")
        .unwrap()
        .take_pending();
    assert!(
        matches!(&actions[..], [ModelsAction::ToggleExpand { provider }] if provider == "anthropic")
    );
}

#[test]
fn clicks_follow_the_table_scroll_offset() {
    let mut page = ModelsPage::new();
    page.set_data(ModelsData {
        models: (0..12)
            .map(|index| crate::test_support::model_info(&format!("provider{index:02}"), "model"))
            .collect(),
        key_status: HashMap::new(),
        active: None,
    });
    let area = Rect::new(2, 2, 80, 8);
    let mut tree = Tree::new(node("models", page));
    tree.focus("models");
    tree.render(
        area,
        &mut Buffer::empty(area),
        &terminalui::Theme::default(),
    );
    tree.handle_event(&crate::test_support::input_event(
        KeyCode::End,
        KeyModifiers::NONE,
    ));
    tree.render(
        area,
        &mut Buffer::empty(area),
        &terminalui::Theme::default(),
    );

    tree.handle_event(&click(3, 5));
    let actions = tree
        .find_mut::<ModelsPage>("models")
        .unwrap()
        .take_pending();
    assert!(
        matches!(&actions[..], [ModelsAction::AskKeyForExpand { provider }] if provider == "provider07")
    );
}

fn ctrl_d() -> InputEvent {
    crate::test_support::input_event(KeyCode::Char('d'), KeyModifiers::CONTROL)
}

#[test]
fn deleting_a_key_requires_confirmation_and_carries_the_provider() {
    let mut tree = Tree::new(node("models", page_with_keys()));
    tree.focus("models");
    tree.handle_event(&ctrl_d());

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

    tree.handle_event(&crate::test_support::input_event(
        KeyCode::Enter,
        KeyModifiers::NONE,
    ));
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
    tree.handle_event(&ctrl_d());
    tree.handle_event(&crate::test_support::input_event(
        KeyCode::Left,
        KeyModifiers::NONE,
    ));
    tree.handle_event(&crate::test_support::input_event(
        KeyCode::Enter,
        KeyModifiers::NONE,
    ));

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
fn arrow_keys_move_between_the_delete_buttons() {
    let mut tree = Tree::new(node("models", page_with_keys()));
    tree.focus("models");
    tree.handle_event(&ctrl_d());

    assert_eq!(
        tree.find::<ModelsPage>("models").expect("page").focus_id(),
        Some("delete_key_confirm")
    );
    tree.handle_event(&crate::test_support::input_event(
        KeyCode::Left,
        KeyModifiers::NONE,
    ));
    assert_eq!(
        tree.find::<ModelsPage>("models").expect("page").focus_id(),
        Some("delete_key_cancel")
    );
    tree.handle_event(&crate::test_support::input_event(
        KeyCode::Right,
        KeyModifiers::NONE,
    ));
    assert_eq!(
        tree.find::<ModelsPage>("models").expect("page").focus_id(),
        Some("delete_key_confirm")
    );
}

#[test]
fn escape_cancels_the_delete_dialog() {
    let mut tree = Tree::new(node("models", page_with_keys()));
    tree.focus("models");
    tree.handle_event(&ctrl_d());
    tree.handle_event(&crate::test_support::input_event(
        KeyCode::Esc,
        KeyModifiers::NONE,
    ));

    let page = tree.find_mut::<ModelsPage>("models").expect("page");
    assert!(matches!(page.mode, Mode::Listing));
    assert!(page.take_pending().is_empty());
}

#[test]
fn the_confirmation_names_the_provider() {
    let mut tree = Tree::new(node("models", page_with_keys()));
    tree.focus("models");
    tree.handle_event(&ctrl_d());

    let sheet = crate::test_support::rendered(&mut tree);
    assert!(
        sheet.contains("Delete the saved key for openrouter?"),
        "{sheet}"
    );
}

#[test]
fn typing_d_still_filters_the_catalogue() {
    let mut page = page_with_keys();
    page.handle_key(&crate::test_support::key_event(
        KeyCode::Char('d'),
        KeyModifiers::NONE,
    ));
    assert_eq!(page.filter.value(), "d");
}

#[test]
fn an_environment_key_cannot_be_deleted() {
    let mut page = ModelsPage::new();
    page.set_data(ModelsData {
        models: vec![crate::test_support::model_info("openrouter", "one")],
        key_status: HashMap::from([(
            "openrouter".to_owned(),
            crate::test_support::provider_key_status(
                norte_code_wire::ProviderKeySource::Environment,
                &["OPENROUTER_API_KEY"],
            ),
        )]),
        active: None,
    });
    let mut tree = Tree::new(node("models", page));
    assert!(tree.focus("models"));

    tree.handle_event(&ctrl_d());

    let page = tree.find::<ModelsPage>("models").expect("page");
    assert!(matches!(page.mode, Mode::Listing));
    assert!(
        tree.find_mut::<ModelsPage>("models")
            .expect("page")
            .take_pending()
            .is_empty()
    );
}

#[test]
fn plain_delete_is_left_to_the_text_input() {
    let mut page = page_with_keys();

    assert_eq!(
        page.handle_key(&crate::test_support::key_event(
            KeyCode::Delete,
            KeyModifiers::NONE
        )),
        Outcome::Ignored
    );
    assert!(matches!(page.mode, Mode::Listing));
}

#[test]
fn ctrl_d_does_not_move_the_selection() {
    let mut page = page_with_keys();
    page.selected = 1;
    let mut tree = Tree::new(node("models", page));
    tree.focus("models");
    tree.handle_event(&ctrl_d());

    let page = tree.find::<ModelsPage>("models").expect("page");
    assert_eq!(page.selected, 1);
    assert!(matches!(
        page.mode,
        Mode::ConfirmDelete { ref provider } if provider == "anthropic"
    ));
}

#[test]
fn ctrl_delete_also_opens_the_confirmation() {
    let mut page = page_with_keys();

    assert_eq!(
        page.handle_key(&crate::test_support::key_event(
            KeyCode::Delete,
            KeyModifiers::CONTROL,
        )),
        Outcome::Consumed
    );
    assert!(matches!(
        page.mode,
        Mode::ConfirmDelete { ref provider } if provider == "openrouter"
    ));
}

#[test]
fn plain_down_still_moves_the_selection() {
    let mut page = page_with_keys();

    page.handle_key(&crate::test_support::key_event(
        KeyCode::Down,
        KeyModifiers::NONE,
    ));

    assert_eq!(page.selected, 1);
}

#[test]
fn plain_space_still_activates_the_selection() {
    let mut tree = Tree::new(node("models", page_with_keys()));
    tree.focus("models");
    tree.handle_event(&crate::test_support::input_event(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
    ));

    let actions = tree
        .find_mut::<ModelsPage>("models")
        .expect("page")
        .take_pending();
    assert!(
        matches!(&actions[..], [ModelsAction::ToggleExpand { provider }] if provider == "openrouter")
    );
}

#[test]
fn backspace_with_an_empty_filter_keeps_the_selection() {
    let mut page = page_with_keys();
    page.selected = 1;

    assert_eq!(
        page.handle_key(&crate::test_support::key_event(
            KeyCode::Backspace,
            KeyModifiers::NONE
        )),
        Outcome::Ignored
    );
    assert_eq!(page.selected, 1);
    assert!(matches!(page.mode, Mode::Listing));
}

#[test]
fn backspace_with_filter_text_still_edits_the_filter() {
    let mut page = page_with_keys();
    page.filter.set_value("open");
    page.selected = 1;

    assert_eq!(
        page.handle_key(&crate::test_support::key_event(
            KeyCode::Backspace,
            KeyModifiers::NONE
        )),
        Outcome::Consumed
    );
    assert_eq!(page.filter.value(), "ope");
    assert_eq!(page.selected, 0);
}

#[test]
fn the_rail_names_the_environment_variable_holding_a_key() {
    let mut page = ModelsPage::new();
    page.set_data(ModelsData {
        models: vec![crate::test_support::model_info("openrouter", "one")],
        key_status: HashMap::from([(
            "openrouter".to_owned(),
            crate::test_support::provider_key_status(
                norte_code_wire::ProviderKeySource::Environment,
                &["OPENROUTER_API_KEY"],
            ),
        )]),
        active: None,
    });
    let mut tree = Tree::new(node("models", page));
    assert!(tree.focus("models"));

    let sheet = crate::test_support::rendered(&mut tree);

    assert!(sheet.contains("source   env OPENROUTER_API_KEY"), "{sheet}");
}

#[test]
fn the_rail_shows_a_saved_key_without_naming_an_environment_variable() {
    let mut tree = Tree::new(node("models", page_with_keys()));
    assert!(tree.focus("models"));

    let sheet = crate::test_support::rendered(&mut tree);

    assert!(sheet.contains("source   saved key"), "{sheet}");
}

#[test]
fn the_rail_shows_the_source_for_a_model_row() {
    let mut page = page_with_keys();
    page.expanded.insert("openrouter".to_owned());
    page.selected = 1;
    let mut tree = Tree::new(node("models", page));
    assert!(tree.focus("models"));

    let sheet = crate::test_support::rendered(&mut tree);

    assert!(sheet.contains("provider openrouter"), "{sheet}");
    assert!(sheet.contains("source   saved key"), "{sheet}");
}
