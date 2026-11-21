//! The multiline composer.
//!
//! Wraps the library `Input` with a rounded border in the theme accent and
//! a caret that breathes; typed events flow through the tree and land here
//! when it has focus. The draft behind the input keeps long pastes and
//! uploads as atomic chips.

use std::any::Any;
use std::sync::Arc;

use norte_code_wire::{UploadReceipt, UserInputPart};
use ratatui::style::Style;
use serde::Serialize;
use terminalui::Node;
use terminalui::components::chrome::{Border, BorderSet, Radius};
use terminalui::components::cursor::{CursorMode, CursorShape, CursorStyle};
use terminalui::components::input::{Input, InputStyle};
use terminalui::components::scroll::ScrollbarStyle;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

use crate::attachment::{
    AttachmentKind, AttachmentNotice, AttachmentSource, ComposerAction, MimeType,
    PreparedAttachment,
};
use crate::chip::{ChipLabel, Draft};
use crate::clipboard::{Clipboard, ClipboardContent, ClipboardImage};

/// Cap on retained sent messages; oldest fall off.
const MAX_HISTORY: usize = 200;

/// A paste longer than this becomes one chip instead of inline text.
const TEXT_CHIP_THRESHOLD: usize = 32;

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
    draft: Draft,
    history: History,
    #[serde(skip)]
    actions: Vec<ComposerAction>,
    /// Hidden outside the chat tab; keeps its focus slot to stay consistent.
    #[serde(skip)]
    visible: bool,
    #[serde(skip)]
    clipboard: Arc<dyn Clipboard>,
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
    /// Creates a composer reading from the given theme and clipboard.
    pub(crate) fn new(theme: &Theme, clipboard: Arc<dyn Clipboard>) -> Self {
        Self {
            input: Input::new()
                .multiline(true)
                .with_placeholder("Ask NORTE to do anything...")
                .with_style(input_style(theme)),
            draft: Draft::default(),
            history: History::default(),
            actions: Vec::new(),
            visible: true,
            clipboard,
        }
    }

    /// Show the composer only on the chat tab.
    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// The displayed text, including chip labels.
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        self.input.value()
    }

    /// The text the message really carries, with chips expanded.
    pub(crate) fn expanded_text(&self) -> String {
        self.draft.expanded_text()
    }

    /// The upload parts the message must carry.
    pub(crate) fn wire_parts(&self) -> Vec<UserInputPart> {
        self.draft.wire_parts()
    }

    /// Requests the app has not handled yet.
    pub(crate) fn take_actions(&mut self) -> Vec<ComposerAction> {
        std::mem::take(&mut self.actions)
    }

    /// Add the chip for an upload the host confirmed.
    pub(crate) fn insert_upload(
        &mut self,
        cursor: usize,
        kind: AttachmentKind,
        receipt: UploadReceipt,
    ) {
        let position = self.draft.insert_upload(cursor, kind, receipt);
        self.apply_draft(position, None);
    }

    /// Replace the whole draft with recalled or restored text.
    fn set_text(&mut self, text: &str) {
        self.draft.clear();
        let position = self.draft.replace_range(0, 0, text);
        self.apply_draft(position, None);
    }

    /// Replace the text; used to clear after sending.
    pub(crate) fn clear(&mut self) {
        self.draft.clear();
        self.input.clear();
    }

    /// Retain a sent message for Up/Down recall.
    pub(crate) fn push_history(&mut self, text: &str) {
        self.history.push(text);
    }

    fn apply_draft(&mut self, cursor: usize, anchor: Option<usize>) {
        let text = self.draft.text().to_owned();
        let len = self.draft.len();
        self.input.set_value(text);
        self.input.state.cursor = self.draft.normalize_index(cursor).min(len);
        self.input.state.anchor = anchor.map(|value| self.draft.normalize_index(value).min(len));
    }

    /// Mirror input edits back into the draft, expanding chip deletions.
    fn reconcile(&mut self) {
        let current = self.input.state.value.clone();
        if current != self.draft.text() {
            let old = self.draft.text().chars().collect::<Vec<_>>();
            let new = current.chars().collect::<Vec<_>>();
            let mut start = 0;
            while start < old.len() && start < new.len() && old[start] == new[start] {
                start += 1;
            }
            let mut old_end = old.len();
            let mut new_end = new.len();
            while old_end > start && new_end > start && old[old_end - 1] == new[new_end - 1] {
                old_end -= 1;
                new_end -= 1;
            }
            let inserted = new[start..new_end].iter().collect::<String>();
            self.draft.replace_range(start, old_end, &inserted);
        }
        let cursor = self.input.state.cursor;
        let anchor = self.input.state.anchor;
        let text = self.draft.text().to_owned();
        if self.input.state.value != text {
            self.input.state.value = text;
        }
        let len = self.draft.len();
        self.input.state.cursor = self.draft.normalize_index(cursor).min(len);
        self.input.state.anchor = anchor.map(|value| self.draft.normalize_index(value).min(len));
    }

    fn copy_range(&self) -> Option<(usize, usize)> {
        if let Some((start, end)) = self.input.selection()
            && start != end
        {
            return Some((start, end));
        }
        self.draft.chip_range_at(self.input.state.cursor)
    }

    /// Copy the selection, or the chip under the caret, to the system clipboard.
    pub(crate) fn copy_selection(&mut self) -> bool {
        let Some((start, end)) = self.copy_range() else {
            return false;
        };
        let text = self.draft.expanded_range(start, end);
        if text.is_empty() {
            return false;
        }
        if let Err(error) = self.clipboard.write(&text) {
            self.actions.push(ComposerAction::Notice(
                AttachmentNotice::ClipboardUnavailable {
                    reason: error.to_string(),
                },
            ));
        }
        true
    }

    /// Cut the selection to the system clipboard, deleting whole chips.
    fn cut_selection(&mut self) -> bool {
        if !self.copy_selection() {
            return false;
        }
        let Some((start, end)) = self.copy_range() else {
            return false;
        };
        let cursor = self.draft.replace_range(start, end, "");
        self.apply_draft(cursor, None);
        true
    }

    fn paste(&mut self, event_text: Option<&str>, cx: &mut Context) -> Outcome {
        match self.clipboard.read() {
            Ok(ClipboardContent::Image(image)) => self.queue_image(image),
            Ok(ClipboardContent::Files(files)) => self.queue_files(files),
            Ok(ClipboardContent::Text(text)) => {
                let paste = event_text.unwrap_or(text.as_str());
                self.insert_paste_text(paste);
            }
            Err(error) => {
                if let Some(text) = event_text {
                    self.insert_paste_text(text);
                } else {
                    self.actions.push(ComposerAction::Notice(
                        AttachmentNotice::ClipboardUnavailable {
                            reason: error.to_string(),
                        },
                    ));
                }
            }
        }
        let _ = cx.is_focused();
        Outcome::Consumed
    }

    fn insert_paste_text(&mut self, text: &str) {
        let (start, end) = self
            .input
            .selection()
            .unwrap_or((self.input.state.cursor, self.input.state.cursor));
        let position = if text.chars().count() > TEXT_CHIP_THRESHOLD {
            self.draft
                .insert_text_chip(start, crate::chip::OriginalText::new(text.to_owned()))
        } else {
            self.draft.replace_range(start, end, text)
        };
        self.apply_draft(position, None);
    }

    fn queue_image(&mut self, image: ClipboardImage) {
        let result = (|| {
            let kind = AttachmentKind::from_mime(&image.mime)?;
            Ok(PreparedAttachment {
                kind,
                mime: image.mime,
                source: AttachmentSource::Image(image.bytes),
                cursor: self.input.state.cursor,
            })
        })();
        self.queue_result(result);
    }

    fn queue_files(&mut self, files: Vec<crate::clipboard::ClipboardFile>) {
        let mut cursor = self.input.state.cursor;
        let mut counts = [0usize; 4];
        for span in self.draft.spans() {
            let Some(chip) = self.draft.chip(span.id) else {
                continue;
            };
            let Some(kind) = chip.kind().attachment() else {
                continue;
            };
            counts[attachment_slot(kind)] += 1;
        }
        for file in files {
            let result = (|| {
                let mime = MimeType::from_path(&file.path)?;
                let kind = AttachmentKind::from_mime(&mime)?;
                Ok((mime, kind))
            })();
            match result {
                Ok((mime, kind)) => {
                    let slot = attachment_slot(kind);
                    counts[slot] += 1;
                    let width = ChipLabel::uploaded(kind, counts[slot])
                        .as_str()
                        .chars()
                        .count();
                    self.actions
                        .push(ComposerAction::Attach(PreparedAttachment {
                            kind,
                            mime,
                            source: AttachmentSource::File(file.path),
                            cursor,
                        }));
                    cursor += width;
                }
                Err(error) => self.queue_result(Err(error)),
            }
        }
    }

    fn queue_result(
        &mut self,
        result: Result<PreparedAttachment, crate::attachment::AttachmentError>,
    ) {
        match result {
            Ok(attachment) => self.actions.push(ComposerAction::Attach(attachment)),
            Err(error) => self.actions.push(ComposerAction::Notice(error.into())),
        }
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
            self.set_text(&text);
            return true;
        }
        false
    }

    fn recall_newer(&mut self) -> bool {
        if let Some(text) = self.history.newer() {
            self.set_text(&text);
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

fn attachment_slot(kind: AttachmentKind) -> usize {
    match kind {
        AttachmentKind::Image => 0,
        AttachmentKind::File => 1,
        AttachmentKind::Audio => 2,
        AttachmentKind::Video => 3,
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
        use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
        if !self.visible {
            let _ = cx.is_focused();
            return Outcome::Ignored;
        }
        if let InputEvent::Key(key) = event
            && key.kind == KeyEventKind::Press
            && key.modifiers == KeyModifiers::CONTROL
        {
            match key.code {
                KeyCode::Char('x') => {
                    let _ = self.cut_selection();
                    return Outcome::Consumed;
                }
                KeyCode::Char('v') => return self.paste(None, cx),
                _ => {}
            }
        }
        if let InputEvent::Paste(text) = event {
            return self.paste(Some(text), cx);
        }
        if let InputEvent::Key(key) = event
            && self.handle_history_key(key)
        {
            return Outcome::Consumed;
        }
        let outcome = self.input.handle_event(event, cx);
        self.reconcile();
        outcome
    }

    fn focusable(&self) -> bool {
        self.input.focusable()
    }
}

#[cfg(test)]
#[path = "composer_tests.rs"]
mod tests;
