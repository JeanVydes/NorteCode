//! The costs page: the ledger above, the breakdown below.

use norte_code_wire::{TelemetryRecord, TelemetryReport};

use crate::components::Component;
use crate::components::views::costs::CostsView;
use crate::ui_model::UiModel;

fn record(provider: &str, model: &str, cost: f64) -> TelemetryRecord {
    TelemetryRecord {
        id: format!("{provider}-{model}"),
        provider: provider.to_owned(),
        model: model.to_owned(),
        cost_usd: Some(cost),
        input_tokens: Some(100),
        output_tokens: Some(50),
        ..TelemetryRecord::default()
    }
}

fn drawn(model: &UiModel) -> String {
    use ratatui::{Terminal, backend::TestBackend};

    let mut view = CostsView::default();
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
fn the_ledger_totals_are_named() {
    let mut model = UiModel::default();
    model.costs = Some(norte_code_client::CostReport {
        total_usd: Some(1.25),
        requests: 9,
    });

    let text = drawn(&model);

    assert!(text.contains("LEDGER"), "drawn:\n{text}");
    assert!(text.contains("$1.2500"), "drawn:\n{text}");
    assert!(text.contains("9 requests"), "drawn:\n{text}");
}

#[test]
fn the_breakdown_lists_the_models_it_spent_on() {
    let mut model = UiModel::default();
    model.telemetry = Some(TelemetryReport {
        session: None,
        records: vec![
            record("openai", "gpt-4o", 0.5),
            record("openai", "gpt-4o", 0.25),
            record("anthropic", "claude", 1.0),
        ],
        ..TelemetryReport::default()
    });

    let text = drawn(&model);

    assert!(text.contains("BY MODEL"), "drawn:\n{text}");
    assert!(text.contains("anthropic/claude"), "drawn:\n{text}");
    assert!(text.contains("openai/gpt-4o"), "drawn:\n{text}");
    assert!(text.contains("$1.0000"), "drawn:\n{text}");
    assert!(text.contains("$0.7500"), "drawn:\n{text}");
}

#[test]
fn a_page_without_telemetry_says_why() {
    let model = UiModel::default();

    let text = drawn(&model);

    assert!(
        text.contains("No request has been recorded"),
        "drawn:\n{text}"
    );
}
