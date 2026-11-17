use norte_code_wire::{Delta, MessageRole, ReplayedKind, ServerEvent, ServerEventKind};
use ratatui::style::Modifier;

use super::TranscriptRenderer;
use crate::{
    tools::DiffMode,
    ui_model::{ThinkingVisibility, TranscriptItem},
};

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

    let lines = content_lines(TranscriptRenderer::lines(&items, DiffMode::Unified, 80));

    assert_eq!(lines.len(), 1);
    assert_eq!(line_content(&lines[0]), "Hello world");
}

#[test]
fn draws_a_replayed_tool_call_as_a_tool_call() {
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

    assert_eq!(line_content(&lines[0]), "✎ edit");
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

    let collapsed_all = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);
    let expanded = content_lines(TranscriptRenderer::lines_with_thinking(
        &items,
        DiffMode::Unified,
        80,
        ThinkingVisibility::Expanded,
        false,
    ));

    assert!(
        collapsed_all
            .iter()
            .any(|line| line_content(line).contains("Thought"))
    );
    let collapsed = content_lines(collapsed_all);
    assert!(collapsed.is_empty(), "a collapsed thought draws no body");
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

fn two_thought_turn() -> Vec<TranscriptItem> {
    let reasoning = |index: u32, text: &str| {
        TranscriptItem::ServerEvent(Box::new(ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Reasoning {
                    index,
                    text: text.to_owned(),
                },
            },
        }))
    };
    let answer = |text: &str| {
        TranscriptItem::ServerEvent(Box::new(ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Text {
                    index: 1,
                    text: text.to_owned(),
                },
            },
        }))
    };
    vec![
        reasoning(0, "first thought"),
        answer("answer"),
        reasoning(1, "second thought"),
    ]
}

fn thought_options(
    thinking: ThinkingVisibility,
    thoughts: &[super::ThoughtFold],
) -> super::render::TranscriptOptions<'_> {
    super::render::TranscriptOptions {
        diff_mode: DiffMode::Unified,
        width: 80,
        thinking_visibility: thinking,
        show_tool_output: crate::ui_model::Toggle::On,
        show_timestamps: crate::ui_model::Toggle::Off,
        theme: crate::ui_model::ThemeMode::Dark,
        developer_mode: crate::ui_model::Toggle::Off,
        shell_output_expanded: crate::ui_model::Toggle::Off,
        thinking_phase: 0,
        thinking_streaming: false,
        folded: &[],
        thoughts,
    }
}

#[test]
fn a_thought_expands_on_its_own() {
    let items = two_thought_turn();
    let fold = super::ThoughtFold {
        key: super::ThoughtKey {
            turn: super::TurnKey::Live(1),
            ordinal: 0,
        },
        expanded: true,
    };

    let annotated = super::render::lines_annotated(
        &items,
        thought_options(ThinkingVisibility::Collapsed, std::slice::from_ref(&fold)),
    );
    let drawn = annotated
        .lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        drawn.contains("first thought"),
        "the unfolded block draws its text"
    );
    assert!(
        !drawn.contains("second thought"),
        "the other block stays folded"
    );
    assert_eq!(annotated.thoughts.len(), 2);
    assert_eq!(annotated.thoughts[0].1.ordinal, 0);
    assert_eq!(annotated.thoughts[1].1.ordinal, 1);
}

#[test]
fn an_expanded_thought_folds_on_its_own() {
    let items = two_thought_turn();
    let fold = super::ThoughtFold {
        key: super::ThoughtKey {
            turn: super::TurnKey::Live(1),
            ordinal: 1,
        },
        expanded: false,
    };

    let annotated = super::render::lines_annotated(
        &items,
        thought_options(ThinkingVisibility::Expanded, std::slice::from_ref(&fold)),
    );
    let drawn = annotated
        .lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        drawn.contains("first thought"),
        "the other block stays expanded"
    );
    assert!(
        !drawn.contains("second thought"),
        "the folded block draws no text"
    );
}

