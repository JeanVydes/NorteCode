use std::collections::HashSet;

use norte_code_wire::{
    Delta, HistorySpeaker, MessageRole, ReplayedKind, ServerEventKind, ToolStatus,
};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, Paragraph, Wrap},
};

use crate::{
    components::palette,
    text_builder::TextBuilder,
    tools::{DiffMode, ToolCallComponent},
    ui_model::{SessionAttachmentState, ThemeMode, ThinkingVisibility, TranscriptItem, TurnState},
};

use super::markdown;

pub(super) struct TranscriptRenderer;

#[derive(Clone, Copy)]
struct TranscriptOptions {
    diff_mode: DiffMode,
    width: u16,
    thinking_visibility: ThinkingVisibility,
    show_tool_output: bool,
    show_timestamps: bool,
    theme: ThemeMode,
    thinking_phase: u16,
}

#[derive(Clone, Copy)]
pub(super) struct TranscriptContext<'a> {
    pub(super) items: &'a [TranscriptItem],
    pub(super) session: Option<&'a str>,
    pub(super) turn_state: TurnState,
    pub(super) model: Option<&'a crate::ui_model::SessionModel>,
    pub(super) diff_mode: DiffMode,
    pub(super) scroll_from_bottom: usize,
    pub(super) focused: bool,
    pub(super) thinking_visibility: ThinkingVisibility,
    pub(super) show_tool_output: bool,
    pub(super) show_timestamps: bool,
    pub(super) theme: ThemeMode,
    pub(super) attachment_state: SessionAttachmentState,
    pub(super) thinking_phase: u16,
}

