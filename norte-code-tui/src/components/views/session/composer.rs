//! The composer: what the reader writes, and where the focus is.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{
    chips::{Chip, ChipGroup},
    components::{ComponentAction, palette},
    keys,
    text_builder::TextBuilder,
    ui_model::{SessionAttachmentState, UiModel},
};

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
    /// The budget the reader is typing, when they are typing one.
    pub(super) budget_entry: Option<String>,
    /// The path of the file being attached, when the reader is typing one.
    pub(super) attach: Option<String>,
}

impl Composer {
    /// What the composer shows while the reader is not typing.
    pub(super) fn placeholder(&self, model: &UiModel) -> String {
        if let Some(approval) = model.budget_approval {
            return format!(
                "Budget reached: ${:.2} spent of ${:.2}. Approve one continuation?",
                approval.spent, approval.limit
            );
        }
        if let Some(path) = &self.attach {
            return format!("Attach file: {path}");
        }
        if let Some(budget) = &self.budget_entry {
            return format!(
                "Session budget USD · {}: {budget} (Enter apply, Esc cancel)",
                TextBuilder::budget_action_description(model.budget_action)
            );
        }
        if model.active_child.is_some() {
            return TextBuilder::child_session_read_only().to_owned();
        }
        if !self.text.is_empty() {
            return self.text.clone();
        }
        if model.attachment_state == SessionAttachmentState::Connecting {
            return "Connecting to session...".to_owned();
        }
        if model.attachment_state == SessionAttachmentState::Failed {
            return "Session connection failed; reopen this session to retry".to_owned();
        }
        if self.focus == SessionFocus::Composer
            && model.active_turn.is_some()
            && model.queue_mode == norte_code_wire::QueueMode::Reject
        {
            return TextBuilder::composer_busy_placeholder().to_owned();
        }
        if self.focus == SessionFocus::Composer {
            return TextBuilder::composer_focused_placeholder().to_owned();
        }
        TextBuilder::composer_mode_placeholder(keys::COMPOSER_CHAR)
    }

    /// The line under the input, when it has something only it can say.
    ///
    /// The queue mode and the reasoning level live in the header now, so the
    /// only statuses left here are the ones no other view carries.
    pub(super) fn hint(model: &UiModel) -> Option<String> {
        model
            .budget_approval
            .is_some()
            .then(|| "a approve · d decline".to_owned())
    }

    /// The style of the line under the input.
    pub(super) fn hint_style() -> Style {
        Style::default().fg(palette::muted())
    }

    /// The input lines that fit in the composer, keeping the newest ones.
    pub(super) fn visible_lines(&self, model: &UiModel) -> Vec<Line<'static>> {
        if self.text.is_empty() {
            return vec![Line::from(self.placeholder(model))];
        }
        self.text
            .lines()
            .rev()
            .take(VISIBLE_INPUT_LINES)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|line| Line::from(line.to_owned()))
            .collect()
    }

    /// How many rows the composer needs: a rule on top, the vertical air
    /// of `py-1`, the input, one line per chip and the hint line when it shows.
    pub(super) fn height(&self, hint: bool) -> u16 {
        let chips = u16::try_from(self.all_chips().len()).unwrap_or(u16::MAX);
        let input = if self.text.is_empty() {
            1_usize
        } else {
            self.text.lines().count().clamp(1, VISIBLE_INPUT_LINES)
        };
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
    pub(super) fn chip_lines(&self) -> Vec<Line<'static>> {
        self.chips
            .iter()
            .flat_map(|group| group.chips.iter())
            .map(|chip| {
                Line::from(vec![
                    Span::styled(
                        format!("{} ", crate::chips::chip_icon(chip.kind())),
                        Style::default().fg(palette::accent(crate::ui_model::ThemeMode::Dark)),
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
        self.budget_entry = None;
        self.focus = SessionFocus::Composer;
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
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if model.turn_state.is_busy() {
                return Some(ComponentAction::CancelTurn);
            }
            return None;
        }
        if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.attach = Some(String::new());
            return None;
        }
        if key.code == keys::SUBMIT && key.modifiers.contains(KeyModifiers::SHIFT) {
            self.text.push('\n');
            return None;
        }
        if key.code == KeyCode::Up {
            return Some(ComponentAction::ScrollTranscript(2));
        }
        if key.code == KeyCode::Down {
            return Some(ComponentAction::ScrollTranscript(-2));
        }
        match key.code {
            keys::CANCEL | keys::FOCUS_NEXT => self.focus = SessionFocus::Transcript,
            keys::SUBMIT
                if (!self.text.trim().is_empty() || !self.chips.is_empty())
                    && model.attachment_state == SessionAttachmentState::Attached
                    && (model.active_turn.is_none()
                        || model.queue_mode != norte_code_wire::QueueMode::Reject) =>
            {
                let text = std::mem::take(&mut self.text);
                let chips = self.take_chips();
                return Some(ComponentAction::SendMessage { text, chips });
            }
            KeyCode::Char(character) => {
                self.text.push(character);
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
        self.text.push_str(text);
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
        }
    }

    /// Erases the last thing the reader added.
    ///
    /// A chip is atomic: the whole chip goes, and the chips that travel
    /// together go together.
    fn erase(&mut self) {
        if self.text.is_empty() {
            self.chips.pop();
            return;
        }
        self.text.pop();
    }
}
