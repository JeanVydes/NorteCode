//! The slash-command suggestions floating above the composer.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::{Context, InputEvent, Outcome, Theme, View};

const MAX_VISIBLE: usize = 5;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct CommandSuggestion {
    pub(crate) completion: String,
    pub(crate) label: String,
}

#[derive(Serialize, Default)]
pub(crate) struct CommandSuggestions {
    values: Vec<CommandSuggestion>,
    selected: usize,
}

impl CommandSuggestions {
    pub(crate) fn set(&mut self, values: Vec<CommandSuggestion>) {
        let selected = self.selected().map(str::to_owned);
        if self.values == values {
            return;
        }
        self.selected = selected
            .and_then(|completion| {
                values
                    .iter()
                    .position(|suggestion| suggestion.completion == completion)
            })
            .unwrap_or_default();
        self.values = values;
    }

    pub(crate) fn is_open(&self) -> bool {
        !self.values.is_empty()
    }

    pub(crate) fn selected(&self) -> Option<&str> {
        self.values
            .get(self.selected)
            .map(|suggestion| suggestion.completion.as_str())
    }

    pub(crate) fn move_selection(&mut self, offset: isize) {
        if self.values.is_empty() {
            return;
        }
        let len = self.values.len().cast_signed();
        let selected = (self.selected.cast_signed() + offset).rem_euclid(len);
        self.selected = usize::try_from(selected).unwrap_or_default();
    }
}

impl View for CommandSuggestions {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "command_suggestions"
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        if self.values.is_empty() || area.width < 8 || area.height < 4 {
            return;
        }
        let visible = self.values.len().min(MAX_VISIBLE);
        let height = u16::try_from(visible + 2).unwrap_or(u16::MAX);
        let y = area
            .y
            .saturating_add(area.height.saturating_sub(7 + height));
        let width = area.width.min(48);
        let rect = Rect::new(area.x.saturating_add(1), y, width, height);
        for row in 0..rect.height {
            for col in 0..rect.width {
                buf[(rect.x + col, rect.y + row)]
                    .set_char(' ')
                    .set_style(Style::default().bg(theme.secondary).fg(theme.foreground));
            }
        }
        let border = theme.border;
        for col in 0..rect.width {
            buf[(rect.x + col, rect.y)].set_char('─').set_fg(border);
            buf[(rect.x + col, rect.y + rect.height - 1)]
                .set_char('─')
                .set_fg(border);
        }
        for row in 0..rect.height {
            buf[(rect.x, rect.y + row)].set_char('│').set_fg(border);
            buf[(rect.x + rect.width - 1, rect.y + row)]
                .set_char('│')
                .set_fg(border);
        }
        let start = self
            .selected
            .saturating_add(1)
            .saturating_sub(visible)
            .min(self.values.len().saturating_sub(visible));
        for (row, suggestion) in self.values.iter().skip(start).take(visible).enumerate() {
            let y = rect
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
            let selected = start + row == self.selected;
            let style = if selected {
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.secondary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground).bg(theme.secondary)
            };
            let label = if selected {
                format!("› {}", suggestion.label)
            } else {
                format!("  {}", suggestion.label)
            };
            let max_width = usize::from(rect.width.saturating_sub(2));
            let clipped = label.chars().take(max_width).collect::<String>();
            buf.set_string(rect.x + 1, y, clipped, style);
        }
        buf.set_string(
            rect.x + 2,
            rect.y + rect.height - 1,
            "tab complete",
            Style::default().fg(theme.muted).bg(theme.secondary),
        );
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}
