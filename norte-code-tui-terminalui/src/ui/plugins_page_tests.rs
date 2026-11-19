//! The plugins page: toolbar actions, prompts and row toggling.

use super::{Action, Mode, PluginsAction, PluginsPage};

fn plugin(name: &str, enabled: bool) -> norte_code_client::PluginSummary {
    norte_code_client::PluginSummary {
        name: name.to_owned(),
        enabled,
        loaded: enabled,
        load_error: None,
    }
}

fn page_with(plugins: Vec<norte_code_client::PluginSummary>) -> PluginsPage {
    let mut page = PluginsPage::new();
    page.set_data(plugins);
    page
}

fn key(character: char) -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    crossterm::event::KeyEvent {
        code: KeyCode::Char(character),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    }
}

fn enter() -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    crossterm::event::KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    }
}

#[test]
fn enter_on_a_row_asks_to_toggle_it() {
    let mut page = page_with(vec![plugin("north-code", true)]);
    page.handle_listing_key(&enter());
    let actions = page.take_pending();
    assert!(matches!(
        &actions[..],
        [PluginsAction::SetEnabled { name, enabled }] if name == "north-code" && !*enabled
    ));
}

#[test]
fn install_prompts_for_repository_then_package() {
    let mut page = page_with(Vec::new());
    page.start_install();
    for character in "https://example.com/repo".chars() {
        let _ = page.type_path(&key(character));
    }
    assert!(page.type_path(&enter()));
    for character in "north-code-plugin-x".chars() {
        let _ = page.type_path(&key(character));
    }
    assert!(page.type_path(&enter()));
    let actions = page.take_pending();
    assert!(matches!(
        &actions[..],
        [PluginsAction::Install { url, package }]
            if url == "https://example.com/repo" && package.as_deref() == Some("north-code-plugin-x")
    ));
}

#[test]
fn install_with_a_blank_package_installs_the_whole_workspace() {
    let mut page = page_with(Vec::new());
    page.start_install();
    for character in "https://example.com/repo".chars() {
        let _ = page.type_path(&key(character));
    }
    assert!(page.type_path(&enter()));
    assert!(page.type_path(&enter()));
    let actions = page.take_pending();
    assert!(matches!(
        &actions[..],
        [PluginsAction::Install { package: None, .. }]
    ));
}

#[test]
fn an_invalid_repository_url_stays_in_the_prompt() {
    let mut page = page_with(Vec::new());
    page.start_install();
    for character in "not a url".chars() {
        let _ = page.type_path(&key(character));
    }
    assert!(page.type_path(&enter()));
    assert_eq!(page.mode, Mode::Repository);
    assert!(page.take_pending().is_empty());
}

#[test]
fn an_invalid_package_name_stays_in_the_prompt() {
    let mut page = page_with(Vec::new());
    page.start_install();
    for character in "https://example.com/repo".chars() {
        let _ = page.type_path(&key(character));
    }
    assert!(page.type_path(&enter()));
    for character in "UPPER _CASE!".chars() {
        let _ = page.type_path(&key(character));
    }
    assert!(page.type_path(&enter()));
    assert_eq!(page.mode, Mode::Package);
    assert!(page.take_pending().is_empty());
}

#[test]
fn a_job_occupies_one_row() {
    let mut page = page_with(vec![plugin("kept", true)]);
    page.area = ratatui::layout::Rect::new(0, 0, 80, 20);
    let baseline = page.body_rows();
    page.set_job(Some(crate::ui_model::PluginJob {
        operation: "install".to_string(),
        step: 2,
        total: 5,
        stage: "cloning".to_string(),
    }));
    assert_eq!(page.body_rows(), baseline - 1);
    page.set_job(None);
    assert_eq!(page.body_rows(), baseline);
}

#[test]
fn remove_needs_the_confirmation() {
    let mut page = page_with(vec![plugin("doomed", true)]);
    page.run(Action::Remove);
    assert!(page.take_pending().is_empty());
    assert!(page.type_path(&enter()));
    let actions = page.take_pending();
    assert!(matches!(&actions[..], [PluginsAction::Remove { name }] if name == "doomed"));
}

#[test]
fn update_and_update_all_go_out_as_actions() {
    let mut page = page_with(vec![plugin("kept", true)]);
    page.run(Action::Update);
    page.run(Action::UpdateAll);
    let actions = page.take_pending();
    assert_eq!(actions.len(), 2);
}
