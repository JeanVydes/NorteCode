//! The shared hints render Spanish labels with real key caps.

use norte_code_wire::ProviderKeySource;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use terminalui::{Theme, Tree, View, node};

use super::{ShortcutHint, key_source_label};

#[test]
fn chat_hint_shows_labels_and_key_caps() {
    let theme = Theme::default();
    let hint = ShortcutHint::chat(&theme);
    let width = hint.measure().map_or(40, |(width, _)| width);
    let area = Rect::new(0, 0, width, 1);
    let mut buf = Buffer::empty(area);
    let mut tree = Tree::new(node("hint", hint));
    tree.render(area, &mut buf, &theme);
    let text: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
    assert!(text.contains("[ Alt+0-9 ]"), "text was {text}");
    assert!(text.contains("tabs"), "text was {text}");
    assert!(text.contains("send"), "text was {text}");
    assert!(text.contains("newline"), "text was {text}");
}

#[test]
fn key_source_names_the_environment_variable() {
    let keys = vec!["EXA_API_KEY".to_owned(), "EXA_TOKEN".to_owned()];
    assert_eq!(
        key_source_label(ProviderKeySource::Environment, &keys),
        "env EXA_API_KEY or EXA_TOKEN"
    );
}

#[test]
fn key_source_distinguishes_saved_keyring_and_missing_keys() {
    let keys = vec!["EXA_API_KEY".to_owned()];
    assert_eq!(
        key_source_label(ProviderKeySource::File, &keys),
        "saved key"
    );
    assert_eq!(
        key_source_label(ProviderKeySource::Keyring, &keys),
        "keyring"
    );
    assert_eq!(
        key_source_label(ProviderKeySource::Missing, &keys),
        "env EXA_API_KEY (not set)"
    );
    assert_eq!(key_source_label(ProviderKeySource::Missing, &[]), "none");
    assert_eq!(
        key_source_label(ProviderKeySource::NotNeeded, &[]),
        "not needed"
    );
    assert_eq!(key_source_label(ProviderKeySource::Unknown, &[]), "unknown");
}