#[test]
fn aligns_user_messages_to_the_right_and_renders_their_markdown() {
    let lines = content_lines(TranscriptRenderer::lines(
        &[TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "**hello**".to_owned(),
            chips: Vec::new(),
        }],
        DiffMode::Unified,
        80,
    ));

    // The block's gray carries from the first column to the last, like the
    // shell box, and the text hugs the right edge.
    let theme = crate::ui_model::ThemeMode::Dark;
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(crate::components::palette::panel_background(theme)))
    );
    assert_eq!(
        lines[0].spans.last().map(|span| span.content.as_ref()),
        Some("hello")
    );
    assert!(!line_content(&lines[0]).contains("you"));
    assert!(line_content(&lines[0]).ends_with("hello"));
    assert!(lines[0]
        .spans
        .iter()
        .any(|span| span.content == "hello" && span.style.add_modifier.contains(Modifier::BOLD)));
}

#[test]
fn assistant_messages_have_no_padded_surface() {
    let lines = content_lines(TranscriptRenderer::lines(
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
    ));

    assert_eq!(line_content(&lines[0]), "hello");
    assert_eq!(lines[0].style.bg, None);
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
    let lines = content_lines(TranscriptRenderer::lines_with_thinking(
        &items,
        DiffMode::Unified,
        80,
        ThinkingVisibility::Expanded,
        true,
    ));

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
fn a_thought_that_stopped_streaming_freezes_on_the_neutral_background() {
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
    let lines = content_lines(TranscriptRenderer::lines_with_thinking(
        &items,
        DiffMode::Unified,
        80,
        ThinkingVisibility::Expanded,
        false,
    ));

    let neutral = Some(crate::components::palette::thinking_background_neutral(
        crate::ui_model::ThemeMode::Dark,
    ));
    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .all(|span| span.style.bg == neutral)
    );
}

#[test]
fn only_the_last_thought_of_a_turn_moves_while_it_streams() {
    let delta = |turn: u64, delta: Delta| {
        TranscriptItem::ServerEvent(Box::new(ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta { turn, delta },
        }))
    };
    let reasoning = |index: usize, text: &str| {
        delta(
            1,
            Delta::Reasoning {
                index: index.try_into().unwrap_or(u32::MAX),
                text: text.to_owned(),
            },
        )
    };
    let answer = |text: &str| {
        delta(
            1,
            Delta::Text {
                index: 0,
                text: text.to_owned(),
            },
        )
    };
    // The reasoning flushes when a text delta arrives, so this is two blocks.
    let items = [
        reasoning(0, "first"),
        answer("answer"),
        reasoning(1, "second"),
    ];

    // While the thinking streams only the newest block carries the sweep.
    let streaming = content_lines(TranscriptRenderer::lines_with_thinking(
        &items,
        DiffMode::Unified,
        80,
        ThinkingVisibility::Expanded,
        true,
    ));
    let swept = Some(crate::components::palette::thinking_background(
        0,
        0,
        80,
        crate::ui_model::ThemeMode::Dark,
    ));
    let neutral = Some(crate::components::palette::thinking_background_neutral(
        crate::ui_model::ThemeMode::Dark,
    ));
    let has = |lines: &[ratatui::text::Line<'static>],
               background: Option<ratatui::style::Color>| {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.style.bg == background)
    };
    assert!(has(&streaming, swept));
    assert!(has(&streaming, neutral));

    // Once the turn moves on to the answer every block is frozen.
    let frozen = content_lines(TranscriptRenderer::lines_with_thinking(
        &items,
        DiffMode::Unified,
        80,
        ThinkingVisibility::Expanded,
        false,
    ));
    assert!(has(&frozen, neutral));
    assert!(!has(&frozen, swept));
}

