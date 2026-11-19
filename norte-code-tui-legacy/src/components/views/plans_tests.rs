//! The plans page: the list on the left, the plan as markdown on the right.

use norte_code_wire::{PlanStatus, PlanSummary};
use ratatui::{Terminal, backend::TestBackend};

use super::PlansView;
use crate::components::Component;
use crate::ui_model::UiModel;

fn plan(content: &str) -> PlanSummary {
    PlanSummary {
        id: "plan-1".to_owned(),
        content: content.to_owned(),
        status: PlanStatus::Pending,
        title: Some("The plan".to_owned()),
        revision: 2,
        model: Some("openai/gpt-4o".to_owned()),
    }
}

fn drawn(view: &mut PlansView, model: &UiModel) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), model))
        .expect("draw");
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
fn the_plan_body_is_rendered_as_markdown() {
    let model = UiModel {
        plans: vec![plan("## Steps\n\n- install it\n- run it\n\n**done**")],
        ..UiModel::default()
    };
    let mut view = PlansView::default();

    let text = drawn(&mut view, &model);

    assert!(text.contains("Steps"), "drawn:\n{text}");
    assert!(text.contains("install it"), "drawn:\n{text}");
    assert!(text.contains('•'), "the list is bulleted:\n{text}");
    assert!(
        !text.contains("**done**"),
        "the markers are drawn, not printed:\n{text}"
    );
    assert!(text.contains("done"), "drawn:\n{text}");
}

#[test]
fn the_header_names_the_plan_and_its_state() {
    let model = UiModel {
        plans: vec![plan("# The plan")],
        ..UiModel::default()
    };
    let mut view = PlansView::default();

    let text = drawn(&mut view, &model);

    assert!(text.contains("The plan"), "drawn:\n{text}");
    assert!(text.contains("awaiting approval"), "drawn:\n{text}");
    assert!(text.contains("v2"), "drawn:\n{text}");
    assert!(text.contains("openai/gpt-4o"), "drawn:\n{text}");
}

#[test]
fn a_markdown_table_is_drawn() {
    let model = UiModel {
        plans: vec![plan("| step | owner |\n|---|---|\n| build | me |")],
        ..UiModel::default()
    };
    let mut view = PlansView::default();

    let text = drawn(&mut view, &model);

    assert!(text.contains('│'), "the table has borders:\n{text}");
    assert!(text.contains("build"), "drawn:\n{text}");
}

#[test]
fn an_empty_page_says_what_to_do() {
    let model = UiModel::default();
    let mut view = PlansView::default();

    let text = drawn(&mut view, &model);

    assert!(text.contains("No plans loaded"), "drawn:\n{text}");
}
