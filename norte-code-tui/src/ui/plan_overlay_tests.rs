//! Rendering behavior of the plan preview overlay.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::{InputEvent, Theme, Tree};

use super::{PlanOverlay, PlanReviewAction};
use crate::theme::{ThemeChoice, ThemeKind};
use crate::ui::{build_tree, sync_views};
use crate::ui_model::UiModel;

fn review_tree() -> (Tree, Theme) {
    let model = UiModel::detached();
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    tree.find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .begin_review("q_1".to_owned(), "# Plan".to_owned());
    sync_views(&mut tree, &model);
    (tree, theme)
}

fn press(tree: &mut Tree, code: KeyCode) {
    tree.handle_event(&InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn overlay_text(content: &str) -> String {
    let model = UiModel::detached();
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    tree.find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .open(norte_code_wire::PlanSummary {
            id: "plan-1".to_owned(),
            content: content.to_owned(),
            ..Default::default()
        });
    sync_views(&mut tree, &model);
    let area = Rect::new(0, 0, 80, 20);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_preview_numbers_markdown_lines_from_one() {
    let text = overlay_text("# Heading\n\nBody");

    assert!(text.contains("Heading"), "{text}");
    assert!(text.contains("Body"), "{text}");
    assert!(text.contains("   1  Heading"), "{text}");
    assert!(text.contains("   2  Body"), "{text}");
}

#[test]
fn the_preview_renders_inline_emphasis() {
    let text = overlay_text("Use **ID:** 42");

    assert!(text.contains("ID: 42"), "{text}");
    assert!(!text.contains("**"), "{text}");
}

#[test]
fn the_preview_repaints_when_the_theme_changes() {
    let mut model = UiModel::detached();
    let dark = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    tree.find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .open(norte_code_wire::PlanSummary {
            id: "plan-1".to_owned(),
            content: "# Heading".to_owned(),
            ..Default::default()
        });
    sync_views(&mut tree, &model);
    let area = Rect::new(0, 0, 80, 20);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &dark);

    model.preferences.theme = ThemeChoice::BuiltIn(ThemeKind::ByeRetina);
    let light = model.preferences.catalog.theme(&model.preferences.theme);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &light);

    let repainted = (0..area.height).any(|y| {
        (0..area.width).any(|x| {
            let cell = &buf[(x, y)];
            cell.symbol() == "H" && cell.fg == light.accent
        })
    });
    assert!(repainted);
}

#[test]
fn clicking_accept_reports_the_plan_decision() {
    let model = UiModel::detached();
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    tree.find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .begin_review("q_1".to_owned(), "# Plan".to_owned());
    sync_views(&mut tree, &model);
    let area = Rect::new(0, 0, 80, 20);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);

    let (row, line) = (0..area.height)
        .map(|y| {
            let line = (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>();
            (y, line)
        })
        .find(|(_, line)| line.contains(" accept "))
        .expect("accept button");
    let column = u16::try_from(line.find(" accept ").expect("accept button") + 1).expect("column");
    let click = InputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
    tree.handle_event(&click);

    let action = tree
        .find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .take_action();
    assert_eq!(action, Some(("q_1".to_owned(), PlanReviewAction::Accept)));
}

#[test]
fn arrow_keys_move_the_selected_review_option() {
    let (mut tree, _) = review_tree();

    press(&mut tree, KeyCode::Right);
    press(&mut tree, KeyCode::Enter);

    let action = tree
        .find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .take_action();
    assert_eq!(action, Some(("q_1".to_owned(), PlanReviewAction::Decline)));
}

#[test]
fn left_arrow_stops_at_the_first_review_option() {
    let (mut tree, _) = review_tree();

    press(&mut tree, KeyCode::Right);
    press(&mut tree, KeyCode::Left);
    press(&mut tree, KeyCode::Enter);

    let action = tree
        .find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .take_action();
    assert_eq!(action, Some(("q_1".to_owned(), PlanReviewAction::Accept)));
}

#[test]
fn enter_on_the_feedback_option_opens_the_editor() {
    let (mut tree, _) = review_tree();

    press(&mut tree, KeyCode::Right);
    press(&mut tree, KeyCode::Right);
    press(&mut tree, KeyCode::Enter);
    for character in "no".chars() {
        press(&mut tree, KeyCode::Char(character));
    }
    press(&mut tree, KeyCode::Enter);

    let action = tree
        .find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .take_action();
    assert_eq!(
        action,
        Some((
            "q_1".to_owned(),
            PlanReviewAction::Feedback(
                crate::review_feedback::ReviewFeedback::new("no").expect("feedback")
            )
        ))
    );
}

#[test]
fn the_selected_review_option_is_painted_as_primary() {
    let (mut tree, theme) = review_tree();
    let area = Rect::new(0, 0, 80, 20);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);

    press(&mut tree, KeyCode::Right);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);

    let background_at = |label: &str| {
        (0..area.height)
            .find_map(|y| {
                let line = (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>();
                line.find(label)
                    .map(|column| buf[(u16::try_from(column).unwrap_or(0), y)].bg)
            })
            .expect("button label")
    };
    assert_eq!(background_at(" decline "), theme.accent);
    assert_eq!(background_at(" accept "), theme.muted);
}

#[test]
fn the_read_only_preview_keeps_its_options_inert() {
    let model = UiModel::detached();
    let mut tree = build_tree(&model);
    tree.find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree")
        .open(norte_code_wire::PlanSummary {
            id: "plan-1".to_owned(),
            content: "# Plan".to_owned(),
            ..Default::default()
        });
    sync_views(&mut tree, &model);

    press(&mut tree, KeyCode::Right);
    press(&mut tree, KeyCode::Enter);

    let overlay = tree
        .find_mut::<PlanOverlay>("plan_overlay")
        .expect("overlay in tree");
    assert!(overlay.is_open());
    assert_eq!(overlay.take_action(), None);
}