#[test]
fn the_chips_of_a_message_are_drawn_under_it() {
    let items = [TranscriptItem::UserMessage {
        queued: false,
        turn_id: None,
        text: "mira esto".to_owned(),
        chips: vec![crate::chips::Chip {
            content: crate::chips::ChipContent::Text("un texto largo".to_owned()),
            label: "un texto largo".to_owned(),
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
    assert!(drawn.contains("[Copied: un texto largo]"));
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

fn line_content(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// The lines that are not the separator that opens a turn.
fn content_lines(lines: Vec<ratatui::text::Line<'static>>) -> Vec<ratatui::text::Line<'static>> {
    lines
        .into_iter()
        .filter(|line| {
            let content = line_content(line);
            !content.starts_with('▾') && !content.starts_with('▸')
        })
        .collect()
}

#[test]
fn tool_calls_hide_raw_arguments_unless_developer_mode_is_on() {
    let arguments = serde_json::json!({"path": "src/lib.rs", "prompt": "secret"});
    let diff_args = |developer_mode| {
        crate::tools::ToolCallComponent::started(
            "read",
            Some(&arguments),
            None,
            DiffMode::Unified,
            80,
            developer_mode,
            crate::ui_model::ThemeMode::Dark,
        )
    };
    let normal = diff_args(false);
    let developer = diff_args(true);

    let normal_text = normal
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");
    let developer_text = developer
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(normal_text.contains("read  src/lib.rs"));
    assert!(!normal_text.contains("secret"));
    assert!(developer_text.contains("raw args"));
    assert!(developer_text.contains("secret"));
}

#[test]
fn a_replayed_plan_call_shows_only_the_plan_name() {
    let items = [TranscriptItem::ServerEvent(Box::new(ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::MessageReplayed {
            index: 1,
            role: MessageRole::Assistant,
            kind: ReplayedKind::ToolCall,
            text: "plan {\n  \"action\": \"propose\",\n  \"content\": \"# Ship the patch\\n\\nDetails\"\n}".to_owned(),
        },
    }))];

    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);

    assert_eq!(line_content(&lines[0]), "☰ plan  Ship the patch");
}

#[test]
fn a_shell_log_shows_only_the_last_five_lines_by_default() {
    let events = (0..9).map(|index| {
        TranscriptItem::ServerEvent(Box::new(ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::ToolOutput {
                turn: 1,
                call_id: "shell-1".to_owned(),
                tool: "shell".to_owned(),
                stream: norte_code_wire::OutputStream::Stdout,
                text: format!("line {index}\n"),
                last: false,
            },
        }))
    });
    let items = vec![TranscriptItem::ServerEvent(Box::new(ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::ToolStarted {
            turn: 1,
            call_id: "shell-1".to_owned(),
            tool: "shell".to_owned(),
            arguments: Some(serde_json::json!({"script": "printf 'line'"})),
            source: None,
        },
    }))]
    .into_iter()
    .chain(events)
    .collect::<Vec<_>>();

    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);
    let drawn = lines
        .iter()
        .map(line_content)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!drawn.contains("line 3"));
    assert!(drawn.contains("line 4"));
    assert!(drawn.contains("line 5"));
    assert!(drawn.contains("line 8"));
    assert!(drawn.contains("… 4 lines"));
}

#[test]
fn the_scroll_counts_wrapped_rows_not_logical_lines() {
    let long = "word ".repeat(200);
    let items = [TranscriptItem::UserMessage {
        queued: false,
        turn_id: None,
        text: long,
        chips: Vec::new(),
    }];

    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 40);
    let wrapped = TranscriptRenderer::wrapped_rows(&items, 40);

    assert_eq!(lines.len(), 2, "the turn's line and the separator");
    assert!(
        wrapped > lines.len(),
        "the paragraph needs {wrapped} rows for one logical line at width 40"
    );
}

#[test]
fn the_scroll_leaves_room_for_the_gap_under_the_last_turn() {
    let items = [TranscriptItem::UserMessage {
        queued: false,
        turn_id: None,
        text: "hola".to_owned(),
        chips: Vec::new(),
    }];

    // The block reserves three rows at the bottom, so a transcript that fits
    // has nothing to scroll and one that overflows starts counting from there.
    let wrapped = TranscriptRenderer::wrapped_rows(&items, 80);
    assert_eq!(wrapped, 2, "the turn's line and the separator");
}

