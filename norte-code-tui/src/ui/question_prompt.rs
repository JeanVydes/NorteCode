//! The ask-user overlay: answers a question the model parked a turn on.
//!
//! The host parks the turn until the client answers with the question id and
//! the answer text. This overlay collects both and reports them to the host;
//! it never talks to the network itself.

use std::any::Any;
use std::collections::BTreeSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::components::chrome::{Chrome, Padding};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

use crate::answer::QuestionAnswer;
use crate::ui_model::PendingQuestion;

/// The interaction mode of the prompt.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) enum InputMode {
    /// Navigate options; `f` switches to free text.
    #[default]
    Respond,
    /// Free text answer.
    Answer,
}

/// A floating question with answer controls.
#[derive(Default, Serialize)]
pub(crate) struct QuestionPrompt {
    /// The question, `None` when closed.
    #[serde(skip)]
    question: Option<PendingQuestion>,
    /// Which mode collects input.
    #[serde(default)]
    mode: InputMode,
    /// The option under the caret.
    #[serde(default)]
    selected: usize,
    /// The options toggled when the model allowed several.
    #[serde(default)]
    toggled: BTreeSet<usize>,
    /// The free text buffer.
    #[serde(default)]
    response: String,
    /// The committed answer waiting for the app to send.
    #[serde(skip)]
    pending_answer: Option<(String, QuestionAnswer)>,
}

impl QuestionPrompt {
    /// Creates a closed prompt.
    pub(crate) fn new() -> Self {
        Self {
            question: None,
            mode: InputMode::Respond,
            selected: 0,
            toggled: BTreeSet::new(),
            response: String::new(),
            pending_answer: None,
        }
    }

    /// Open the prompt with `question`; state resets.
    pub(crate) fn begin(&mut self, question: PendingQuestion) {
        self.question = Some(question);
        self.mode = InputMode::Respond;
        self.selected = 0;
        self.toggled = BTreeSet::new();
        self.response.clear();
        self.pending_answer = None;
    }

    /// Whether the prompt is collecting input.
    pub(crate) fn is_open(&self) -> bool {
        self.question.is_some()
    }

    /// Take the committed answer, if one is pending.
    pub(crate) fn take_pending_answer(&mut self) -> Option<(String, QuestionAnswer)> {
        self.pending_answer.take()
    }

    /// Close the prompt without answering; state resets.
    pub(crate) fn close(&mut self) {
        self.question = None;
        self.response.clear();
        self.toggled.clear();
        self.selected = 0;
        self.mode = InputMode::Respond;
        self.pending_answer = None;
    }

    fn commit(&mut self) {
        let Some(question) = &self.question else {
            return;
        };
        let text = if question.multiple && !self.toggled.is_empty() {
            self.toggled
                .iter()
                .filter_map(|index| question.options.get(*index))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        } else if let Some(option) = question.options.get(self.selected) {
            option.clone()
        } else {
            self.response.clone()
        };
        let Some(answer) = QuestionAnswer::new(text) else {
            return;
        };
        self.pending_answer = Some((question.question.clone(), answer));
        // Keep pending_answer; `close` would drop it before the app drains it.
        self.question = None;
        self.response.clear();
        self.toggled.clear();
        self.selected = 0;
        self.mode = InputMode::Respond;
    }

    fn move_down(&mut self) {
        if let Some(question) = &self.question {
            self.selected = (self.selected + 1).min(question.options.len().saturating_sub(1));
        }
    }

    fn toggle(&mut self) {
        if let Some(question) = &self.question
            && question.multiple
            && self.selected < question.options.len()
            && !self.toggled.insert(self.selected)
        {
            self.toggled.remove(&self.selected);
        }
    }

    fn push_answer_char(&mut self, character: char) {
        self.response.push(character);
    }

    fn visible_rows(&self) -> std::ops::Range<usize> {
        let Some(question) = &self.question else {
            return 0..0;
        };
        if self.selected < 8 {
            return 0..question.options.len().min(8);
        }
        let start = self.selected - 7;
        start..question.options.len().min(start + 8)
    }

    fn wrapped_prompt(&self, width: usize) -> Vec<String> {
        let Some(question) = &self.question else {
            return Vec::new();
        };
        wrap_text(&question.prompt, width)
    }
}

