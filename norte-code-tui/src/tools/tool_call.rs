use norte_code_wire::{OutputStream, ToolStatus};

/// A shell's structured result, parsed from its text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShellResult {
    pub(crate) code: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) bytes_read: Option<u64>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use unicode_width::UnicodeWidthChar;

use crate::{components::palette, text_builder::TextBuilder, tools::EditDiffComponent};

pub(crate) struct ToolCallComponent;

const SHELL_OUTPUT_TAIL: usize = 5;

/// How much of an agent's message a header shows.
const MESSAGE_PREVIEW: usize = 96;
const RAW_JSON_TAIL: usize = 18;

#[derive(Clone, Copy)]
pub(crate) struct ToolFinish<'a> {
    pub(crate) name: &'a str,
    pub(crate) status: ToolStatus,
    pub(crate) output: Option<&'a str>,
    pub(crate) error: Option<&'a str>,
    pub(crate) shell: bool,
    pub(crate) theme: crate::ui_model::ThemeMode,
    pub(crate) width: u16,
    pub(crate) show_tool_output: bool,
}

impl ToolCallComponent {
    pub(crate) fn started(
        name: &str,
        arguments: Option<&Value>,
        source: Option<&str>,
        diff_mode: super::DiffMode,
        width: u16,
        developer_mode: bool,
        theme: crate::ui_model::ThemeMode,
    ) -> Vec<Line<'static>> {
        let target = arguments.and_then(|arguments| Self::target(name, arguments));
        let mut lines = vec![TextBuilder::tool_call_header(
            Self::icon(name),
            name,
            target.as_deref(),
        )];
        if name == "edit" {
            lines.extend(EditDiffComponent::render(
                source, arguments, diff_mode, width, theme,
            ));
        }
        if developer_mode && let Some(arguments) = arguments {
            lines.extend(Self::raw_json_lines(arguments, width, name, theme));
        }
        lines
    }

    pub(crate) fn finished(finished: ToolFinish<'_>) -> Vec<Line<'static>> {
        if finished.shell {
            return Self::finished_shell(finished);
        }
        let mut lines = Vec::new();
        if let Some(text) = finished.error {
            lines.push(Self::banner("error", finished.width, finished.theme));
            lines.extend(text.lines().map(|line| {
                Self::terminal_line(OutputStream::Stderr, line, finished.width, finished.theme)
            }));
            return lines;
        }
        if finished.name != "edit" {
            lines.push(TextBuilder::tool_finished(finished.name, finished.status));
        }
        if finished.show_tool_output
            && let Some(text) = finished.output
        {
            lines.extend(Self::raw_lines(text));
        }
        lines
    }

    /// What a finished shell call draws: its stdout, always, and the whole
    /// blob when the reader asked for the tool output.
    fn finished_shell(finished: ToolFinish<'_>) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if finished.error.is_none() && finished.status == ToolStatus::Cancelled {
            lines.push(TextBuilder::tool_detached_output());
            return lines;
        }
        if let Some(text) = finished.error {
            let rows = text.lines().map(|line| {
                Self::terminal_line(OutputStream::Stderr, line, finished.width, finished.theme)
            });
            return Self::shell_box(rows.collect(), finished.width, finished.theme);
        }
        let Some(text) = finished.output else {
            return lines;
        };
        if let Some(result) = Self::parse_shell_result(text) {
            let mut rows: Vec<Line<'static>> = result
                .stdout
                .lines()
                .map(|line| {
                    Self::terminal_line(OutputStream::Stdout, line, finished.width, finished.theme)
                })
                .collect();
            if !result.stderr.is_empty() {
                rows.extend(result.stderr.lines().map(|line| {
                    Self::terminal_line(OutputStream::Stderr, line, finished.width, finished.theme)
                }));
            }
            lines.extend(Self::shell_box(rows, finished.width, finished.theme));
        } else {
            let rows = text.lines().map(|line| {
                Self::terminal_line(OutputStream::Stdout, line, finished.width, finished.theme)
            });
            lines.extend(Self::shell_box(
                rows.collect(),
                finished.width,
                finished.theme,
            ));
        }
        if finished.show_tool_output {
            lines.extend(Self::raw_lines(text));
        }
        lines
    }

    /// The text of a tool result, as the tool wrote it.
    fn raw_lines(text: &str) -> Vec<Line<'static>> {
        text.lines()
            .map(|line| Line::from(Span::raw(line.to_owned())))
            .collect()
    }

    pub(crate) fn shell_output_tail(
        records: &[(OutputStream, String)],
        expanded: bool,
        theme: crate::ui_model::ThemeMode,
        width: u16,
    ) -> Vec<Line<'static>> {
        let lines = records
            .iter()
            .flat_map(|(stream, text)| text.lines().map(|line| (*stream, line)))
            .collect::<Vec<_>>();
        let omitted = lines.len().saturating_sub(SHELL_OUTPUT_TAIL);
        let selected: &[(OutputStream, &str)] = if expanded {
            &lines
        } else {
            &lines[lines.len().saturating_sub(SHELL_OUTPUT_TAIL)..]
        };
        let mut rows = Vec::new();
        if omitted > 0 && !expanded {
            rows.push(Self::frame_body(
                "omitted",
                format!("… {omitted} lines"),
                width,
                theme,
            ));
        }
        rows.extend(
            selected
                .iter()
                .map(|(stream, line)| Self::terminal_line(*stream, line, width, theme)),
        );
        Self::shell_box(rows, width, theme)
    }

    /// The lines a replayed tool result is drawn with: the same boxed
    /// block the live path draws, instead of a wall of assistant text.
    pub(crate) fn replayed_output(
        text: &str,
        width: u16,
        theme: crate::ui_model::ThemeMode,
        show_tool_output: bool,
    ) -> Vec<Line<'static>> {
        const REPLAYED_OUTPUT_TAIL: usize = 25;
        let mut output = Vec::new();
        if let Some(result) = ToolCallComponent::parse_shell_result(text) {
            // The box around the output, with inset and air, says the rest;
            // nothing orphaned on its own line.
            let mut rows: Vec<Line<'static>> = result
                .stdout
                .lines()
                .map(|line| Self::terminal_line(OutputStream::Stdout, line, width, theme))
                .collect();
            if !result.stderr.is_empty() {
                rows.extend(
                    result
                        .stderr
                        .lines()
                        .map(|line| Self::terminal_line(OutputStream::Stderr, line, width, theme)),
                );
            }
            output.extend(Self::shell_box(rows, width, theme));
            if show_tool_output {
                output.extend(Self::raw_lines(text));
            }
            return output;
        }
        // Not a shell result: the tool output is all there is.
        if !show_tool_output {
            return output;
        }
        let lines = text.lines().collect::<Vec<_>>();
        let omitted = lines.len().saturating_sub(REPLAYED_OUTPUT_TAIL);
        if omitted > 0 {
            output.push(Self::frame_body(
                "omitted",
                format!("… {omitted} lines"),
                width,
                theme,
            ));
        }
        output.extend(
            lines
                .iter()
                .skip(omitted)
                .map(|line| Self::terminal_line(OutputStream::Other, line, width, theme)),
        );
        output
    }

    /// The raw arguments of a call, as the box the developer mode draws.
    pub(crate) fn raw_arguments(
        arguments: &Value,
        width: u16,
        tool: &str,
        theme: crate::ui_model::ThemeMode,
    ) -> Vec<Line<'static>> {
        Self::raw_json_lines(arguments, width, tool, theme)
    }

    /// The output a tool streamed, one line per line it wrote.
    pub(crate) fn output_chunks(
        chunks: &[(OutputStream, String)],
        _width: u16,
        _theme: crate::ui_model::ThemeMode,
    ) -> Vec<Line<'static>> {
        chunks
            .iter()
            .map(|(stream, text)| {
                let style = match stream {
                    OutputStream::Stderr => Style::default().fg(palette::danger()),
                    OutputStream::Stdout | OutputStream::Other => {
                        Style::default().fg(palette::text_muted(crate::ui_model::ThemeMode::Dark))
                    }
                };
                let label = TextBuilder::tool_output_label(*stream);
                Line::from(vec![
                    Span::styled(format!("{label} "), style),
                    Span::styled(text.to_owned(), style),
                ])
            })
            .collect()
    }

    fn raw_json_lines(
        arguments: &Value,
        width: u16,
        tool: &str,
        theme: crate::ui_model::ThemeMode,
    ) -> Vec<Line<'static>> {
        let title = format!("raw args · {tool}");
        let body = serde_json::to_string_pretty(arguments).unwrap_or_else(|_| "{ }".to_owned());
        let mut lines = vec![Self::banner(&title, width, theme)];
        let mut entries = body.lines().collect::<Vec<_>>();
        let omitted = entries.len().saturating_sub(RAW_JSON_TAIL);
        if omitted > 0 {
            entries.truncate(RAW_JSON_TAIL);
        }
        for raw in entries {
            lines.push(Self::json_body(raw, width, theme));
        }
        if omitted > 0 {
            lines.push(Self::frame_body(
                "omitted",
                format!("… {omitted} lines"),
                width,
                theme,
            ));
        }
        lines
    }

    fn json_body(line: &str, width: u16, theme: crate::ui_model::ThemeMode) -> Line<'static> {
        let trimmed = line.trim_start();
        let indent = &line[..line.len().saturating_sub(trimmed.len())];
        let spans = if let Some(key_end) = trimmed.find("\":") {
            let key_end = key_end + 1;
            let value = trimmed[key_end..].trim_start();
            let mut spans = vec![
                Span::raw(indent.to_owned()),
                Span::styled(
                    trimmed[..key_end].to_owned(),
                    Style::default().fg(palette::accent(theme)),
                ),
            ];
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                value.to_owned(),
                Style::default().fg(Self::json_scalar_style(value)),
            ));
            spans
        } else {
            vec![
                Span::raw(indent.to_owned()),
                Span::styled(
                    trimmed.to_owned(),
                    Style::default().fg(Self::json_scalar_style(trimmed)),
                ),
            ]
        };
        Self::padded_line(spans, width, theme)
    }

    fn json_scalar_style(token: &str) -> ratatui::style::Color {
        let token = token.trim().trim_end_matches(',');
        if token.starts_with('"')
            || token.starts_with('{')
            || token.starts_with('[')
            || matches!(token, "true" | "false")
        {
            palette::info()
        } else if token == "null" {
            palette::warning()
        } else {
            palette::success()
        }
    }

    /// The banner line of a block, the way the footer paints its gray.
    fn banner(title: &str, width: u16, theme: crate::ui_model::ThemeMode) -> Line<'static> {
        Self::padded_line(
            vec![Span::styled(
                title.to_owned(),
                Style::default()
                    .fg(palette::accent(crate::ui_model::ThemeMode::Dark))
                    .bg(palette::surface(theme))
                    .add_modifier(ratatui::style::Modifier::BOLD),
            )],
            width,
            theme,
        )
    }

    fn frame_body(
        label: &str,
        content: String,
        width: u16,
        theme: crate::ui_model::ThemeMode,
    ) -> Line<'static> {
        let background = palette::surface(theme);
        let used = 1 + Self::display_width(label) + 1 + Self::display_width(&content) + 1;
        let padding = usize::from(width).saturating_sub(used);
        Line::from(vec![
            Span::styled(" ", Style::default().bg(background)),
            Span::styled(
                format!("{label} "),
                Style::default()
                    .fg(palette::accent(crate::ui_model::ThemeMode::Dark))
                    .bg(background)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::styled(
                content,
                Style::default().fg(palette::text(theme)).bg(background),
            ),
            Span::styled(
                " ".repeat(padding.saturating_add(1)),
                Style::default().bg(background),
            ),
        ])
        .style(Style::default().bg(background))
    }

    /// A line whose spans carry the block's gray to the last column.
    fn padded_line(
        spans: Vec<Span<'static>>,
        width: u16,
        theme: crate::ui_model::ThemeMode,
    ) -> Line<'static> {
        let background = palette::surface(theme);
        let used = spans
            .iter()
            .map(|span| Self::display_width(span.content.as_ref()))
            .sum::<usize>();
        let mut rendered = spans;
        rendered.push(Span::styled(
            " ".repeat(usize::from(width).saturating_sub(used)),
            Style::default().bg(background),
        ));
        Line::from(rendered).style(Style::default().bg(background))
    }

    /// The structured result a shell returns, parsed into its parts.
    ///
    /// The shell tool renders one blob with `code:`, `exit_code:`,
    /// `duration_ms:`, `bytes_read:` and `stdout:`/`stderr:` sections; the
    /// header carries the status and the duration, so the body only needs
    /// the two streams.
    pub(crate) fn parse_shell_result(text: &str) -> Option<ShellResult> {
        let mut code = None;
        let mut exit_code = None;
        let mut duration_ms = None;
        let mut bytes_read = None;
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut section: Option<&mut String> = None;
        for line in text.lines() {
            if line == "stdout:" {
                section = Some(&mut stdout);
                continue;
            }
            if line == "stderr:" {
                section = Some(&mut stderr);
                continue;
            }
            if let Some(target) = section.as_mut() {
                if !target.is_empty() {
                    target.push('\n');
                }
                target.push_str(line);
                continue;
            }
            if let Some(rest) = line.strip_prefix("code: ") {
                code = Some(rest.to_owned());
            } else if let Some(rest) = line.strip_prefix("exit_code: ") {
                exit_code = Self::parse_exit_code(rest);
            } else if let Some(rest) = line.strip_prefix("duration_ms: ") {
                duration_ms = rest.trim().parse().ok();
            } else if let Some(rest) = line.strip_prefix("bytes_read: ") {
                bytes_read = rest.trim().parse().ok();
            }
        }
        let code = code?;
        Some(ShellResult {
            code,
            exit_code,
            duration_ms,
            bytes_read,
            stdout,
            stderr,
        })
    }

    fn parse_exit_code(text: &str) -> Option<i32> {
        let text = text.trim();
        if text == "None" {
            return None;
        }
        let inner = text.strip_prefix("Some(")?.strip_suffix(")")?;
        inner.parse().ok()
    }

    /// The icon a shell header carries for its outcome.
    pub(crate) fn shell_status_icon(code: &str, is_error: bool) -> &'static str {
        if is_error {
            return "✗";
        }
        match code {
            "success" | "Success" => "✓",
            "detached" | "cancelled" | "Canceled" | "Cancelled" => "⊘",
            "timeout" | "Timeout" => "⏱",
            _ => "✗",
        }
    }

    fn display_width(line: &str) -> usize {
        line.chars()
            .map(|character| UnicodeWidthChar::width(character).unwrap_or(0))
            .sum()
    }

    fn target(name: &str, arguments: &Value) -> Option<String> {
        match name {
            "todo" | "list_agents" => return None,
            "plan" => {
                return Self::plan_name(arguments).or_else(|| Self::plan_action(arguments));
            }
            "spawn_agent" => return Self::spawn_target(arguments),
            "message_agent" => return Self::message_target(arguments),
            "wait_agent" => return Self::wait_target(arguments),
            "stop_agent" => return Self::stop_target(arguments),
            _ => {}
        }
        let path = arguments
            .get("path")
            .or_else(|| arguments.get("file_path"))
            .and_then(Value::as_str);
        if name == "grep" {
            return arguments
                .get("pattern")
                .and_then(Value::as_str)
                .map(|pattern| {
                    TextBuilder::grep_target(pattern, arguments.get("path").and_then(Value::as_str))
                });
        }
        if name == "shell" {
            return arguments
                .get("script")
                .or_else(|| arguments.get("command"))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if name == "write" {
            let count = arguments
                .get("content")
                .and_then(Value::as_str)
                .map_or(0, |content| content.lines().count());
            return path.map(|path| TextBuilder::write_target(path, count));
        }
        path.map(str::to_owned)
    }

    /// What a spawned child is called: its agent and its title.
    ///
    /// The prompt is what the child is given, so a title or an agent is what
    /// names the call; a call with neither shows the start of the prompt.
    fn spawn_target(arguments: &Value) -> Option<String> {
        let agent = arguments.get("agent_id").and_then(Value::as_str);
        let title = arguments.get("title").and_then(Value::as_str);
        match (agent, title) {
            (Some(agent), Some(title)) => Some(format!("{agent}  {title}")),
            (Some(agent), None) => Some(agent.to_owned()),
            (None, Some(title)) => Some(title.to_owned()),
            (None, None) => arguments
                .get("prompt")
                .and_then(Value::as_str)
                .map(|prompt| TextBuilder::truncate_inline(prompt, MESSAGE_PREVIEW)),
        }
    }

    /// The message a parent sent a child, cut to one readable line.
    fn message_target(arguments: &Value) -> Option<String> {
        let child = arguments
            .get("child_session_id")
            .and_then(Value::as_str)
            .map(|child| child.chars().take(8).collect::<String>());
        let message = arguments.get("content").and_then(Self::content_text);
        match (child, message) {
            (Some(child), Some(message)) => Some(format!(
                "{child}  {}",
                TextBuilder::truncate_inline(&message, MESSAGE_PREVIEW)
            )),
            (Some(child), None) => Some(child),
            (None, message) => {
                message.map(|message| TextBuilder::truncate_inline(&message, MESSAGE_PREVIEW))
            }
        }
    }

    /// What a wait is waiting for: the child, the condition and the ceiling.
    fn wait_target(arguments: &Value) -> Option<String> {
        let child = arguments
            .get("child_session_id")
            .and_then(Value::as_str)
            .map(|child| child.chars().take(8).collect::<String>());
        let until = arguments
            .get("until")
            .and_then(Value::as_str)
            .unwrap_or("idle");
        let timeout = arguments
            .get("timeout_s")
            .and_then(Value::as_u64)
            .unwrap_or(300);
        child.map(|child| format!("{child}  until {until}  within {timeout}s"))
    }

    /// What a stop is aimed at.
    fn stop_target(arguments: &Value) -> Option<String> {
        let child = arguments
            .get("child_session_id")
            .and_then(Value::as_str)
            .map(|child| child.chars().take(8).collect::<String>())?;
        let mode = arguments
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("graceful");
        Some(format!("{child}  {mode}"))
    }

    /// The text a `content` argument carries, whether it is a string or parts.
    fn content_text(content: &Value) -> Option<String> {
        if let Some(text) = content.as_str() {
            return Some(text.to_owned());
        }
        let parts = content.as_array()?;
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        (!text.is_empty()).then_some(text)
    }

    fn plan_name(arguments: &Value) -> Option<String> {
        let content = arguments
            .get("content")
            .or_else(|| arguments.get("plan"))
            .and_then(Value::as_str)?;
        content.lines().find_map(|line| {
            let trimmed = line.trim();
            let title = trimmed.strip_prefix('#').map(str::trim)?;
            (!title.is_empty()).then(|| title.to_owned())
        })
    }

    fn plan_action(arguments: &Value) -> Option<String> {
        arguments
            .get("action")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    fn icon(name: &str) -> &'static str {
        match name {
            "read" => "▤",
            "grep" => "⌕",
            "write" | "edit" => "✎",
            "shell" => "$",
            "todo" => "✓",
            "plan" => "☰",
            "spawn_agent" | "list_agents" => "⑃",
            "message_agent" => "✉",
            "wait_agent" => "◔",
            "stop_agent" => "⊘",
            _ => "◆",
        }
    }

    /// The box every shell block shares: the gray reaches both edges, one
    /// blank row of it above and below, one cell of inset for the text,
    /// like `px-1 py-1` on any other component.
    pub(crate) fn shell_box(
        rows: Vec<Line<'static>>,
        width: u16,
        theme: crate::ui_model::ThemeMode,
    ) -> Vec<Line<'static>> {
        let background = palette::surface(theme);
        let blank = || {
            Line::from(Span::styled(
                " ".repeat(usize::from(width)),
                Style::default().bg(background),
            ))
        };
        std::iter::once(blank())
            .chain(rows)
            .chain(std::iter::once(blank()))
            .collect()
    }

    /// One output row the way a terminal prints it: muted body, stderr in
    /// red, one cell of inset each side, the gray carried to the edges.
    fn terminal_line(
        stream: OutputStream,
        text: &str,
        width: u16,
        theme: crate::ui_model::ThemeMode,
    ) -> Line<'static> {
        let color = match stream {
            OutputStream::Stderr => palette::danger(),
            OutputStream::Stdout | OutputStream::Other => {
                palette::text_muted(crate::ui_model::ThemeMode::Dark)
            }
        };
        let background = palette::surface(theme);
        let used = Self::display_width(text);
        let padding = usize::from(width).saturating_sub(used + 2);
        Line::from(vec![
            Span::styled(" ", Style::default().bg(background)),
            Span::styled(text.to_owned(), Style::default().fg(color).bg(background)),
            Span::styled(
                " ".repeat(padding.saturating_add(1)),
                Style::default().bg(background),
            ),
        ])
        .style(Style::default().bg(background))
    }
}

#[cfg(test)]
#[path = "tool_call_tests.rs"]
mod tests;
