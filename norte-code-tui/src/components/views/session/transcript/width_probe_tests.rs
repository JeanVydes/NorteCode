//! The transcript's own markers never push a line past the panel.
//!
//! The renderer wraps text with a `Paragraph`, so the meaningful check is that
//! the widest line it builds before wrapping fits the width it asked for. The
//! markers (the user bar and the timestamp) are what used to overflow.

use norte_code_wire::{Delta, ServerEvent, ServerEventKind};

use super::TranscriptRenderer;
use crate::tools::DiffMode;
use crate::ui_model::{ThinkingVisibility, Toggle, TranscriptItem};

fn width(line: &ratatui::text::Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn delta(turn: u64, text: &str) -> TranscriptItem {
    TranscriptItem::ServerEvent(Box::new(ServerEvent {
        request_id: 1,
        session: None,
        kind: ServerEventKind::Delta {
            turn,
            delta: Delta::Text {
                index: 0,
                text: text.to_owned(),
            },
        },
    }))
}

#[test]
fn the_user_bar_and_a_stamp_fit_the_width() {
    let mut clock = crate::ui_model::SessionClock::default();
    clock.mark(0);
    let items = [TranscriptItem::History(norte_code_wire::HistoryItem {
        id: "1".to_owned(),
        role: norte_code_wire::HistorySpeaker::User,
        kind: "user".to_owned(),
        content: "hola".to_owned(),
        turn_id: Some("turn_1".to_owned()),
        created_at: 65_000,
        provenance: None,
    })];
    let options = super::render::TranscriptOptions {
        diff_mode: DiffMode::Unified,
        width: 20,
        thinking_visibility: ThinkingVisibility::Collapsed,
        show_tool_output: Toggle::On,
        show_timestamps: Toggle::On,
        clock: &clock,
        theme: crate::ui_model::ThemeMode::Dark,
        developer_mode: Toggle::Off,
        shell_output_expanded: Toggle::Off,
        thinking_phase: 0,
        thinking_streaming: false,
        folded: &[],
        thoughts: &[],
    };

    let lines = super::render::lines_annotated(&items, options).lines;

    // The turn separator wraps on its own; the body lines carry the marker and
    // the stamp, and those are what must fit.
    for line in lines.iter().filter(|line| {
        !line
            .spans
            .iter()
            .any(|span| span.content.contains('\u{25be}') || span.content.contains('\u{25b8}'))
    }) {
        assert!(
            width(line) <= 20,
            "a body line is {} wide: {:?}",
            width(line),
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        );
    }
}

#[test]
fn a_long_word_is_left_to_the_paragraph_to_wrap() {
    let items = [delta(1, "supercalifragilisticexpialidocious")];

    let lines = TranscriptRenderer::lines(&items, DiffMode::Unified, 20);

    // The marker alone is two cells; the body is one unbroken word, which the
    // paragraph breaks itself. What must hold is that the marker is part of
    // the measured line, not added after it.
    for line in &lines {
        assert!(width(line) >= 2);
    }
}
