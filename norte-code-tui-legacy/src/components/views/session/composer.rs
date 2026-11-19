//! The composer: what the reader writes, and where the focus is.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::{
    chips::{Chip, ChipGroup},
    components::ComponentAction,
    keys,
    text_builder::TextBuilder,
    ui_model::{SessionAttachmentState, UiModel},
};

mod caret;

use caret::Caret;

/// How much text the composer shows inline before it becomes a chip.
pub(crate) const INLINE_LIMIT: usize = 120;

/// How many input lines the composer keeps visible.
const VISIBLE_INPUT_LINES: usize = 5;

#[cfg(test)]
#[path = "composer_tests.rs"]
mod tests;

/// Where the keys go.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum SessionFocus {
    Transcript,
    #[default]
    Composer,
}

/// The composer of one session.
#[derive(Default)]
pub(super) struct Composer {
    /// What the reader wrote.
    pub(super) text: String,
    /// The chips, as units: a group is deleted as a whole.
    pub(super) chips: Vec<ChipGroup>,
    /// Where the keys go.
    pub(super) focus: SessionFocus,
    /// The path of the file being attached, when the reader is typing one.
    pub(super) attach: Option<String>,
    /// Where the caret sits.
    caret: Caret,
}

impl Composer {
    /// What the composer shows while the reader is not typing.
    pub(super) fn placeholder(&self, model: &UiModel) -> String {
        if let Some(path) = &self.attach {
            return format!("Attach file: {path}");
        }
        if model.active_child.is_some() {
            return TextBuilder::child_session_read_only().to_owned();
        }
        if model.attachment_state == SessionAttachmentState::Connecting {
            return "Connecting to session...".to_owned();
        }
        if model.attachment_state == SessionAttachmentState::Failed {
            return "Session connection failed; reopen this session to retry".to_owned();
        }
        if self.focus == SessionFocus::Composer
            && model.active_turn.is_some()
            && model.send_mode == crate::ui_model::SendMode::Reject
        {
            return TextBuilder::composer_busy_placeholder().to_owned();
        }
        if self.focus == SessionFocus::Composer {
            return TextBuilder::composer_focused_placeholder().to_owned();
        }
        TextBuilder::composer_placeholder()
    }

    /// The line the caret is on, drawn with the caret in it.
    fn line_with_caret(line: String, col: usize) -> Line<'static> {
        let chars: Vec<char> = line.chars().collect();
        if col >= chars.len() {
            // The caret sits at the end of the line.
            return Line::from(vec![
                Span::raw(line),
                Span::styled(
                    " ".to_owned(),
                    Style::default().add_modifier(Modifier::REVERSED),
                ),
            ]);
        }
        let pre: String = chars[..col].iter().collect();
        let current = chars[col].to_string();
        let post: String = chars[col + 1..].iter().collect();
        Line::from(vec![
            Span::raw(pre),
            Span::styled(current, Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(post),
        ])
    }

    /// Whether the composer owns the keys.
    pub(super) const fn is_focused(&self) -> bool {
        matches!(self.focus, SessionFocus::Composer)
    }

