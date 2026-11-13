use norte_code_wire::{OutputStream, ToolStatus};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;

use crate::{components::palette, text_builder::TextBuilder, tools::EditDiffComponent};

pub(crate) struct ToolCallComponent;

impl ToolCallComponent {
    pub(crate) fn started(
        name: &str,
        arguments: Option<&Value>,
        source: Option<&str>,
        diff_mode: super::DiffMode,
        width: u16,
    ) -> Vec<Line<'static>> {
        let target = arguments.and_then(|arguments| Self::target(name, arguments));
        let mut lines = vec![TextBuilder::tool_call_header(name, target.as_deref())];
        if name == "edit" {
            lines.extend(EditDiffComponent::render(
                source, arguments, diff_mode, width,
            ));
        }
        lines
    }

    pub(crate) fn output(stream: OutputStream, text: &str) -> Vec<Line<'static>> {
        let style = match stream {
            OutputStream::Stdout | OutputStream::Other => Style::default().fg(palette::muted()),
            OutputStream::Stderr => Style::default().fg(palette::danger()),
        };
        let label = TextBuilder::tool_output_label(stream);
        text.lines()
            .map(|line| {
                Line::from(vec![
                    Span::styled(format!("{label} "), style),
                    Span::styled(line.to_owned(), style),
                ])
            })
            .collect()
    }

    pub(crate) fn finished(
        name: &str,
        status: ToolStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Vec<Line<'static>> {
        let mut lines = vec![TextBuilder::tool_finished(name, status)];
        if let Some(text) = error.or(output) {
            lines.extend(Self::output(
                match status {
                    ToolStatus::Error => OutputStream::Stderr,
                    ToolStatus::Ok | ToolStatus::Cancelled => OutputStream::Stdout,
                },
                text,
            ));
        }
        lines
    }

    fn target(name: &str, arguments: &Value) -> Option<String> {
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
        path.map(str::to_owned)
    }
}