mod tail {
    use super::*;
    use crate::components::views::session::transcript::{TranscriptContext, TranscriptRenderer};
    use crate::ui_model::{
        SessionAttachmentState, ThemeMode, ThinkingVisibility, Toggle, TurnState,
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    fn context_with(
        items: &[TranscriptItem],
        scroll_from_bottom: usize,
        show_tool_output: bool,
    ) -> TranscriptContext<'_> {
        TranscriptContext {
            items,
            session: None,
            turn_state: TurnState::Idle,
            model: None,
            queued_model: None,
            diff_mode: DiffMode::Unified,
            scroll_from_bottom,
            focused: true,
            thinking_visibility: ThinkingVisibility::Collapsed,
            show_tool_output: if show_tool_output {
                Toggle::On
            } else {
                Toggle::Off
            },
            show_timestamps: Toggle::Off,
            theme: ThemeMode::Dark,
            attachment_state: SessionAttachmentState::Attached,
            developer_mode: Toggle::Off,
            shell_output_expanded: Toggle::Off,
            thinking_phase: 0,
            folded: &[],
            thoughts: &[],
        }
    }

    fn buffer(items: &[TranscriptItem], width: u16, height: u16) -> String {
        buffer_with(items, width, height, true)
    }

    fn buffer_with(
        items: &[TranscriptItem],
        width: u16,
        height: u16,
        show_tool_output: bool,
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, height);
                let _ = TranscriptRenderer::render(
                    frame,
                    area,
                    context_with(items, 0, show_tool_output),
                );
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_last_turn_is_visible_when_the_transcript_is_at_the_bottom() {
        let items = (1..=30)
            .map(|turn| TranscriptItem::UserMessage {
                queued: false,
                turn_id: None,
                text: format!("turn {turn}"),
                chips: Vec::new(),
            })
            .collect::<Vec<_>>();

        let drawn = buffer(&items, 24, 12);

        assert!(drawn.contains("turn 30"), "the tail is drawn:\n{drawn}");
        assert!(
            drawn.contains("turn 29"),
            "and the turn before it:\n{drawn}"
        );
    }

    #[test]
    fn a_wrapped_tail_is_visible_at_the_bottom() {
        let items = (1..=12)
            .map(|turn| TranscriptItem::UserMessage {
                queued: false,
                turn_id: None,
                text: format!("turn {turn}"),
                chips: Vec::new(),
            })
            .chain(std::iter::once(TranscriptItem::UserMessage {
                queued: false,
                turn_id: None,
                text: "the last turn ".repeat(12),
                chips: Vec::new(),
            }))
            .collect::<Vec<_>>();

        let drawn = buffer(&items, 30, 10);

        assert!(
            drawn.contains("the last turn"),
            "the wrapped tail is drawn:\n{drawn}"
        );
    }

    #[test]
    fn the_tail_is_visible_at_every_height() {
        let items = (1..=6)
            .map(|turn| TranscriptItem::UserMessage {
                queued: false,
                turn_id: None,
                text: format!("turn {turn}"),
                chips: Vec::new(),
            })
            .collect::<Vec<_>>();

        for height in 2..=14 {
            let drawn = buffer(&items, 24, height);
            assert!(
                drawn.contains("turn 6"),
                "height {height} hides the tail:\n{drawn}"
            );
        }
    }

    #[test]
    fn a_short_transcript_keeps_its_text_and_leaves_the_gap_empty() {
        let items = [TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "solo esto".to_owned(),
            chips: Vec::new(),
        }];

        let drawn = buffer(&items, 40, 8);
        let rows = drawn.lines().collect::<Vec<_>>();

