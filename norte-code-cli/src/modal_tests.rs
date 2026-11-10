use super::*;

/// An editor over three tools, which is the shape `/tools` builds.
fn tools() -> Modal {
    Modal::editable(
        "tools",
        vec![
            Row::heading("rules"),
            Row::actable("read", Action::Deny("read".to_owned())).noted("allow"),
            Row::actable("shell", Action::Allow("shell".to_owned())).noted("deny"),
            Row::actable("write", Action::Allow("write".to_owned())).noted("deny"),
        ],
    )
}

#[test]
fn the_panel_takes_most_of_the_screen_and_not_all_of_it() {
    // A panel that covered everything would be indistinguishable from the
    // program having changed, and the conversation around the edges is what
    // says it can be closed.
    let screen = Rect::new(0, 0, 100, 40);
    let panel = panel_area(screen);
    assert!(panel.width < screen.width, "{panel:?}");
    assert!(panel.height < screen.height, "{panel:?}");
    assert!(panel.width > screen.width / 2, "{panel:?}");
    assert!(panel.height > screen.height / 2, "{panel:?}");
}

#[test]
fn an_empty_panel_is_not_drawn() {
    // A command that reported nothing is not a panel with nothing in it: a
    // frame around an empty box says something went wrong.
    assert!(Modal::report("nothing", Vec::new()).is_empty());
}

#[test]
fn the_cursor_starts_on_a_row_that_can_be_acted_on() {
    // A heading is drawn and never landed on: a cursor that started on one
    // would make Enter do nothing on the first thing a person sees.
    let modal = tools();
    assert_eq!(modal.cursor, 1, "the first actable row, not the heading");
    assert_eq!(
        modal.rows[modal.cursor].action,
        Some(Action::Deny("read".to_owned()))
    );
}

#[test]
fn the_cursor_skips_headings_and_holds_at_both_ends() {
    let mut modal = Modal::editable(
        "t",
        vec![
            Row::heading("h"),
            Row::actable("a", Action::Allow("a".to_owned())),
            Row::heading("h2"),
            Row::actable("b", Action::Allow("b".to_owned())),
        ],
    );
    assert_eq!(modal.cursor, 1);
    modal.move_by(1);
    assert_eq!(modal.cursor, 3, "the heading is stepped over");
    // And it holds: a key at the end does not wrap, because a row that
    // depends on how many times a key was pressed is a row nobody can aim
    // at.
    modal.move_by(1);
    assert_eq!(modal.cursor, 3);
    modal.move_by(-1);
    assert_eq!(modal.cursor, 1);
    modal.move_by(-1);
    assert_eq!(modal.cursor, 1);
}

#[test]
fn enter_acts_on_the_row_under_the_cursor_and_carries_the_scope() {
    // The panel does not know what allowing a tool does: what it produces is
    // the intent, and the loop that owns the service is what acts on it.
    let mut modal = tools();
    assert_eq!(
        modal.key(Key::Enter),
        Choice::Act {
            action: Action::Deny("read".to_owned()),
            scope: Scope::Session,
        }
    );
    // The scope is the panel's, and it is carried by the decision.
    assert_eq!(modal.key(Key::ToggleScope), Choice::None);
    assert_eq!(modal.scope, Scope::Global);
    assert_eq!(
        modal.key(Key::Enter),
        Choice::Act {
            action: Action::Deny("read".to_owned()),
            scope: Scope::Global,
        }
    );
}

#[test]
fn a_report_has_no_cursor_and_no_actions() {
    // A report and an editor are one widget: a report has no actable rows,
    // so Enter is nothing and the cursor never moves.
    let mut modal = Modal::report("tools", vec!["a".to_owned(), "b".to_owned()]);
    assert!(!modal.editable);
    assert_eq!(modal.key(Key::Down), Choice::None);
    assert_eq!(modal.cursor, 0);
    assert_eq!(modal.key(Key::Enter), Choice::None);
    assert_eq!(modal.key(Key::Escape), Choice::Closed);
}

#[test]
fn scrolling_clamps_at_both_ends() {
    let mut modal = Modal::report("tools", (0..100).map(|n| format!("line {n}")).collect());
    modal.scroll_by(-1_000, 10);
    assert_eq!(modal.top, 0, "there is nothing above the first line");
    modal.scroll_by(10_000, 10);
    assert_eq!(
        modal.top, 90,
        "100 lines, ten shown: ninety is as far as the top goes"
    );
}
