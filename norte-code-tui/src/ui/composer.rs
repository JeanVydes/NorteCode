//! The multiline composer.
//!
//! Wraps the library `Input` with a rounded border in the theme accent and
//! a caret that breathes; typed events flow through the tree and land here
//! when it has focus.

use std::any::Any;

use ratatui::style::Style;
use serde::Serialize;
use terminalui::Node;
use terminalui::components::chrome::{Border, BorderSet, Radius};
use terminalui::components::cursor::{CursorMode, CursorShape, CursorStyle};
use terminalui::components::input::{Input, InputStyle};
use terminalui::components::scroll::ScrollbarStyle;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// Cap on retained sent messages; oldest fall off.
const MAX_HISTORY: usize = 200;

/// Sent-message history with the unsent draft stashed while browsing.
#[derive(Default, serde::Serialize)]
struct History {
    /// Oldest first.
    entries: Vec<String>,
    /// Position into `entries` from the back while browsing; `0` is the
    /// newest. `None` when the user is not browsing.
    browse: Option<usize>,
    /// The text in the composer when browsing began.
    draft: String,
}

impl History {
    /// Retain a sent message; browsing resets.
    fn push(&mut self, text: &str) {
        if text.is_empty() || self.entries.last().is_some_and(|last| last == text) {
            return;
        }
        self.entries.push(text.to_owned());
        if self.entries.len() > MAX_HISTORY {
            self.entries.remove(0);
        }
        self.browse = None;
        self.draft.clear();
    }

    /// Recall the next-older message; `draft` is the text being edited.
    fn older(&mut self, draft: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let offset = match self.browse {
            None => {
                self.draft = draft.to_string();
                0
            }
            Some(offset) => {
                if offset + 1 >= self.entries.len() {
                    return None;
                }
                offset + 1
            }
        };
        self.browse = Some(offset);
        Some(self.entries[self.entries.len() - 1 - offset].clone())
    }

    /// Recall the next-newer message, or the stashed draft at the bottom.
    fn newer(&mut self) -> Option<String> {
        let offset = self.browse?;
        if offset == 0 {
            self.browse = None;
            return Some(std::mem::take(&mut self.draft));
        }
        self.browse = Some(offset - 1);
        Some(self.entries[self.entries.len() - offset].clone())
    }
}

/// The composed composer box; the tree knows it as one node.
#[derive(Serialize)]
pub(crate) struct Composer {
    input: Input,
    history: History,
    /// Hidden outside the chat tab; keeps its focus slot to stay consistent.
    #[serde(skip)]
    visible: bool,
}

/// The input chrome, rebuilt from the theme in use.
fn input_style(theme: &Theme) -> InputStyle {
    InputStyle {
        border: Some(Border {
            set: BorderSet::Rounded,
            color: Some(theme.accent),
            radius: Radius::Small,
        }),
        radius: Radius::Small,
        background: Some(theme.secondary),
        foreground: Some(theme.foreground),
        placeholder: Some(theme.muted),
        cursor: Some(theme.accent),
        scrollbar: ScrollbarStyle::default(),
        cursor_style: CursorStyle {
            shape: CursorShape::Line,
            mode: CursorMode::Pulse,
            blink_ms: 530,
            opacity: 1.0,
            easing: terminalui::Easing::EaseInOutSine,
        },
    }
}

impl Composer {
    /// Creates a composer reading from the given theme.
    pub(crate) fn new(theme: &Theme) -> Self {
        Self {
            input: Input::new()
                .multiline(true)
                .with_placeholder("Ask NORTE to do anything...")
                .with_style(input_style(theme)),
            history: History::default(),
            visible: true,
        }
    }

    /// Show the composer only on the chat tab.
    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// The current text, trimmed for dispatch.
    pub(crate) fn text(&self) -> &str {
        self.input.value()
    }

    /// Replace the text; used to clear after sending.
    pub(crate) fn clear(&mut self) {
        self.input.clear();
    }

    /// Retain a sent message for Up/Down recall.
    pub(crate) fn push_history(&mut self, text: &str) {
        self.history.push(text);
    }

    /// The caret's logical line, `(line, last_line)`.
    fn caret_line(&self) -> (usize, usize) {
        let mut line = 0usize;
        for (index, ch) in self.input.state.value.chars().enumerate() {
            if index == self.input.state.cursor {
                break;
            }
            if ch == '\n' {
                line += 1;
            }
        }
        let last_line = self
            .input
            .state
            .value
            .chars()
            .filter(|ch| *ch == '\n')
            .count();
        (line, last_line)
    }

    fn caret_on_first_line(&self) -> bool {
        self.caret_line().0 == 0
    }

    fn caret_on_last_line(&self) -> bool {
        let (line, last_line) = self.caret_line();
        line == last_line
    }

    fn recall_older(&mut self) -> bool {
        if self.input.state.disabled {
            return false;
        }
        if let Some(text) = self.history.older(self.input.value()) {
            self.input.set_value(text);
            return true;
        }
        false
    }

    fn recall_newer(&mut self) -> bool {
        if let Some(text) = self.history.newer() {
            self.input.set_value(text);
            return true;
        }
        false
    }

    /// History recall on bare arrows; everything else flows to the input.
    fn handle_history_key(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
        if key.kind != KeyEventKind::Press || key.modifiers != KeyModifiers::NONE {
            return false;
        }
        match key.code {
            KeyCode::Up if self.caret_on_first_line() => self.recall_older(),
            KeyCode::Down if self.caret_on_last_line() => self.recall_newer(),
            _ => false,
        }
    }
}

impl View for Composer {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "composer"
    }

    fn children(&self) -> &[Node] {
        &[]
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut []
    }

    fn measure(&self) -> Option<(u16, u16)> {
        None
    }

    fn render(
        &mut self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::buffer::Buffer,
        cx: &mut Context,
        theme: &Theme,
    ) {
        self.input.style = input_style(theme);
        if self.visible {
            self.input.render(area, buf, cx, theme);
        } else {
            // Erase the box left by previous frames, then keep the focus
            // counter in step with the tree's count_focusable.
            buf.set_style(area, Style::default().bg(theme.background));
            for y in area.y..area.y + area.height {
                buf.set_stringn(area.x, y, "", 0, Style::default());
                for x in area.x..area.x + area.width {
                    if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                        cell.set_symbol(" ");
                        cell.set_style(Style::default().bg(theme.background));
                    }
                }
            }
            let _ = cx.is_focused();
        }
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        if !self.visible {
            let _ = cx.is_focused();
            return Outcome::Ignored;
        }
        if let InputEvent::Key(key) = event
            && self.handle_history_key(key)
        {
            return Outcome::Consumed;
        }
        self.input.handle_event(event, cx)
    }

    fn focusable(&self) -> bool {
        self.input.focusable()
    }
}

#[cfg(test)]
#[path = "composer_tests.rs"]
mod tests;
