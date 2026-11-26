use super::{HostOverlay, wrap_fields};
use crate::ui::detail_panel::Field;
use crate::ui_model::UiModel;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};
use terminalui::{InputEvent, Theme, Tree, node};

#[test]
fn wraps_values_without_losing_information() {
    let fields = vec![Field::new("address", "https://long.example.test/path")];
    let rows = wrap_fields(&fields, 8);
    assert_eq!(rows[0].0, "address");
    assert!(rows.iter().skip(1).all(|(label, _)| label.is_empty()));
    assert_eq!(
        rows.into_iter().map(|(_, value)| value).collect::<String>(),
        fields[0].value
    );
}

#[test]
fn scrolls_long_host_information_and_closes_with_escape() {
    let mut model = UiModel::detached();
    model.service_url = "https://example.test/".repeat(8);
    let mut overlay = HostOverlay::default();
    overlay.open(&model);
    let mut tree = Tree::new(node("host", overlay));
    assert!(tree.focus("host"));
    let area = Rect::new(0, 0, 35, 12);
    tree.render(area, &mut Buffer::empty(area), &Theme::default());
    tree.handle_event(&InputEvent::Key(KeyEvent::new(
        KeyCode::End,
        KeyModifiers::NONE,
    )));
    assert!(tree.find::<HostOverlay>("host").unwrap().scroll > 0);
    tree.handle_event(&InputEvent::Key(KeyEvent::new(
        KeyCode::Esc,
        KeyModifiers::NONE,
    )));
    assert!(!tree.find::<HostOverlay>("host").unwrap().is_open());
}
