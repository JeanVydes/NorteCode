use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Text},
    widgets::{Block, Paragraph, Wrap},
};

use crate::{
    components::palette,
    text_builder::TextBuilder,
    tools::DiffMode,
    ui_model::{
        SessionAttachmentState, SessionModel, ThemeMode, ThinkingVisibility, Toggle,
        TranscriptItem, TurnState,
    },
};

mod markdown;
mod node;
mod render;
mod tree;

pub(super) use node::TurnKey;
pub(super) use render::turn_layout;
pub(super) use render::{TranscriptOptions, TurnLayout};

/// The blank rows between the last turn and the input.
const GAP_UNDER_LAST_TURN: usize = 3;

pub(super) struct TranscriptRenderer;

/// What the last draw measured: how far the transcript can scroll, how wide
/// it is and where each turn separator sits, so a click lands on a row.
#[derive(Clone, Default)]
pub(super) struct TranscriptLayout {
    pub(super) max_scroll: usize,
    pub(super) width: u16,
    /// Where the wrapped text actually sits.
    pub(super) text_area: Rect,
    /// The absolute rows a turn separator takes, and the turn it opens.
    pub(super) separators: Vec<(usize, TurnKey)>,
}

/// What the transcript is drawn from.
#[derive(Clone, Copy)]
pub(super) struct TranscriptContext<'a> {
    pub(super) items: &'a [TranscriptItem],
    pub(super) session: Option<&'a str>,
    pub(super) turn_state: TurnState,
    pub(super) model: Option<&'a SessionModel>,
    /// A model change the host queued for the next turn boundary.
    pub(super) queued_model: Option<&'a SessionModel>,
    pub(super) diff_mode: DiffMode,
    pub(super) scroll_from_bottom: usize,
    pub(super) focused: bool,
    pub(super) thinking_visibility: ThinkingVisibility,
    pub(super) show_tool_output: Toggle,
    pub(super) show_timestamps: Toggle,
    pub(super) theme: ThemeMode,
    pub(super) attachment_state: SessionAttachmentState,
    pub(super) developer_mode: Toggle,
    pub(super) shell_output_expanded: Toggle,
    /// The phase of the thinking sweep, ticking while the view redraws.
    pub(super) thinking_phase: u16,
    /// The turns the reader folded away.
    pub(super) folded: &'a [TurnKey],
}

impl TranscriptRenderer {
    pub(super) fn render(
        frame: &mut Frame<'_>,
        area: Rect,
        context: TranscriptContext<'_>,
    ) -> TranscriptLayout {
        // The title row opens the block; px-4 wraps the conversation in four
        // cells of air on each side.
        let block = Block::default()
            .title(TextBuilder::transcript_title(
                context.session,
                context.turn_state,
                context.attachment_state,
                context.model,
                context.queued_model,
                context.theme,
            ))
            .style(palette::transcript_style(context.theme))
            .padding(ratatui::widgets::Padding::horizontal(4))
            .title_style(if context.focused {
                Style::default().fg(palette::accent(context.theme))
            } else {
                Style::default()
            });
        let inner = block.inner(area);
        let content_width = inner.width;
        let (mut lines, separator_marks) = render::lines_annotated(
            context.items,
            render::TranscriptOptions {
                diff_mode: context.diff_mode,
                width: content_width,
                thinking_visibility: context.thinking_visibility,
                show_tool_output: context.show_tool_output,
                show_timestamps: context.show_timestamps,
                theme: context.theme,
                developer_mode: context.developer_mode,
                shell_output_expanded: context.shell_output_expanded,
                thinking_phase: context.thinking_phase,
                // The sweep runs while the reasoning streams and freezes
                // the moment the turn moves on to the answer.
                thinking_streaming: matches!(context.turn_state, TurnState::Thinking),
                folded: context.folded,
            },
        );
        if lines.is_empty() {
            lines.push(Line::from(TextBuilder::transcript_empty()));
        }

        let (starts, wrapped) = render::wrapped_starts(&lines, content_width);
        let separators: Vec<(usize, TurnKey)> = separator_marks
            .into_iter()
            .map(|(index, key)| (starts[index], key))
            .collect();
        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });

        // The gap between the last turn and the input is reserved out of the
        // text area, not appended to the content. Reserved, it is three blank
        // rows the reader can never scroll past and no turn can hide behind;
        // appended, it would be three rows of the transcript itself and a
        // short window would show nothing but the gap.
        let reserve = if wrapped > usize::from(inner.height) {
            GAP_UNDER_LAST_TURN.min(inner.height.saturating_sub(1).into())
        } else {
            0
        };
        let text_height = inner
            .height
            .saturating_sub(u16::try_from(reserve).unwrap_or(u16::MAX));
        let max_scroll = wrapped.saturating_sub(usize::from(text_height));
        let scroll = max_scroll.saturating_sub(context.scroll_from_bottom);
        frame.render_widget(block, area);
        frame.render_widget(
            paragraph
                .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0))
                .style(palette::transcript_style(context.theme)),
            Rect {
                height: text_height,
                ..inner
            },
        );
        TranscriptLayout {
            max_scroll,
            width: content_width,
            text_area: Rect {
                height: text_height,
                ..inner
            },
            separators,
        }
    }
}

#[cfg(test)]
#[path = "transcript/tests.rs"]
mod tests;

/// The lines the transcript is drawn with, for the tests that read them.
#[cfg(test)]
impl TranscriptRenderer {
    pub(super) fn lines(
        items: &[TranscriptItem],
        diff_mode: DiffMode,
        width: u16,
    ) -> Vec<Line<'static>> {
        Self::lines_with_thinking(
            items,
            diff_mode,
            width,
            ThinkingVisibility::Collapsed,
            false,
        )
    }

    /// The rows a paragraph needs once it wraps, which is what the scroll
    /// offset counts.
    pub(super) fn wrapped_rows(items: &[TranscriptItem], width: u16) -> usize {
        let mut lines = Self::lines_with_thinking(
            items,
            DiffMode::Unified,
            width,
            ThinkingVisibility::Collapsed,
            false,
        );
        if lines.is_empty() {
            lines.push(Line::from(TextBuilder::transcript_empty()));
        }
        ratatui::widgets::Paragraph::new(ratatui::text::Text::from(lines))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .line_count(width)
    }

    pub(super) fn lines_with_thinking(
        items: &[TranscriptItem],
        diff_mode: DiffMode,
        width: u16,
        thinking_visibility: ThinkingVisibility,
        thinking_streaming: bool,
    ) -> Vec<Line<'static>> {
        render::lines_annotated(
            items,
            render::TranscriptOptions {
                diff_mode,
                width,
                thinking_visibility,
                show_tool_output: Toggle::On,
                show_timestamps: Toggle::Off,
                theme: ThemeMode::Dark,
                developer_mode: Toggle::Off,
                shell_output_expanded: Toggle::Off,
                thinking_phase: 0,
                thinking_streaming,
                folded: &[],
            },
        )
        .0
    }
}