impl QuestionPrompt {
    fn paint_header(inner: Rect, buf: &mut Buffer, prompt_lines: &[String], theme: &Theme) -> u16 {
        let mut row = inner.y;
        let title = crate::ui::copy::ASK_USER;
        buf.set_stringn(
            inner.x,
            row,
            title,
            title.chars().count(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        row += 1;
        for line in prompt_lines {
            if row >= inner.y + inner.height.saturating_sub(1) {
                break;
            }
            let line_budget = inner.width as usize;
            buf.set_stringn(
                inner.x,
                row,
                line,
                line_budget,
                Style::default().fg(theme.foreground),
            );
            row += 1;
        }
        row
    }

    fn paint_options(&self, inner: Rect, buf: &mut Buffer, row: u16, theme: &Theme) -> u16 {
        let mut row = row;
        let Some(question) = &self.question else {
            return row;
        };
        let rows = self.visible_rows();
        for (index, option) in question
            .options
            .iter()
            .enumerate()
            .skip(rows.start)
            .take(rows.end - rows.start)
        {
            if row >= inner.y + inner.height {
                break;
            }
            let selected = index == self.selected;
            let marked = self.toggled.contains(&index);
            let row_marker = match (question.multiple, marked) {
                (true, true) => "▣ ",
                (true, false) => "▢ ",
                (false, _) if selected => "▌ ",
                _ => "  ",
            };
            let style = if selected {
                Style::default()
                    .fg(theme.secondary)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground).bg(theme.secondary)
            };
            if selected {
                buf.set_style(
                    Rect {
                        x: inner.x,
                        y: row,
                        width: inner.width,
                        height: 1,
                    },
                    Style::default().bg(theme.accent),
                );
            }
            let text = format!("{row_marker}{option}");
            buf.set_stringn(inner.x, row, &text, text.chars().count(), style);
            row += 1;
        }
        row
    }

    fn paint_answer(&self, inner: Rect, buf: &mut Buffer, row: u16, theme: &Theme) -> u16 {
        if self.mode != InputMode::Answer || row >= inner.y + inner.height {
            return row;
        }
        buf.set_stringn(
            inner.x,
            row,
            crate::ui::copy::ANSWER,
            6,
            Style::default().fg(theme.muted),
        );
        let row = row + 1;
        let visual = format!("{}▏", self.response);
        buf.set_stringn(
            inner.x,
            row,
            &visual,
            visual.chars().count(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        row + 1
    }

    fn paint_hint(&self, inner: Rect, buf: &mut Buffer, row: u16, theme: &Theme) {
        let hint = match self.mode {
            InputMode::Respond if self.question.as_ref().is_some_and(|q| q.options.is_empty()) => {
                "enter: escribir respuesta"
            }
            InputMode::Respond => "↑↓ elegir · space marcar · enter enviar · f escribir",
            InputMode::Answer => "enter: enviar · esc: volver",
        };
        let _ = row;
        buf.set_stringn(
            inner.x,
            inner.y + inner.height - 1,
            hint,
            inner.width as usize,
            Style::default().fg(theme.muted),
        );
    }
}

#[cfg(test)]
#[path = "question_prompt_tests.rs"]
mod tests;

impl View for QuestionPrompt {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "question_prompt"
    }

    fn children(&self) -> &[terminalui::Node] {
        &[]
    }

    fn children_mut(&mut self) -> &mut [terminalui::Node] {
        &mut []
    }

    fn measure(&self) -> Option<(u16, u16)> {
        None
    }

    fn focusable(&self) -> bool {
        self.is_open()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        if !self.is_open() {
            return;
        }
        let inner_width = (area.width.saturating_sub(8)).min(88);
        let full_prompt_lines = self.wrapped_prompt(usize::from(inner_width));
        let prompt_max = 4usize;
        let mut prompt_lines: Vec<String> =
            full_prompt_lines.iter().take(prompt_max).cloned().collect();
        if full_prompt_lines.len() > prompt_max
            && let Some(last) = prompt_lines.last_mut()
        {
            last.truncate(last.len().saturating_sub(1));
            last.push('…');
        }
        let options_lines = self.visible_rows().len();
        let answer_rows = if self.mode == InputMode::Answer { 2 } else { 0 };
        let body_rows = 1 + prompt_lines.len() + options_lines + answer_rows + 1;
        let height = u16::try_from(body_rows.max(6))
            .unwrap_or(9)
            .max(9)
            .min(area.height);
        let width = (inner_width + 4).min(area.width);
        let x = area.x + (area.width - width) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let panel = Rect::new(x, y, width, height);

        let chrome = Chrome::themed(
            theme,
            Some(theme.accent),
            Some(theme.secondary),
            Padding {
                horizontal: 2,
                vertical: 1,
            },
        );
        let inner = terminalui::components::chrome::draw_box(buf, panel, &chrome, theme);
        let mut row = Self::paint_header(inner, buf, &prompt_lines, theme);
        row = self.paint_options(inner, buf, row, theme);
        row = self.paint_answer(inner, buf, row, theme);
        self.paint_hint(inner, buf, row, theme);
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        if !self.is_open() {
            return Outcome::Ignored;
        }
        match event {
            InputEvent::Paste(text) => {
                if self.mode == InputMode::Answer {
                    self.response.push_str(text);
                    Outcome::Consumed
                } else {
                    Outcome::Ignored
                }
            }
            InputEvent::Key(key) => {
                use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
                if key.kind != KeyEventKind::Press {
                    return Outcome::Ignored;
                }
                match (self.mode, key.modifiers, key.code) {
                    (InputMode::Answer, KeyModifiers::NONE, KeyCode::Esc) => {
                        self.mode = InputMode::Respond;
                    }
                    (InputMode::Answer, KeyModifiers::NONE, KeyCode::Backspace) => {
                        self.response.pop();
                    }
                    (InputMode::Answer, KeyModifiers::NONE, KeyCode::Char(character)) => {
                        self.push_answer_char(character);
                    }
                    (InputMode::Respond, KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k')) => {
                        self.selected = self.selected.saturating_sub(1);
                    }
                    (
                        InputMode::Respond,
                        KeyModifiers::NONE,
                        KeyCode::Down | KeyCode::Char('j'),
                    ) => {
                        self.move_down();
                    }
                    (InputMode::Respond, KeyModifiers::NONE, KeyCode::Char(' ')) => {
                        self.toggle();
                    }
                    (InputMode::Respond, KeyModifiers::NONE, KeyCode::Enter) => {
                        if self.question.as_ref().is_some_and(|q| q.options.is_empty()) {
                            self.mode = InputMode::Answer;
                        } else {
                            self.commit();
                        }
                    }
                    (InputMode::Respond, KeyModifiers::NONE, KeyCode::Char('f')) => {
                        self.mode = InputMode::Answer;
                    }
                    (_, KeyModifiers::NONE, KeyCode::Enter) => self.commit(),
                    _ => return Outcome::Ignored,
                }
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.len() + word.len() + 1 > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