        assert!(drawn.contains("solo esto"), "the turn is drawn:\n{drawn}");
        assert_eq!(rows.len(), 8);
        assert!(rows[1].contains('▾'), "the turn opens with its separator");
        assert!(
            rows[2].contains("solo esto"),
            "right under the separator:\n{drawn}"
        );
        assert!(
            rows[3..].iter().all(|row| row.trim().is_empty()),
            "the rest is blank:\n{drawn}"
        );
    }

    #[test]
    fn a_shell_call_shows_its_stdout_under_a_header_with_its_duration() {
        let items = [
            TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
                request_id: 1,
                session: None,
                kind: ServerEventKind::ToolStarted {
                    turn: 1,
                    call_id: "call-1".to_owned(),
                    tool: "shell".to_owned(),
                    arguments: Some(
                        serde_json::json!({"script": "cat /tmp/a.txt"}),
                    ),
                    source: None,
                },
            })),
            TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
                request_id: 1,
                session: None,
                kind: ServerEventKind::ToolFinished {
                    turn: 1,
                    call_id: "call-1".to_owned(),
                    tool: "shell".to_owned(),
                    is_error: false,
                    error: None,
                    output: Some(
                        "code: Success\nexit_code: Some(0)\nduration_ms: 8\nbytes_read: 12\nstdout:\nline one\nline two"
                            .to_owned(),
                    ),
                    truncated: false,
                    detached: false,
                    duration_ms: 8,
                },
            })),
        ];

        let drawn = buffer_with(&items, 60, 12, false);

        assert!(
            drawn.contains("✓ $ shell  cat /tmp/a.txt  8ms"),
            "the header carries the status, the command and the duration:\n{drawn}"
        );
        assert!(drawn.contains("line one"), "the stdout is drawn:\n{drawn}");
        assert!(
            !drawn.contains("code: Success"),
            "the blob is not:\n{drawn}"
        );
    }

    fn shell_items(chunks: &[&str], finished: bool) -> Vec<TranscriptItem> {
        let mut items = vec![TranscriptItem::ServerEvent(Box::new(
            norte_code_wire::ServerEvent {
                request_id: 1,
                session: None,
                kind: ServerEventKind::ToolStarted {
                    turn: 1,
                    call_id: "call-1".to_owned(),
                    tool: "shell".to_owned(),
                    arguments: Some(
                        serde_json::json!({"script": "for i in 1 2 3; do echo $i; sleep 1; done"}),
                    ),
                    source: None,
                },
            },
        ))];
        for (index, chunk) in chunks.iter().enumerate() {
            items.push(TranscriptItem::ServerEvent(Box::new(
                norte_code_wire::ServerEvent {
                    request_id: 1,
                    session: None,
                    kind: ServerEventKind::ToolOutput {
                        turn: 1,
                        call_id: "call-1".to_owned(),
                        tool: "shell".to_owned(),
                        stream: norte_code_wire::OutputStream::Stdout,
                        text: (*chunk).to_owned(),
                        last: index + 1 == chunks.len(),
                    },
                },
            )));
        }
        if finished {
            items.push(TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
                request_id: 1,
                session: None,
                kind: ServerEventKind::ToolFinished {
                    turn: 1,
                    call_id: "call-1".to_owned(),
                    tool: "shell".to_owned(),
                    is_error: false,
                    error: None,
                    output: Some(
                        "code: Success\nexit_code: Some(0)\nduration_ms: 3\nbytes_read: 6\nstdout:\n1\n2\n3"
                            .to_owned(),
                    ),
                    truncated: false,
                    detached: false,
                    duration_ms: 3000,
                },
            })));
        }
        items
    }

    #[test]
    fn a_running_shell_shows_its_output_as_it_arrives() {
        let first = buffer(&shell_items(&["1\n"], false), 60, 12);
        assert!(first.contains('1'), "the first chunk is drawn:\n{first}");

        let third = buffer(&shell_items(&["1\n", "2\n", "3\n"], false), 60, 12);
        assert!(
            third.contains('1'),
            "the first chunk is still there:\n{third}"
        );
        assert!(third.contains('2'), "the second chunk arrived:\n{third}");
        assert!(third.contains('3'), "the third chunk arrived:\n{third}");
    }

    #[test]
    fn a_finished_shell_shows_its_stdout_once() {
        let drawn = buffer(&shell_items(&["1\n", "2\n"], true), 60, 12);

        assert!(drawn.contains('1'), "the stdout is drawn:\n{drawn}");
        assert!(drawn.contains('2'));
        assert_eq!(
            drawn.matches('2').count(),
            1,
            "the stdout is drawn once:\n{drawn}"
        );
    }

    fn detached_shell_items(chunks: &[&str]) -> Vec<TranscriptItem> {
        let mut items = vec![
            TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
                request_id: 1,
                session: None,
                kind: ServerEventKind::ToolStarted {
                    turn: 1,
                    call_id: "call-1".to_owned(),
                    tool: "shell".to_owned(),
                    arguments: Some(
                        serde_json::json!({"script": "for i in 1 2 3; do echo $i; sleep 1; done", "detach": true}),
                    ),
                    source: None,
                },
            })),
            TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
                request_id: 1,
                session: None,
                kind: ServerEventKind::JobStarted {
                    job_id: "job-1".to_owned(),
                    tool_call_id: "call-1".to_owned(),
                    command: "for i in 1 2 3".to_owned(),
                    started_at: 0,
                    limits: norte_code_wire::JobLimits::default(),
                },
            })),
        ];
        for chunk in chunks {
            items.push(TranscriptItem::ServerEvent(Box::new(
                norte_code_wire::ServerEvent {
                    request_id: 1,
                    session: None,
                    kind: ServerEventKind::JobProgressDelta {
                        job_id: "job-1".to_owned(),
                        stream: norte_code_wire::OutputStream::Stdout,
                        delta: (*chunk).to_owned(),
                        index: 0,
                    },
                },
            )));
        }
        items
    }

    #[test]
    fn a_detached_shell_shows_its_output_as_the_job_writes_it() {
        let first = buffer(&detached_shell_items(&["1\n"]), 60, 12);
        assert!(first.contains("$ shell"), "the call is announced:\n{first}");
        assert!(first.contains('1'), "the first chunk is drawn:\n{first}");

        let third = buffer(&detached_shell_items(&["1\n", "2\n", "3\n"]), 60, 12);
        assert!(third.contains('2'), "the second chunk arrived:\n{third}");
        assert!(third.contains('3'), "the third chunk arrived:\n{third}");
    }

    #[test]
    fn the_gap_between_the_transcript_and_the_input_stays_empty() {
        let items = (1..=30)
            .map(|turn| TranscriptItem::UserMessage {
                queued: false,
                turn_id: None,
                text: format!("turn {turn}"),
                chips: Vec::new(),
            })
            .collect::<Vec<_>>();

        let drawn = buffer(&items, 24, 12);
        let rows = drawn.lines().collect::<Vec<_>>();
        let last_text = rows
            .iter()
            .rposition(|row| !row.trim().is_empty())
            .expect("content");
        let tail: Vec<&str> = rows[last_text + 1..].to_vec();

        assert!(tail.len() >= 3, "three rows are reserved: {tail:?}");
        assert!(
            tail.iter().all(|row| row.trim().is_empty()),
            "the reserved rows are blank: {tail:?}"
        );
    }
}

