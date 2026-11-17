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
fn the_header_marks_the_queue_and_reasoning_buttons() {
    use crate::ui_model::UiModel;

    let model = UiModel::default();
    let layout = TextBuilder::header_layout(&model);

    assert_eq!(layout.buttons.len(), 2);
    assert_eq!(layout.buttons[0].2, crate::components::HeaderModal::Queue);
    assert_eq!(
        layout.buttons[1].2,
        crate::components::HeaderModal::Reasoning
    );

    let drawn: Vec<char> = layout
        .line
        .spans
        .iter()
        .flat_map(|span| span.content.chars())
        .collect();
    let button_text = |(start, end): (usize, usize)| {
        drawn[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_owned()
    };

    assert_eq!(
        button_text((layout.buttons[0].0, layout.buttons[0].1)),
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
        button_text((layout.buttons[1].0, layout.buttons[1].1)),
        format!("reasoning {}", model.reasoning.label())
    );
}