impl TranscriptRenderer {
    pub(super) fn render(
        frame: &mut Frame<'_>,
        area: Rect,
        context: TranscriptContext<'_>,
    ) -> usize {
        let mut lines = Self::lines_with_preferences(
            context.items,
            TranscriptOptions {
                diff_mode: context.diff_mode,
                width: area.width,
                thinking_visibility: context.thinking_visibility,
                show_tool_output: context.show_tool_output,
                show_timestamps: context.show_timestamps,
                theme: context.theme,
                thinking_phase: context.thinking_phase,
            },
        );
        if lines.is_empty() {
            lines.push(Line::from(TextBuilder::transcript_empty()));
        }

        let block = Block::default()
            .title(TextBuilder::transcript_title(
                context.session,
                context.turn_state,
                context.attachment_state,
                context.model,
            ))
            .style(palette::transcript_style(context.theme))
            .title_style(if context.focused {
                Style::default().fg(palette::accent(context.theme))
            } else {
                Style::default()
            });
        let inner_height = block.inner(area).height;
        let max_scroll = lines.len().saturating_sub(usize::from(inner_height));
        let scroll = max_scroll.saturating_sub(context.scroll_from_bottom);
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0))
                .style(palette::transcript_style(context.theme)),
            area,
        );
        max_scroll
    }

    #[cfg(test)]
    fn lines(items: &[TranscriptItem], diff_mode: DiffMode, width: u16) -> Vec<Line<'static>> {
        Self::lines_with_thinking(items, diff_mode, width, ThinkingVisibility::Collapsed)
    }

    #[cfg(test)]
    fn lines_with_thinking(
        items: &[TranscriptItem],
        diff_mode: DiffMode,
        width: u16,
        thinking_visibility: ThinkingVisibility,
    ) -> Vec<Line<'static>> {
        Self::lines_with_preferences(
            items,
            TranscriptOptions {
                diff_mode,
                width,
                thinking_visibility,
                show_tool_output: true,
                show_timestamps: false,
                theme: ThemeMode::Dark,
                thinking_phase: 0,
            },
        )
    }

    fn lines_with_preferences(
        items: &[TranscriptItem],
        options: TranscriptOptions,
    ) -> Vec<Line<'static>> {
        let TranscriptOptions {
            width,
            show_tool_output,
            show_timestamps,
            theme,
            ..
        } = options;
        let mut lines = Vec::new();
        let mut assistant = String::new();
        let mut reasoning = String::new();
        let mut calls_with_output = HashSet::new();
        for item in items {
            match item {
                TranscriptItem::UserMessage { text, chips } => {
                    Self::flush_buffers(&mut lines, &mut assistant, &mut reasoning, options);
                    lines.extend(Self::message_lines(
                        MessageRole::User,
                        text,
                        width,
                        None,
                        theme,
                    ));
                    lines.extend(Self::chip_lines(chips));
                }
                TranscriptItem::History(item) => {
                    if !show_tool_output && item.kind == "tool_result" {
                        continue;
                    }
                    if item.kind == "reasoning" {
                        Self::flush_assistant(&mut lines, &mut assistant, options);
                        reasoning.push_str(&item.content);
                        continue;
                    }
                    Self::flush_buffers(&mut lines, &mut assistant, &mut reasoning, options);
                    let role = match item.role {
                        HistorySpeaker::User => MessageRole::User,
                        HistorySpeaker::Assistant => MessageRole::Assistant,
                        HistorySpeaker::Tool => MessageRole::Tool,
                        HistorySpeaker::SystemEvent => MessageRole::System,
                    };
                    lines.extend(Self::message_lines(
                        role,
                        &item.content,
                        width,
                        show_timestamps.then_some(item.created_at),
                        theme,
                    ));
                }
                TranscriptItem::Error(error) => {
                    Self::flush_buffers(&mut lines, &mut assistant, &mut reasoning, options);
                    lines.push(TextBuilder::transcript_error(error));
                }
                TranscriptItem::Usage(usage) => {
                    Self::flush_buffers(&mut lines, &mut assistant, &mut reasoning, options);
                    if let Some(line) = TextBuilder::usage_footer(usage) {
                        lines.push(line);
                    }
                }
                TranscriptItem::Status(status) => {
                    Self::flush_buffers(&mut lines, &mut assistant, &mut reasoning, options);
                    lines.push(Line::from(Span::styled(
                        status.clone(),
                        Style::default().fg(palette::brand()),
                    )));
                }
                TranscriptItem::ServerEvent(event) => Self::append_server_event(
                    &mut lines,
                    &mut assistant,
                    &mut reasoning,
                    &mut calls_with_output,
                    event,
                    options,
                ),
            }
        }
        Self::flush_buffers(&mut lines, &mut assistant, &mut reasoning, options);
        lines
    }

    fn append_server_event<'a>(
        lines: &mut Vec<Line<'static>>,
        assistant: &mut String,
        reasoning: &mut String,
        calls_with_output: &mut HashSet<&'a str>,
        event: &'a norte_code_wire::ServerEvent,
        options: TranscriptOptions,
    ) {
        let _ = options;
        match &event.kind {
            ServerEventKind::RetryScheduled {
                attempt,
                delay_ms,
                reason,
                ..
            } => {
                Self::flush_buffers(lines, assistant, reasoning, options);
                lines.push(TextBuilder::retry_scheduled(*attempt, *delay_ms, reason));
            }
            ServerEventKind::RetryStarted { attempt, .. } => {
                Self::flush_buffers(lines, assistant, reasoning, options);
                lines.push(TextBuilder::retry_started(*attempt));
            }
            ServerEventKind::ReasoningDowngraded { from, to, reason } => {
                Self::flush_buffers(lines, assistant, reasoning, options);
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
            other => Self::append_turn_event(
                lines,
                assistant,
                reasoning,
                calls_with_output,
                other,
                options,
            ),
        }
    }

    fn append_turn_event<'a>(
        lines: &mut Vec<Line<'static>>,
        assistant: &mut String,
        reasoning: &mut String,
        calls_with_output: &mut HashSet<&'a str>,
        kind: &'a ServerEventKind,
        options: TranscriptOptions,
    ) {
        let TranscriptOptions {
            diff_mode,
            width,
            show_tool_output,
            theme,
            ..
        } = options;
        match kind {
            ServerEventKind::Delta { delta, .. } => match delta {
                Delta::Text { text, .. } | Delta::Refusal { text, .. } => {
                    Self::flush_reasoning(lines, reasoning, options);
                    assistant.push_str(text);
                }
                Delta::Reasoning { text, .. } => {
                    Self::flush_assistant(lines, assistant, options);
                    reasoning.push_str(text);
                }
                Delta::ToolArguments { .. } => {
                    // The start event for this call follows and carries the
                    // tool name and target; the raw JSON arguments add noise,
                    // not information.
                }
                Delta::Media { bytes, last, .. } => {
                    Self::flush_buffers(lines, assistant, reasoning, options);
                    lines.push(TextBuilder::media_delta(*bytes, *last));
                }
            },
            ServerEventKind::ToolStarted {
                tool,
                arguments,
                source,
                ..
            } => {
                Self::flush_buffers(lines, assistant, reasoning, options);
                lines.extend(ToolCallComponent::started(
                    tool,
                    arguments.as_ref(),
                    source.as_deref(),
                    diff_mode,
                    width.saturating_sub(2),
                ));
            }
            ServerEventKind::ToolOutput {
                call_id,
                stream,
                text,
                ..
            } => {
                Self::flush_buffers(lines, assistant, reasoning, options);
                calls_with_output.insert(call_id.as_str());
                if show_tool_output {
                    lines.extend(ToolCallComponent::output(*stream, text));
                }
            }
            ServerEventKind::ToolFinished {
                call_id,
                tool,
                is_error,
                detached,
                error,
                output,
                ..
            } => {
                Self::flush_buffers(lines, assistant, reasoning, options);
                let final_output = (!calls_with_output.contains(call_id.as_str()))
                    .then_some(output.as_deref())
                    .flatten()
                    .filter(|_| show_tool_output);
                lines.extend(ToolCallComponent::finished(
                    tool,
                    ToolStatus::from_outcome(*detached, *is_error),
                    final_output,
                    error.as_deref(),
                ));
            }
            ServerEventKind::MessageReplayed {
                role: _,
                kind: ReplayedKind::Reasoning,
                text,
                ..
            } => {
                Self::flush_assistant(lines, assistant, options);
                reasoning.push_str(text);
            }
            ServerEventKind::MessageReplayed {
                role, kind, text, ..
            } => {
                if *kind == ReplayedKind::ToolResult && !show_tool_output {
                    return;
                }
                Self::flush_buffers(lines, assistant, reasoning, options);
                lines.extend(Self::replayed_lines(*role, *kind, text, width, theme));
            }
            ServerEventKind::Error { code, message } => {
                Self::flush_buffers(lines, assistant, reasoning, options);
                lines.push(TextBuilder::host_error(*code, message));
            }
            _ => {}
        }
    }

    fn flush_assistant(
        lines: &mut Vec<Line<'static>>,
        text: &mut String,
        options: TranscriptOptions,
    ) {
        if !text.is_empty() {
            lines.extend(Self::message_lines(
                MessageRole::Assistant,
                text,
                options.width,
                None,
                options.theme,
            ));
            text.clear();
        }
    }

    fn flush_reasoning(
        lines: &mut Vec<Line<'static>>,
        text: &mut String,
        options: TranscriptOptions,
    ) {
        if text.is_empty() {
            return;
        }
        let expanded = options.thinking_visibility == ThinkingVisibility::Expanded;
        let mut summary = TextBuilder::thinking_summary();
        palette::style_thinking_line(
            &mut summary,
            options.thinking_phase,
            options.width,
            options.theme,
        );
        lines.push(summary);
        if expanded {
            for mut line in markdown::render(text, options.width.saturating_sub(4)) {
                line.spans.insert(0, Span::raw("  "));
                palette::style_thinking_line(
                    &mut line,
                    options.thinking_phase,
                    options.width,
                    options.theme,
                );
                lines.push(line);
            }
        }
        text.clear();
    }

    fn flush_buffers(
        lines: &mut Vec<Line<'static>>,
        assistant: &mut String,
        reasoning: &mut String,
        options: TranscriptOptions,
    ) {
        Self::flush_assistant(lines, assistant, options);
        Self::flush_reasoning(lines, reasoning, options);
    }

    /// The lines the chips of a message are drawn with.
    fn chip_lines(chips: &[crate::chips::Chip]) -> Vec<Line<'static>> {
        chips
            .iter()
            .map(|chip| {
                Line::from(vec![
                    Span::styled(
                        format!("{} ", crate::chips::chip_icon(chip.kind())),
                        Style::default().fg(palette::muted()),
                    ),
                    Span::raw(chip.label.clone()),
                    Span::styled(
                        format!(" · {}", chip.detail),
                        Style::default().fg(palette::muted()),
                    ),
                ])
            })
            .collect()
    }

    fn message_lines(
        role: MessageRole,
        text: &str,
        width: u16,
        timestamp: Option<i64>,
        theme: ThemeMode,
    ) -> Vec<Line<'static>> {
        let role_label_width = if role == MessageRole::Assistant { 8 } else { 0 };
        let mut lines = markdown::render(text, width.saturating_sub(role_label_width + 4));
        if lines.is_empty() {
            lines.push(Line::default());
        }
        let label = if role == MessageRole::User {
            None
        } else {
            Some(TextBuilder::speaker_label(role, "").spans.remove(0))
        };
        let background = match role {
            MessageRole::User => Some(palette::user_message_background(theme)),
            MessageRole::Assistant => Some(palette::assistant_message_background(theme)),
            _ => None,
        };
        for (index, line) in lines.iter_mut().enumerate() {
            if index == 0 {
                if let Some(timestamp) = timestamp {
                    line.spans.insert(
                        0,
                        Span::styled(
                            format!("[{}s] ", timestamp.div_euclid(1_000)),
                            Style::default().fg(palette::muted()),
                        ),
                    );
                }
                if let Some(label) = &label {
                    line.spans.insert(0, label.clone());
                }
            } else if role != MessageRole::User {
                line.spans.insert(0, Span::raw("  "));
            }
            if let Some(background) = background {
                line.spans.insert(0, Span::raw("  "));
                line.spans.push(Span::raw("  "));
                line.style = Style::default().bg(background);
            }
            if role == MessageRole::User {
                *line = std::mem::take(line).alignment(Alignment::Right);
            }
        }
        lines
    }

    fn replayed_lines(
        role: MessageRole,
        kind: ReplayedKind,
        text: &str,
        width: u16,
        theme: ThemeMode,
    ) -> Vec<Line<'static>> {
        if kind == ReplayedKind::ToolCall {
            return vec![TextBuilder::replayed_message(role, kind, text)];
        }
        Self::message_lines(role, text, width, None, theme)
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
