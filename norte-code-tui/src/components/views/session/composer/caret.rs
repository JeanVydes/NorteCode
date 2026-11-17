//! Where the caret sits in the composer, and how it moves.

/// The caret of the composer: an index into the text, in characters, and the
/// column it keeps while it moves between lines.
#[derive(Debug, Default)]
pub(super) struct Caret {
    index: usize,
    desired_col: Option<usize>,
}

impl Caret {
    /// The byte index of the `char_index`-th character.
    fn byte_index(text: &str, char_index: usize) -> usize {
        text.char_indices()
            .nth(char_index)
            .map_or(text.len(), |(index, _)| index)
    }

    /// How many lines the text has. A trailing newline opens one more.
    fn line_count(text: &str) -> usize {
        text.chars().filter(|character| *character == '\n').count() + 1
    }

    /// The caret's line and column, in characters.
    pub(super) fn line_col(&self, text: &str) -> (usize, usize) {
        let mut line = 0;
        let mut col = 0;
        for (index, character) in text.chars().enumerate() {
            if index == self.index {
                break;
            }
            if character == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    /// The character index of `(line, col)`, clamped to the line's end.
    fn index_at(text: &str, line: usize, col: usize) -> usize {
        let mut current_line = 0;
        let mut col_count = 0;
        for (index, character) in text.chars().enumerate() {
            if current_line == line {
                if character == '\n' || col_count == col {
                    return index;
                }
                col_count += 1;
                continue;
            }
            if character == '\n' {
                current_line += 1;
            }
        }
        text.chars().count()
    }

    /// Keeps the caret inside the text it points at.
    pub(super) fn clamp(&mut self, text: &str) {
        self.index = self.index.min(text.chars().count());
    }

    /// Puts the caret back at the start, for an empty composer.
    pub(super) fn reset(&mut self) {
        self.index = 0;
        self.desired_col = None;
    }

    /// Moves the caret one place left or right, across lines.
    pub(super) fn move_horizontal(&mut self, text: &str, delta: isize) {
        self.clamp(text);
        let end = text.chars().count();
        self.index = self.index.saturating_add_signed(delta).min(end);
        self.desired_col = None;
    }

    /// Moves the caret between lines, keeping the column it aimed for even
    /// through a line too short to reach it.
    pub(super) fn move_vertical(&mut self, text: &str, delta: isize) {
        self.clamp(text);
        let (line, col) = self.line_col(text);
        let desired = self.desired_col.unwrap_or(col);
        let last = Self::line_count(text).saturating_sub(1);
        let target = line.saturating_add_signed(delta).min(last);
        self.index = Self::index_at(text, target, desired);
        self.desired_col = Some(desired);
    }

    /// Inserts a character at the caret.
    pub(super) fn insert(&mut self, text: &mut String, character: char) {
        self.clamp(text);
        let byte = Self::byte_index(text, self.index);
        text.insert(byte, character);
        self.index += 1;
        self.desired_col = None;
    }

    /// Inserts a whole text at the caret.
    pub(super) fn insert_str(&mut self, text: &mut String, insert: &str) {
        self.clamp(text);
        let byte = Self::byte_index(text, self.index);
        text.insert_str(byte, insert);
        self.index += insert.chars().count();
        self.desired_col = None;
    }

    /// Erases the character before the caret, if there is one.
    pub(super) fn erase_before(&mut self, text: &mut String) {
        self.clamp(text);
        if self.index == 0 {
            return;
        }
        let start = Self::byte_index(text, self.index - 1);
        let end = Self::byte_index(text, self.index);
        text.replace_range(start..end, "");
        self.index -= 1;
        self.desired_col = None;
    }
}
