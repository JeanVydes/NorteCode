//! The tool call: its header and everything it produced.

use norte_code_wire::ToolStatus;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::super::node::ToolNode;
use super::{TranscriptOptions, separate};
use crate::{
    components::palette,
    text_builder::TextBuilder,
    tools::{ToolCallComponent, ToolFinish},
};

/// Draws one tool call: what it was, what it printed and how it ended.
pub(crate) fn draw_tool(
    lines: &mut Vec<Line<'static>>,
    tool: &ToolNode,
    options: TranscriptOptions<'_>,
) {
    let TranscriptOptions {
        diff_mode,
        width,
        theme,
        developer_mode,
        show_tool_output,
        shell_output_expanded,
        ..
    } = options;
    separate(lines);
    if tool.replayed {
        if tool.name == "shell" {
            draw_replayed_shell(lines, tool, width, theme, show_tool_output.is_on());
            return;
        }
        lines.extend(ToolCallComponent::started(
            &tool.name,
            tool.arguments.as_ref(),
            tool.source.as_deref(),
            diff_mode,
            width.saturating_sub(2),
            false,
            theme,
        ));
        if let Some(output) = tool.output.as_deref() {
            lines.extend(ToolCallComponent::replayed_output(
                output,
                width,
                theme,
                show_tool_output.is_on(),
            ));
        }
        return;
    }
    if tool.name == "shell" {
        lines.push(shell_header(tool));
        if developer_mode.is_on()
            && let Some(arguments) = tool.arguments.as_ref()
        {
            lines.extend(ToolCallComponent::raw_arguments(
                arguments, width, &tool.name, theme,
            ));
        }
        match tool.status {
            None => lines.extend(ToolCallComponent::shell_output_tail(
                &tool.chunks,
                shell_output_expanded.is_on(),
                theme,
                width,
            )),
            Some(ToolStatus::Cancelled) => {
                lines.extend(ToolCallComponent::shell_output_tail(
                    &tool.chunks,
                    shell_output_expanded.is_on(),
                    theme,
                    width,
                ));
                lines.push(TextBuilder::tool_detached_output());
            }
            Some(_) => lines.extend(ToolCallComponent::finished(ToolFinish {
                name: &tool.name,
                status: tool.status.unwrap_or(ToolStatus::Ok),
                output: tool.output.as_deref(),
                error: tool.error.as_deref(),
                shell: true,
                theme,
                width,
                show_tool_output: show_tool_output.is_on(),
            })),
        }
        return;
    }
    lines.extend(ToolCallComponent::started(
        &tool.name,
        tool.arguments.as_ref(),
        tool.source.as_deref(),
        diff_mode,
        width.saturating_sub(2),
        developer_mode.is_on(),
        theme,
    ));
    if show_tool_output.is_on() && !tool.chunks.is_empty() {
        lines.extend(ToolCallComponent::output_chunks(&tool.chunks, width, theme));
    }
    lines.extend(ToolCallComponent::finished(ToolFinish {
        name: &tool.name,
        status: tool.status.unwrap_or(ToolStatus::Ok),
        output: tool.output.as_deref(),
        error: tool.error.as_deref(),
        shell: false,
        theme,
        width,
        show_tool_output: show_tool_output.is_on(),
    }));
}

/// The command line of a shell call: its outcome, the tool and the script.
fn shell_command_line(
    icon: &'static str,
    color: ratatui::style::Color,
    arguments: Option<&serde_json::Value>,
    duration_ms: Option<u64>,
) -> Line<'static> {
    let command = arguments
        .and_then(|arguments| {
            arguments
                .get("script")
                .or_else(|| arguments.get("command"))
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or_default();
    let duration = duration_ms.map_or_else(String::new, |duration| format!("  {duration}ms"));
    Line::from(vec![
        Span::styled(
            format!("{icon} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "$ shell  ".to_owned(),
            Style::default()
                .fg(palette::brand())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(command.to_owned(), Style::default().fg(palette::muted())),
        Span::styled(duration, Style::default().fg(palette::muted())),
    ])
}

/// A replayed shell: the same shape the live draw uses, a header with
/// the outcome and the box around the output. Anything else would be a
/// dangling glyph.
fn draw_replayed_shell(
    lines: &mut Vec<Line<'static>>,
    tool: &ToolNode,
    width: u16,
    theme: crate::ui_model::ThemeMode,
    show_tool_output: bool,
) {
    lines.push(replayed_shell_header(tool));
    if let Some(output) = tool.output.as_deref() {
        lines.extend(ToolCallComponent::replayed_output(
            output,
            width,
            theme,
            show_tool_output,
        ));
    }
}

/// The header of a shell call as it happened: outcome, script and how long
/// it ran.
fn shell_header(tool: &ToolNode) -> Line<'static> {
    let (icon, color) = match tool.status {
        Some(ToolStatus::Ok) => ("✓", palette::success()),
        Some(ToolStatus::Error) => ("✗", palette::danger()),
        Some(ToolStatus::Cancelled) => ("⊘", palette::warning()),
        None => ("…", palette::muted()),
    };
    shell_command_line(icon, color, tool.arguments.as_ref(), tool.duration_ms)
}

/// The header of a replayed shell call: the outcome comes from the result
/// the host recorded, the duration with it; otherwise the same head the
/// live path draws.
fn replayed_shell_header(tool: &ToolNode) -> Line<'static> {
    if let Some(result) = tool
        .output
        .as_deref()
        .and_then(ToolCallComponent::parse_shell_result)
    {
        let icon = ToolCallComponent::shell_status_icon(&result.code, false);
        let color = match icon {
            "✓" => palette::success(),
            "⊘" | "⏱" => palette::warning(),
            _ => palette::danger(),
        };
        return shell_command_line(icon, color, tool.arguments.as_ref(), result.duration_ms);
    }
    shell_header(tool)
}
