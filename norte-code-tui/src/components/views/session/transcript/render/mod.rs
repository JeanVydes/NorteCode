//! Draws the transcript tree.
//!
//! The tree says what belongs together; this says how each node looks. The
//! spacing rules live here, so the reader sees one blank line between
//! messages and none between the parts of one tool call.

use norte_code_wire::MessageRole;
use ratatui::{
    style::Style,
    text::{Line, Span},
};

mod blocks;
mod tool;

use super::node::{TranscriptNode, TranscriptNodeKind, TurnKey};
use super::tree::TranscriptTree;
use crate::{
    components::palette,
    text_builder::TextBuilder,
    tools::DiffMode,
    ui_model::{ThemeMode, ThinkingVisibility, Toggle, TranscriptItem},
};

pub(crate) use blocks::{chip_lines, message_lines, reasoning_lines};
pub(crate) use tool::draw_tool;

/// The options the transcript is drawn with.
#[derive(Clone, Copy)]
pub(crate) struct TranscriptOptions<'a> {
    pub(crate) diff_mode: DiffMode,
    pub(crate) width: u16,
    pub(crate) thinking_visibility: ThinkingVisibility,
    pub(crate) show_tool_output: Toggle,
    pub(crate) show_timestamps: Toggle,
    pub(crate) theme: ThemeMode,
    pub(crate) developer_mode: Toggle,
    pub(crate) shell_output_expanded: Toggle,
    /// The phase of the thinking sweep, ticking while thinking streams.
    pub(crate) thinking_phase: u16,
    /// Whether thinking is streaming right now: the last block animates.
    pub(crate) thinking_streaming: bool,
    /// The turns the reader folded away.
    pub(crate) folded: &'a [TurnKey],
}

/// How many reasoning blocks the tree holds, and how many were drawn.
///
/// The block that streams last is the one that animates; measurement paths
/// pass the zero value, which animates nothing.
#[derive(Clone, Copy, Default)]
struct ReasoningProgress {
    total: usize,
    seen: usize,
}

/// How many reasoning blocks a subtree holds.
fn reasoning_total(node: &TranscriptNode) -> usize {
    let mut total = usize::from(matches!(node.kind(), TranscriptNodeKind::Reasoning { .. }));
    for child in node.children() {
        total += reasoning_total(child);
    }
    total
}

/// The lines the tree is drawn with, and where the turn separators are.
///
/// The click view maps a row to the turn it opens, so the caller needs the
/// logical indexes of the separator lines.
pub(super) fn lines_annotated(
    items: &[TranscriptItem],
    options: TranscriptOptions<'_>,
) -> (Vec<Line<'static>>, Vec<(usize, TurnKey)>) {
    let tree = TranscriptTree::build(items);
    let mut progress = ReasoningProgress {
        total: tree.iter().map(reasoning_total).sum(),
        seen: 0,
    };
    let mut lines = Vec::new();
    let mut separators = Vec::new();
    for node in &tree {
        draw_node(
            &mut lines,
            node,
            options,
            Some(&mut separators),
            &mut progress,
        );
    }
    (lines, separators)
}

