use super::{ClearAction, ClearDataOverlay};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};
use terminalui::{Theme, Tree, node};

#[test]
fn confirmation_defaults_to_cancel_and_mouse_hits_the_visible_button() {
    let mut view = ClearDataOverlay::default();
    view.open();
    assert!(matches!(
        view.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ClearAction::Cancel)
    ));
    let mut tree = Tree::new(node("dialog", view));
    let area = Rect::new(0, 0, 80, 24);
    let mut buffer = Buffer::empty(area);
    tree.render(area, &mut buffer, &Theme::default());
    let view = tree.find_mut::<ClearDataOverlay>("dialog").unwrap();
    let area = view.confirm_area;
    assert!(matches!(
        view.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE
        }),
        Some(ClearAction::Confirm)
    ));
}

#[test]
fn confirmation_remains_visible_in_a_small_terminal() {
    let mut view = ClearDataOverlay::default();
    view.open();
    let mut tree = Tree::new(node("dialog", view));
    let area = Rect::new(0, 0, 20, 6);
    let mut buffer = Buffer::empty(area);
    tree.render(area, &mut buffer, &Theme::default());
    let row = (0..area.width)
        .map(|x| buffer[(x, 3)].symbol())
        .collect::<String>();
    assert!(row.contains("Clear data?"));
}