fn parity_items() -> Vec<TranscriptItem> {
    vec![
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "hola".to_owned(),
            chips: Vec::new(),
        },
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Reasoning {
                    index: 0,
                    text: "thinking".to_owned(),
                },
            },
        })),
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Text {
                    index: 0,
                    text: "buenos dias".to_owned(),
                },
            },
        })),
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::ToolStarted {
                turn: 1,
                call_id: "call-1".to_owned(),
                tool: "shell".to_owned(),
                arguments: Some(serde_json::json!({"script": "ls -la"})),
                source: None,
            },
        })),
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::ToolFinished {
                turn: 1,
                call_id: "call-1".to_owned(),
                tool: "shell".to_owned(),
                is_error: false,
                error: None,
                output: Some(
                    "code: Success\nexit_code: Some(0)\nduration_ms: 3\nbytes_read: 6\nstdout:\nfile.txt"
                        .to_owned(),
                ),
                truncated: false,
                detached: false,
                duration_ms: 3,
            },
        })),
        TranscriptItem::Status("Job started: ls".to_owned()),
        TranscriptItem::Error("algo fallo".to_owned()),
    ]
}

#[test]
fn the_tree_draws_what_the_flat_log_drew() {
    let items = parity_items();
    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 60);
    let drawn = lines
        .iter()
        .map(|line| line_content(line).trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n");

    let expected = format!(
        "▾ turn 1 · 1 tools · 2 messages · 3ms\n{}hola\n\n▸ Thought\n\nbuenos dias\n\n✓ $ shell  ls -la  3ms\n\n file.txt\n\ncode: Success\nexit_code: Some(0)\nduration_ms: 3\nbytes_read: 6\nstdout:\nfile.txt\nJob started: ls\nerror  algo fallo",
        " ".repeat(56)
    );

    assert_eq!(drawn, expected);
}

