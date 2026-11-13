use norte_code_wire::{Delta, MessageRole, ReplayedKind, ServerEvent, ServerEventKind};
use ratatui::{layout::Alignment, style::Modifier};

use super::{ThinkingVisibility, TranscriptRenderer};
use crate::{tools::DiffMode, ui_model::TranscriptItem};

#[test]
fn combines_adjacent_text_deltas_into_one_assistant_message() {
    let items = [
        ServerEventKind::Delta {
            turn: 1,
            delta: Delta::Text {
                index: 0,
                text: "Hello ".to_owned(),
            },
        },
        ServerEventKind::Delta {
            turn: 1,
            delta: Delta::Text {
                index: 0,
                text: "world".to_owned(),
            },
        },
    ]
    .map(|kind| {
        TranscriptItem::ServerEvent(Box::new(ServerEvent {
            request_id: 1,
            session: None,
            kind,
        }))
    });

    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);

    assert_eq!(lines.len(), 1);
    assert_eq!(line_content(&lines[0]), "  norte  Hello world  ");
}

#[test]
fn styles_replayed_tool_calls_from_the_wire_enum() {
    let items = [TranscriptItem::ServerEvent(Box::new(ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::MessageReplayed {
            index: 1,
            role: MessageRole::Assistant,
            kind: ReplayedKind::ToolCall,
            text: "edit".to_owned(),
        },
    }))];

    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);

    assert_eq!(line_content(&lines[0]), "assistant  edit");
    assert_eq!(
        lines[0].spans[0].style.fg,
        Some(crate::components::palette::warning())
    );
}

#[test]
fn combines_reasoning_deltas_as_one_collapsed_thought_by_default() {
    let items = ["The first part ", "**and the second part**"].map(|text| {
        TranscriptItem::ServerEvent(Box::new(ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Reasoning {
                    index: 0,
                    text: text.to_owned(),
                },
            },
        }))
    });

    let collapsed = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);
    let expanded = TranscriptRenderer::lines_with_thinking(
        &items,
        DiffMode::Unified,
        80,
        ThinkingVisibility::Expanded,
    );

    assert_eq!(collapsed.len(), 1);
    assert!(line_content(&collapsed[0]).contains("Thought"));
    assert!(line_content(&expanded[0]).contains("Thought"));
    assert!(
        expanded
            .iter()
            .any(|line| line_content(line).contains("The first part and the second part"))
    );
    assert!(
        expanded
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| {
                span.content.contains('s') && span.style.add_modifier.contains(Modifier::BOLD)
            })
    );
}

#[test]
fn aligns_user_messages_to_the_right_and_renders_their_markdown() {
    let lines = TranscriptRenderer::lines(
        &[TranscriptItem::UserMessage {
            text: "**hello**".to_owned(),
            chips: Vec::new(),
        }],
        DiffMode::Unified,
        80,
    );

    assert_eq!(lines[0].alignment, Some(Alignment::Right));
    assert!(!line_content(&lines[0]).contains("you"));
    assert!(line_content(&lines[0]).contains("hello"));
    assert_eq!(
        lines[0].style.bg,
        Some(crate::components::palette::user_message_background(
            crate::ui_model::ThemeMode::Dark
        ))
    );
    assert!(lines[0]
        .spans
        .iter()
        .any(|span| span.content == "hello" && span.style.add_modifier.contains(Modifier::BOLD)));
}

#[test]
fn assistant_messages_have_their_own_padded_surface() {
    let lines = TranscriptRenderer::lines(
        &[TranscriptItem::ServerEvent(Box::new(ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Text {
                    index: 0,
                    text: "hello".to_owned(),
                },
            },
        }))],
        DiffMode::Unified,
        80,
    );

    assert!(line_content(&lines[0]).starts_with("  norte  hello"));
    assert_eq!(
        lines[0].style.bg,
        Some(crate::components::palette::assistant_message_background(
            crate::ui_model::ThemeMode::Dark
        ))
    );
}

#[test]
fn thought_content_stays_gray_and_uses_a_moving_palette_background() {
    let items = [TranscriptItem::ServerEvent(Box::new(ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::Delta {
            turn: 1,
            delta: Delta::Reasoning {
                index: 0,
                text: "**private thought**".to_owned(),
            },
        },
    }))];
    let lines = TranscriptRenderer::lines_with_thinking(
        &items,
        DiffMode::Unified,
        80,
        ThinkingVisibility::Expanded,
    );

    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter(|span| !span.content.trim().is_empty())
            .all(|span| span.style.fg == Some(crate::components::palette::thinking_foreground()))
    );
    assert!(lines.iter().any(|line| {
        line.spans.iter().any(|span| {
            span.style.bg
                == Some(crate::components::palette::thinking_background(
                    0,
                    0,
                    80,
                    crate::ui_model::ThemeMode::Dark,
                ))
        })
    }));
    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .all(|span| !span.content.contains("to expand"))
    );
}

#[test]
fn the_chips_of_a_message_are_drawn_under_it() {
    let items = [TranscriptItem::UserMessage {
        text: "mira esto".to_owned(),
        chips: vec![crate::chips::Chip {
            content: crate::chips::ChipContent::Text("un texto largo".to_owned()),
            label: "text".to_owned(),
            detail: "14 chars".to_owned(),
            kind: None,
        }],
    }];
    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);

    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(drawn.contains("mira esto"));
    assert!(drawn.contains("text"));
    assert!(drawn.contains("14 chars"));
}

#[test]
fn a_provider_retry_names_the_attempt_the_delay_and_the_reason() {
    let items = [TranscriptItem::ServerEvent(Box::new(ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::RetryScheduled {
            turn: 1,
            attempt: 2,
            delay_ms: 10_000,
            reason: "429 too many requests".to_owned(),
        },
    }))];
    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);

    let drawn = line_content(&lines[0]);
    assert!(drawn.contains("attempt 2"));
    assert!(drawn.contains("10.0s"));
    assert!(drawn.contains("429 too many requests"));
}

#[test]
fn a_reasoning_downgrade_is_shown_as_a_state_change() {
    let items = [TranscriptItem::ServerEvent(Box::new(ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::ReasoningDowngraded {
            from: "high".to_owned(),
            to: "medium".to_owned(),
            reason: "model supports only medium and high".to_owned(),
        },
    }))];
    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);

    let drawn = line_content(&lines[0]);
    assert!(drawn.contains("high"));
    assert!(drawn.contains("medium"));
}

#[test]
fn the_usage_footer_names_the_model_the_tokens_and_the_cost() {
    let items = [TranscriptItem::Usage(crate::ui_model::UsageSummary {
        provider: Some("openai".to_owned()),
        model: Some("gpt-5".to_owned()),
        input_tokens: Some(1200),
        output_tokens: Some(340),
        cache_read_tokens: Some(900),
        cost_usd: Some(0.25),
        duration_ms: Some(820),
        tokens_per_second: Some(42.5),
    })];
    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);

    let footer = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(footer.contains("openai/gpt-5"));
    assert!(footer.contains("1.2k in"));
    assert!(footer.contains("340 out"));
    assert!(footer.contains("900 cached"));
    assert!(footer.contains("820 ms"));
    assert!(footer.contains("$0.2500"));
}

fn line_content(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}
