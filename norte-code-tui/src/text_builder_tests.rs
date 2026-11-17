use crate::{components::ComponentId, text_builder::TextBuilder};

#[test]
fn a_ticker_shows_the_start_when_its_source_fits() {
    let source = TextBuilder::footer_help(ComponentId::Plans);

    assert_eq!(
        TextBuilder::footer_ticker(ComponentId::Plans, 0, 200)
            .chars()
            .count(),
        source.chars().count()
    );
}

#[test]
fn a_ticker_slides_the_shortcuts_left() {
    let first = TextBuilder::footer_ticker(ComponentId::Session, 0, 40);
    let shifted = TextBuilder::footer_ticker(ComponentId::Session, 1, 40);

    assert_eq!(first.chars().count(), 40);
    assert_eq!(shifted.chars().count(), 40);
    assert_ne!(first, shifted);
}

#[test]
fn a_window_cannot_cut_a_wide_character_in_half() {
    let line = TextBuilder::footer_ticker(ComponentId::Session, 1, 7);

    assert_eq!(line.chars().count(), 7);
}

#[test]
fn the_header_marks_the_model_and_the_queue_and_reasoning_buttons() {
    use crate::ui_model::{SessionModel, SessionStatsSummary, UiModel};

    let mut model = UiModel::default();
    model.session_model = Some(SessionModel {
        provider: "openrouter".to_owned(),
        model: "particle-1:free".to_owned(),
    });
    model.session_stats = Some(SessionStatsSummary {
        requests: 3,
        input_tokens: 12_000,
        output_tokens: 340,
        cache_read_tokens: 9_600,
        cache_write_tokens: 100,
        cost_usd: 0.001,
        context_used: 12_000,
        context_window: Some(200_000),
    });
    let layout = TextBuilder::header_layout(&model);

    assert_eq!(layout.buttons.len(), 3);
    assert_eq!(
        layout.buttons[0].2,
        crate::text_builder::HeaderAction::OpenView(ComponentId::Models)
    );
    assert_eq!(
        layout.buttons[1].2,
        crate::text_builder::HeaderAction::Modal(crate::components::HeaderModal::Queue)
    );
    assert_eq!(
        layout.buttons[2].2,
        crate::text_builder::HeaderAction::Modal(crate::components::HeaderModal::Reasoning)
    );

    let drawn: Vec<char> = layout
        .line
        .spans
        .iter()
        .flat_map(|span| span.content.chars())
        .collect();
    let whole = drawn.iter().collect::<String>();
    assert!(whole.contains("3 req · 12.0k in · 340 out · 9.6k cached · $0.0010"));
    let button_text = |(start, end): (usize, usize)| {
        drawn[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_owned()
    };

    assert_eq!(
        button_text((layout.buttons[0].0, layout.buttons[0].1)),
        "openrouter/particle-1:free"
    );
    assert_eq!(
        button_text((layout.buttons[1].0, layout.buttons[1].1)),
        format!(
            "queue {}",
            match model.send_mode {
                crate::ui_model::SendMode::Steer => "steer",
                crate::ui_model::SendMode::Queue => "queue",
                crate::ui_model::SendMode::Interrupt => "interrupt",
                crate::ui_model::SendMode::Reject => "reject",
            }
        )
    );
    assert_eq!(
        button_text((layout.buttons[2].0, layout.buttons[2].1)),
        format!("reasoning {}", model.reasoning.label())
    );
}
