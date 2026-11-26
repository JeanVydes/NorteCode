use super::CommandDock;
use ratatui::{buffer::Buffer, layout::Rect};
use std::time::Duration;
use terminalui::{Theme, Tree, node};

#[test]
fn feedback_slides_in_then_expires_and_progress_waits_for_completion() {
    let mut tree = Tree::new(node("dock", CommandDock::default()));
    let area = Rect::new(0, 0, 80, 24);
    let dock = tree.find_mut::<CommandDock>("dock").unwrap();
    dock.set_chat_visible(true);
    dock.show("Chart density: 4", false);
    tree.tick(Duration::from_millis(300));
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &Theme::default());
    let row = (0..80).map(|x| buf[(x, 16)].symbol()).collect::<String>();
    assert!(row.contains("Chart density: 4"), "{row}");
    tree.tick(Duration::from_millis(5001));
    assert!(!tree.find::<CommandDock>("dock").unwrap().is_active());
    tree.find_mut::<CommandDock>("dock")
        .unwrap()
        .show("Compacting", true);
    tree.tick(Duration::from_secs(20));
    assert!(tree.find::<CommandDock>("dock").unwrap().is_active());
    tree.find_mut::<CommandDock>("dock")
        .unwrap()
        .finish_progress("Context compacted");
    tree.tick(Duration::from_millis(5301));
    assert!(!tree.find::<CommandDock>("dock").unwrap().is_active());
}