    /// The lines that fit in the input area, the caret's line always visible.
    ///
    /// A composer that does not own the keys draws no caret: a blinking cursor
    /// in an input that ignores what you type is a lie.
    pub(super) fn visible_lines(&self, model: &UiModel) -> Vec<Line<'static>> {
        if self.text.is_empty() {
            return vec![Line::from(self.placeholder(model))];
        }
        if !self.is_focused() {
            return self
                .text
                .split('\n')
                .take(VISIBLE_INPUT_LINES)
                .map(|line| Line::from(line.to_owned()))
                .collect();
        }
        let lines: Vec<&str> = self.text.split('\n').collect();
        let (caret_line, caret_col) = self.caret.line_col(&self.text);
        let start = caret_line.saturating_sub(VISIBLE_INPUT_LINES - 1);
        lines
            .iter()
            .skip(start)
            .take(VISIBLE_INPUT_LINES)
            .enumerate()
            .map(|(index, line)| {
                if start + index == caret_line {
                    Self::line_with_caret((*line).to_owned(), caret_col)
                } else {
                    Line::from((*line).to_owned())
                }
            })
            .collect()
    }

    /// How many rows the composer needs: a rule on top, the vertical air
    /// of `py-1`, the input, one line per chip and the hint line when it shows.
    pub(super) fn height(&self, hint: bool) -> u16 {
        let chips = u16::try_from(self.all_chips().len()).unwrap_or(u16::MAX);
        let input = self.text.split('\n').count().clamp(1, VISIBLE_INPUT_LINES);
        let input = u16::try_from(input).unwrap_or(u16::MAX);
        1 + 1 + input + chips + u16::from(hint) + 1
    }

    /// The chips the composer holds, in the order they were added.
    pub(super) fn all_chips(&self) -> Vec<&Chip> {
        self.chips
            .iter()
            .flat_map(|group| group.chips.iter())
            .collect()
    }

    /// The lines the chips are drawn with.
    pub(super) fn chip_lines(&self, theme: crate::ui_model::ThemeMode) -> Vec<Line<'static>> {
        self.chips
            .iter()
            .flat_map(|group| group.chips.iter())
            .map(|chip| crate::chips::chip_line(chip, theme))
            .collect()
    }

    /// Takes the chips the composer holds.
    pub(super) fn take_chips(&mut self) -> Vec<Chip> {
        let chips = self.all_chips().into_iter().cloned().collect();
        self.chips.clear();
        chips
    }

    /// Empties the composer, for a session that starts clean.
    pub(super) fn reset(&mut self) {
        self.text.clear();
        self.chips.clear();
        self.attach = None;
        self.focus = SessionFocus::Composer;
        self.caret.reset();
    }

    /// Adds a chip to the composer.
    pub(super) fn add_chip(&mut self, chip: Chip) {
        match self.chips.last_mut() {
            Some(group) if group.chips.len() == 1 && group.chips[0].kind == chip.kind => {
                group.chips.push(chip);
            }
            _ => self.chips.push(ChipGroup::one(chip)),
        }
    }

    /// Handles the keys that reach the composer.
    pub(super) fn handle_key(&mut self, key: KeyEvent, model: &UiModel) -> Option<ComponentAction> {
        if self.attach.is_some() {
            return self.handle_attach_key(key);
        }
        if keys::is_cancel_turn(key) {
            if model.turn_state.is_busy() {
                return Some(ComponentAction::CancelTurn);
            }
            return None;
        }
        if keys::is_attach(key) {
            self.attach = Some(String::new());
            return None;
        }
        // A new line: Shift+Enter where the terminal reports modifiers, and
        // Ctrl+J everywhere, which is the chord terminals send for the same
        // thing when they cannot tell the two Enters apart.
        let new_line = (key.code == keys::SUBMIT && key.modifiers.contains(KeyModifiers::SHIFT))
            || keys::is_newline(key);
        if new_line {
            self.caret.insert(&mut self.text, '\n');
            self.chipify_long_text();
            return None;
        }
        match key.code {
            KeyCode::Up => {
                self.caret.move_vertical(&self.text, -1);
                return None;
            }
            KeyCode::Down => {
                self.caret.move_vertical(&self.text, 1);
                return None;
            }
            KeyCode::Left => {
                self.caret.move_horizontal(&self.text, -1);
                return None;
            }
            KeyCode::Right => {
                self.caret.move_horizontal(&self.text, 1);
                return None;
            }
            KeyCode::Home => {
                self.caret.move_to_line_start(&self.text);
                return None;
            }
            KeyCode::End => {
                self.caret.move_to_line_end(&self.text);
                return None;
            }
            KeyCode::Delete => {
                self.caret.erase_after(&mut self.text);
                return None;
            }
            _ => {}
        }
        match key.code {
            keys::CANCEL | keys::NEXT_TAB => self.focus = SessionFocus::Transcript,
            keys::SUBMIT
                if (!self.text.trim().is_empty() || !self.chips.is_empty())
                    && model.attachment_state == SessionAttachmentState::Attached
                    && (model.active_turn.is_none()
                        || model.send_mode != crate::ui_model::SendMode::Reject) =>
            {
                let text = std::mem::take(&mut self.text);
                let chips = self.take_chips();
                self.caret.reset();
                return Some(ComponentAction::SendMessage { text, chips });
            }
            KeyCode::Char(character) => {
                self.caret.insert(&mut self.text, character);
                self.chipify_long_text();
            }
            keys::ERASE => {
                self.erase();
            }
            _ => {}
        }
        None
    }

    /// Takes a pasted text as one unit.
    ///
    /// A paste that is too long to read inline becomes one chip, so the
    /// composer stays short and the paste is deleted as a whole.
    pub(super) fn paste(&mut self, text: &str) {
        if let Some(path) = &mut self.attach {
            path.push_str(text);
            return;
        }
        if text.chars().count() > INLINE_LIMIT {
            self.chips.push(ChipGroup::one(Chip::text(text.to_owned())));
            return;
        }
        self.caret.insert_str(&mut self.text, text);
    }

    /// Handles the keys that reach the attach prompt.
    fn handle_attach_key(&mut self, key: KeyEvent) -> Option<ComponentAction> {
        match key.code {
            keys::CANCEL => self.attach = None,
            keys::SUBMIT => {
                let path = self.attach.take()?;
                let path = path.trim().to_owned();
                if !path.is_empty() {
                    return Some(ComponentAction::AttachFile(path));
                }
            }
            keys::ERASE => {
                if let Some(path) = &mut self.attach {
                    path.pop();
                }
            }
            KeyCode::Char(character) => {
                if let Some(path) = &mut self.attach {
                    path.push(character);
                }
            }
            _ => {}
        }
        None
    }

    /// Turns text that grew past the limit into a chip.
    fn chipify_long_text(&mut self) {
        if self.text.chars().count() > INLINE_LIMIT {
            let text = std::mem::take(&mut self.text);
            self.chips.push(ChipGroup::one(Chip::text(text)));
            self.caret.reset();
        }
    }

    /// Erases the thing before the caret.
    ///
    /// A chip is atomic: the whole chip goes, and the chips that travel
    /// together go together.
    fn erase(&mut self) {
        if self.text.is_empty() {
            self.chips.pop();
            return;
        }
        self.caret.erase_before(&mut self.text);
    }
}