#[test]
fn a_turn_opens_with_its_separator() {
    let items = [
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "hola".to_owned(),
            chips: Vec::new(),
        },
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Text {
                    index: 0,
                    text: "buenos dias".to_owned(),
                },
            },
        })),
    ];

    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 60);

    assert!(line_content(&lines[0]).starts_with('▾'), "the turn opens");
    assert!(
        line_content(&lines[0]).contains("turn 1"),
        "it names the turn"
    );
    assert!(line_content(&lines[0]).contains("2 messages"));
    assert!(line_content(&lines[1]).ends_with("hola"));
}

#[test]
fn a_folded_turn_shows_only_its_separator() {
    let items = [
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "hola".to_owned(),
            chips: Vec::new(),
        },
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Text {
                    index: 0,
                    text: "buenos dias".to_owned(),
                },
            },
        })),
    ];
    let folded = [super::TurnKey::Live(1)];

    let lines = super::render::lines_annotated(
        &items,
        super::render::TranscriptOptions {
            diff_mode: DiffMode::Unified,
            width: 60,
            thinking_visibility: ThinkingVisibility::Collapsed,
            show_tool_output: crate::ui_model::Toggle::On,
            show_timestamps: crate::ui_model::Toggle::Off,
            theme: crate::ui_model::ThemeMode::Dark,
            developer_mode: crate::ui_model::Toggle::Off,
            shell_output_expanded: crate::ui_model::Toggle::Off,
            thinking_phase: 0,
            thinking_streaming: false,
            folded: &folded,
            thoughts: &[],
        },
    )
    .lines;

    assert_eq!(
        lines.len(),
        2,
        "the fold leaves the separator and a summary"
    );
    assert!(
        line_content(&lines[0]).starts_with('▸'),
        "the separator says it is folded"
    );
    assert!(line_content(&lines[1]).contains("buenos dias"));
}

#[test]
fn the_layout_says_where_each_turn_starts() {
    let items = [
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "hola".to_owned(),
            chips: Vec::new(),
        },
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Text {
                    index: 0,
                    text: "buenos dias".to_owned(),
                },
            },
        })),
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "otro turno".to_owned(),
            chips: Vec::new(),
        },
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 2,
                delta: Delta::Text {
                    index: 0,
                    text: "hola de nuevo".to_owned(),
                },
            },
        })),
    ];

    let layout = super::render::turn_layout(
        &items,
        60,
        super::render::TranscriptOptions {
            diff_mode: DiffMode::Unified,
            width: 60,
            thinking_visibility: ThinkingVisibility::Collapsed,
            show_tool_output: crate::ui_model::Toggle::On,
            show_timestamps: crate::ui_model::Toggle::Off,
            theme: crate::ui_model::ThemeMode::Dark,
            developer_mode: crate::ui_model::Toggle::Off,
            shell_output_expanded: crate::ui_model::Toggle::Off,
            thinking_phase: 0,
            thinking_streaming: false,
            folded: &[],
            thoughts: &[],
        },
    );

    assert_eq!(layout.len(), 2, "the log has two turns");
    assert_eq!(layout[0].key, super::TurnKey::Live(1));
    assert_eq!(layout[1].key, super::TurnKey::Live(2));
    assert_eq!(layout[0].start, 0);
    assert!(
        layout[1].start > layout[0].start,
        "the second turn starts later"
    );
}

fn layout_options(folded: &[super::TurnKey]) -> super::render::TranscriptOptions<'_> {
    super::render::TranscriptOptions {
        diff_mode: DiffMode::Unified,
        width: 60,
        thinking_visibility: ThinkingVisibility::Collapsed,
        show_tool_output: crate::ui_model::Toggle::On,
        show_timestamps: crate::ui_model::Toggle::Off,
        theme: crate::ui_model::ThemeMode::Dark,
        developer_mode: crate::ui_model::Toggle::Off,
        shell_output_expanded: crate::ui_model::Toggle::Off,
        thinking_phase: 0,
        thinking_streaming: false,
        folded,
        thoughts: &[],
    }
}