/// The rows each line takes once it wraps, and the row every line starts at.
pub(super) fn wrapped_starts(lines: &[Line<'static>], width: u16) -> (Vec<usize>, usize) {
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut total = 0_usize;
    for line in lines {
        starts.push(total);
        total += ratatui::widgets::Paragraph::new(ratatui::text::Text::from(line.clone()))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .line_count(width);
    }
    starts.push(total);
    (starts, total)
}

/// A turn and the row it starts at.
#[derive(Clone, Debug)]
pub(crate) struct TurnLayout {
    pub(crate) key: TurnKey,
    pub(crate) start: usize,
}

/// The row each turn starts at, in the order the turns appear.
///
/// The rows are measured the way the paragraph measures them, so a turn that
/// wraps takes the rows it really takes.
pub(crate) fn turn_layout(
    items: &[TranscriptItem],
    width: u16,
    options: TranscriptOptions<'_>,
) -> Vec<TurnLayout> {
    let tree = TranscriptTree::build(items);
    let mut progress = ReasoningProgress::default();
    let mut layout = Vec::new();
    let mut start = 0;
    for node in &tree {
        let lines = lines_of_with_progress(std::slice::from_ref(node), options, &mut progress);
        let rows = ratatui::widgets::Paragraph::new(ratatui::text::Text::from(lines))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .line_count(width);
        if let Some(key) = node.turn_key() {
            layout.push(TurnLayout {
                key: key.clone(),
                start,
            });
        }
        start += rows;
    }
    layout
}

/// The lines a group of nodes is drawn with, for a caller that already
/// counts the reasoning blocks it draws.
fn lines_of_with_progress(
    nodes: &[TranscriptNode],
    options: TranscriptOptions<'_>,
    progress: &mut ReasoningProgress,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut separators = Vec::new();
    for node in nodes {
        draw_node(&mut lines, node, options, Some(&mut separators), progress);
    }
    lines
}

/// Draws one node and the nodes under it.
///
/// `progress` counts the reasoning blocks the caller expects and the ones
/// drawn so far, so the draw knows which block streams last.
fn draw_node(
    lines: &mut Vec<Line<'static>>,
    node: &TranscriptNode,
    options: TranscriptOptions<'_>,
    mut separators: Option<&mut Vec<(usize, TurnKey)>>,
    progress: &mut ReasoningProgress,
) {
    match node.kind() {
        TranscriptNodeKind::Turn { key } => {
            let folded = options.folded.contains(key);
            separate(lines);
            if let Some(marks) = &mut separators {
                marks.push((lines.len(), key.clone()));
            }
            lines.push(turn_separator(key, node, folded));
            if folded {
                if let Some(summary) = folded_summary(node, options.width) {
                    lines.push(summary);
                }
                return;
            }
            for child in node.children() {
                draw_node(lines, child, options, separators.as_deref_mut(), progress);
            }
        }
        TranscriptNodeKind::Message {
            role,
            text,
            chips,
            timestamp,
            queued,
        } => {
            separate(lines);
            lines.extend(message_lines(
                *role,
                text,
                options.width,
                options
                    .show_timestamps
                    .is_on()
                    .then_some(*timestamp)
                    .flatten(),
                options.theme,
            ));
            lines.extend(chip_lines(chips));
            if *queued {
                lines.push(Line::from(Span::styled(
                    "· queued, waiting for the turn to finish",
                    Style::default().fg(palette::muted()),
                )));
            }
        }
        TranscriptNodeKind::Reasoning { text } => {
            separate(lines);
            let ordinal = progress.seen;
            progress.seen += 1;
            let phase = (options.thinking_streaming && ordinal + 1 == progress.total)
                .then_some(options.thinking_phase);
            lines.extend(reasoning_lines(text, options, phase));
        }
        TranscriptNodeKind::Tool(tool) => draw_tool(lines, tool, options),
        TranscriptNodeKind::Usage { usage } => {
            if let Some(line) = TextBuilder::usage_footer(usage) {
                lines.push(line);
            }
        }
        TranscriptNodeKind::Error { code, message } => {
            lines.push(match code {
                Some(code) => TextBuilder::host_error(*code, message),
                None => TextBuilder::transcript_error(message),
            });
        }
        TranscriptNodeKind::Notice { text } => {
            lines.push(Line::from(Span::styled(
                text.clone(),
                Style::default().fg(palette::brand()),
            )));
        }
        TranscriptNodeKind::Retry {
            attempt,
            delay_ms,
            reason,
        } => {
            lines.push(match delay_ms {
                Some(delay_ms) => TextBuilder::retry_scheduled(*attempt, *delay_ms, reason),
                None => TextBuilder::retry_started(*attempt),
            });
        }
        TranscriptNodeKind::Downgrade { from, to, reason } => {
            let detail = if reason.trim().is_empty() {
                format!("Reasoning changed from {from} to {to}.")
            } else {
                format!("Reasoning changed from {from} to {to}: {reason}")
            };
            lines.push(Line::from(Span::styled(
                detail,
                Style::default().fg(palette::warning()),
            )));
        }
        TranscriptNodeKind::Media { bytes, last } => {
            lines.push(TextBuilder::media_delta(*bytes, *last));
        }
    }
}

/// The line that opens a turn.
fn turn_separator(key: &TurnKey, turn: &TranscriptNode, folded: bool) -> Line<'static> {
    let mut tools = 0_usize;
    let mut messages = 0_usize;
    let mut duration_ms = 0_u64;
    for child in turn.children() {
        match child.kind() {
            TranscriptNodeKind::Tool(tool) => {
                tools += 1;
                if let Some(duration) = tool.duration_ms {
                    duration_ms += duration;
                }
            }
            TranscriptNodeKind::Message { .. } => messages += 1,
            _ => {}
        }
    }
    let duration = (duration_ms > 0).then_some(duration_ms);
    let label = match key {
        TurnKey::Live(turn) => format!("turn {turn}"),
        TurnKey::Named(_) => "turn".to_owned(),
    };
    TextBuilder::turn_separator(&label, tools, messages, duration, folded)
}

/// The line that closes a folded turn: what its answer says, when any.
fn folded_summary(turn: &TranscriptNode, width: u16) -> Option<Line<'static>> {
    let text = turn
        .children()
        .iter()
        .find_map(|child| match child.kind() {
            TranscriptNodeKind::Message { role, text, .. } if *role == MessageRole::Assistant => {
                text.lines()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_owned)
            }
            _ => None,
        })?;
    let max = usize::from(width).saturating_sub(4).max(16);
    let mut summary = text.chars().take(max).collect::<String>();
    if text.chars().count() > max {
        summary.push_str("...");
    }
    Some(Line::from(Span::styled(
        format!("  {summary}"),
        Style::default().fg(palette::muted()),
    )))
}

/// The blank row that keeps one block from running into the next.
///
/// The line that opens a turn already separates what came before it, so the
/// first block of a turn does not add another one.
pub(super) fn separate(lines: &mut Vec<Line<'static>>) {
    let Some(last) = lines.last() else {
        return;
    };
    if last.spans.is_empty() || opens_a_turn(last) {
        return;
    }
    lines.push(Line::default());
}

/// Whether a line opens a turn.
fn opens_a_turn(line: &Line<'_>) -> bool {
    line.spans
        .first()
        .is_some_and(|span| span.content.starts_with('▾') || span.content.starts_with('▸'))
}