#[test]
fn a_folded_turn_takes_one_row() {
    let items = [
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "hola".to_owned(),
            chips: Vec::new(),
        },
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 1,
                delta: Delta::Text {
                    index: 0,
                    text: "buenos dias".to_owned(),
                },
            },
        })),
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "otro turno".to_owned(),
            chips: Vec::new(),
        },
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::Delta {
                turn: 2,
                delta: Delta::Text {
                    index: 0,
                    text: "hola de nuevo".to_owned(),
                },
            },
        })),
    ];
    let folded = [super::TurnKey::Live(1)];

    let open = super::render::turn_layout(&items, 60, layout_options(&[]));
    let shut = super::render::turn_layout(&items, 60, layout_options(&folded));

    assert_eq!(open.len(), 2);
    assert_eq!(shut.len(), 2);
    assert!(
        shut[1].start < open[1].start,
        "the folded turn leaves its rows to the turn that follows"
    );
}

#[test]
fn the_separator_shows_how_long_the_tools_ran() {
    let items = [
        TranscriptItem::UserMessage {
            queued: false,
            turn_id: None,
            text: "hola".to_owned(),
            chips: Vec::new(),
        },
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::ToolStarted {
                turn: 1,
                call_id: "call-1".to_owned(),
                tool: "shell".to_owned(),
                arguments: None,
                source: None,
            },
        })),
        TranscriptItem::ServerEvent(Box::new(norte_code_wire::ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::ToolFinished {
                turn: 1,
                call_id: "call-1".to_owned(),
                tool: "shell".to_owned(),
                is_error: false,
                error: None,
                output: None,
                truncated: false,
                detached: false,
                duration_ms: 1234,
            },
        })),
    ];

    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 60);

    assert!(
        line_content(&lines[0]).contains("1.2s"),
        "the separator carries the duration: {}",
        line_content(&lines[0])
    );
}

#[test]
fn after_a_history_reload_the_error_lands_after_the_turn() {
    use crate::ui_model::TranscriptItem;
    use norte_code_wire::HistorySpeaker;

    // The shape `replace_history(history, merged, preserve_live = true)`
    // produces when the reload lands after a turn ended: the replayed page
    // first, then the uncommitted live items in their log order.
    let history = vec![
        TranscriptItem::History(norte_code_wire::HistoryItem {
            id: "u6".to_owned(),
            kind: "user".to_owned(),
            content: "quiero chrome".to_owned(),
            created_at: 1,
            turn_id: Some("6".to_owned()),
            role: HistorySpeaker::User,
            provenance: None,
        }),
        TranscriptItem::History(norte_code_wire::HistoryItem {
            id: "a6".to_owned(),
            kind: "assistant".to_owned(),
            content: "¡Funciona! Chrome se inició.".to_owned(),
            created_at: 2,
            turn_id: Some("6".to_owned()),
            role: HistorySpeaker::Assistant,
            provenance: None,
        }),
    ];
    let tail = vec![TranscriptItem::Error(
        "turn is not active (TURN_NOT_ACTIVE)".to_owned(),
    )];
    let merged: Vec<TranscriptItem> = history.into_iter().chain(tail).collect();

    let lines = TranscriptRenderer::lines(&merged, DiffMode::Unified, 80);
    let rendered: Vec<String> = lines.into_iter().map(|line| line_content(&line)).collect();
    let error = rendered
        .iter()
        .position(|line| line.contains("turn is not active"))
        .expect("error rendered");
    let separator = rendered
        .iter()
        .position(|line| line.starts_with('▾'))
        .expect("separator rendered");
    assert!(
        error > separator,
        "the error renders after the turn: {rendered:?}"
    );
}

#[test]
fn a_queued_user_message_says_it_waits_for_the_turn() {
    use crate::ui_model::TranscriptItem;

    let items = [
        TranscriptItem::UserMessage {
            text: "quiero que no seas headless".to_owned(),
            chips: Vec::new(),
            queued: true,
            turn_id: None,
        },
        TranscriptItem::UserMessage {
            text: "y los logs?".to_owned(),
            chips: Vec::new(),
            queued: false,
            turn_id: None,
        },
    ];

    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 80);
    let rendered: Vec<String> = lines.into_iter().map(|line| line_content(&line)).collect();
    assert!(
        rendered.iter().any(|line| line.contains("· queued")),
        "the pending message says so: {rendered:?}"
    );
}
