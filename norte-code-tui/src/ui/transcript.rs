//! The conversation stream.
//!
//! Items arrive from the host and are painted bottom-pinned in a windowed
//! view: only visible rows render, and per-item heights are cached instead
//! of measured every frame. While an agent turn is in flight a thinking row
//! is appended after the newest content.

use std::any::Any;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEventKind};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::diff::LineKind;
use serde::Serialize;

use terminalui::components::console::Console;
use terminalui::components::scroll::{Scroll, ScrollbarStyle, render_scrollbar};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BLOCK_GAP: u16 = 1;

/// Execution state of a tool block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub(crate) enum ToolState {
    /// Completed successfully.
    Done,
    /// In flight right now.
    Running,
}

/// How loud a notice is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub(crate) enum Tone {
    /// Informational; muted.
    Neutral,
    /// Something needs attention.
    Warn,
    /// Something failed.
    Error,
}

/// One block of the stream.
#[derive(Serialize)]
#[non_exhaustive]
pub(crate) enum Item {
    /// The user asked.
    User {
        /// The raw text.
        text: String,
    },
    /// The assistant answered; inline code draws as accented spans.
    Assistant {
        /// The raw text.
        text: String,
    },
    /// A tool call with a trailing status glyph.
    Tool {
        /// The call id the host names; used to match completions.
        call_id: String,
        /// Tool name, e.g. `grep`.
        name: String,
        /// Arguments or target, when the host reports one.
        target: Option<String>,
        /// Execution state.
        state: ToolState,
        /// Optional meta column, e.g. `+41 −12`.
        meta: Option<String>,
        /// The change an edit made, drawn under its header.
        diff: Option<crate::diff::Diff>,
        /// A shell call's live output, under its header.
        console: Option<Box<Console>>,
    },
    /// A host notice, e.g. `model switched to claude-sonnet-4.5`.
    Notice {
        /// The notice text.
        text: String,
        /// How loud the notice is.
        tone: Tone,
    },
    /// A retry announcement; updates in place so a retry storm is one line.
    Retry {
        /// The retry text.
        text: String,
        /// How loud the notice is.
        tone: Tone,
    },
    /// Reasoning text, muted; collapsed by default, click expands.
    Reasoning {
        /// The raw text.
        text: String,
        /// Whether the block is expanded in place.
        expanded: bool,
    },
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for logical in text.lines() {
        if logical.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut cells = 0usize;
        for word in logical.split_whitespace() {
            let word_cells = display_width(word);
            let needed = if cells == 0 {
                word_cells
            } else {
                cells + 1 + word_cells
            };
            if needed > width && cells > 0 {
                lines.push(std::mem::take(&mut current));
                cells = 0;
            }
            if word_cells > width {
                let mut chunk = String::new();
                let mut chunk_cells = 0usize;
                for character in word.chars() {
                    let size = char_width(character);
                    if chunk_cells + size > width && !chunk.is_empty() {
                        lines.push(std::mem::take(&mut chunk));
                        chunk_cells = 0;
                    }
                    chunk.push(character);
                    chunk_cells += size;
                }
                current = chunk;
                cells = chunk_cells;
                continue;
            }
            if cells > 0 {
                current.push(' ');
                cells += 1;
            }
            current.push_str(word);
            cells += word_cells;
        }
        lines.push(current);
    }
    if text.ends_with('\n') {
        lines.push(String::new());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

fn char_width(character: char) -> usize {
    unicode_width::UnicodeWidthChar::width(character).unwrap_or(0)
}

/// The conversation stream, bottom-pinned.
#[derive(Serialize)]
pub(crate) struct Transcript {
    items: Vec<Item>,
    #[serde(skip)]
    first_visible: u16,
    #[serde(skip)]
    view_height: u16,
    #[serde(skip)]
    heights: Vec<u16>,
    #[serde(skip)]
    content_height: u16,
    #[serde(skip)]
    heights_width: u16,
    #[serde(skip)]
    dirty: bool,
    /// Time spent in the current thinking phase; `None` when idle.
    #[serde(skip)]
    thinking: Option<Duration>,
    /// The y of the item area, set during render; used for click hit tests.
    #[serde(skip)]
    origin_y: u16,
    /// Whether the view follows the tail as new content streams in.
    #[serde(skip)]
    follow_tail: bool,
    /// The transcript's area; mouse hit-testing is meaningless without it.
    #[serde(skip)]
    area: Rect,
    /// The in-progress text selection.
    #[serde(skip)]
    selection: Option<Selection>,
    /// The painted lines of the last render, one per row.
    #[serde(skip)]
    painted: Vec<String>,
    /// Whether the row's source item may be copied or highlighted.
    #[serde(skip)]
    painted_copyable: Vec<bool>,
    /// Whether a row carries the user message's leading bar.
    #[serde(skip)]
    painted_user: Vec<bool>,
    /// How edit diffs are drawn.
    #[serde(skip)]
    diff_mode: crate::diff::DiffMode,
}

/// A cell position in transcript coordinates (rows from the transcript's top).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Pos {
    /// Row within the transcript.
    row: u16,
    /// Column within the transcript.
    col: u16,
}

/// The drag-selected range.
#[derive(Clone, Copy, Debug)]
struct Selection {
    /// Where the drag started.
    anchor: Pos,
    /// Where the drag currently is.
    cur: Pos,
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcript {
    /// Creates a bottom-pinned empty stream.
    pub(crate) fn new() -> Self {
        Self {
            items: Vec::new(),
            first_visible: u16::MAX,
            view_height: 0,
            heights: Vec::new(),
            content_height: 0,
            heights_width: 0,
            dirty: true,
            thinking: None,
            origin_y: 0,
            area: Rect::new(0, 0, 0, 0),
            selection: None,
            painted: Vec::new(),
            painted_copyable: Vec::new(),
            painted_user: Vec::new(),
            diff_mode: crate::diff::DiffMode::Unified,
            follow_tail: true,
        }
    }

    /// Set the diff layout; a change rebuilds the cached row heights.
    pub(crate) fn set_diff_mode(&mut self, mode: crate::diff::DiffMode) {
        if self.diff_mode != mode {
            self.diff_mode = mode;
            self.dirty = true;
        }
    }

    /// Forget everything and pin the view to the bottom.
    pub(crate) fn clear_all(&mut self) {
        self.items.clear();
        self.thinking = None;
        self.dirty = true;
        self.first_visible = u16::MAX;
        self.follow_tail = true;
    }

    /// Replace the stream with a history page; scroll pins to the bottom.
    pub(crate) fn replace_history(&mut self, items: Vec<norte_code_wire::HistoryItem>) {
        self.items.clear();
        self.thinking = None;
        let mut last_shell: Option<usize> = None;
        for item in items {
            match item.kind.as_str() {
                "user" => self.items.push(Item::User { text: item.content }),
                "assistant" => self.items.push(Item::Assistant { text: item.content }),
                "reasoning" => self.items.push(Item::Reasoning {
                    text: item.content,
                    expanded: false,
                }),
                "refusal" => self.items.push(Item::Notice {
                    text: item.content,
                    tone: Tone::Warn,
                }),
                "tool_result" => {
                    if let Some(index) = last_shell
                        && let Some(Item::Tool {
                            console: Some(console),
                            ..
                        }) = self.items.get_mut(index)
                    {
                        for line in item.content.lines() {
                            console.push_line(line.to_owned());
                        }
                    }
                }
                "system_event" | "tool" | "tool_call" => {
                    let call_id = item.id.clone();
                    let mut lines = item.content.lines();
                    let first = lines.next().unwrap_or("tool").trim();
                    let has_brace = first.ends_with('{');
                    let summary = first.strip_suffix('{').unwrap_or(first).trim().to_owned();
                    let rest = lines.collect::<Vec<_>>().join("\n");
                    let arguments = if has_brace { format!("{{{rest}") } else { rest };
                    let edit = serde_json::from_str::<serde_json::Value>(&arguments)
                        .ok()
                        .and_then(|value| {
                            serde_json::from_value::<crate::diff::EditArgs>(value).ok()
                        });
                    let shell = serde_json::from_str::<serde_json::Value>(&arguments)
                        .ok()
                        .and_then(|value| {
                            serde_json::from_value::<crate::shell::ShellArgs>(value).ok()
                        });
                    let command = shell.as_ref().and_then(crate::shell::ShellArgs::command);
                    let is_shell = matches!(summary.as_str(), "shell" | "bash");
                    let target = edit
                        .as_ref()
                        .and_then(|edit| edit.path.clone())
                        .or_else(|| command.map(str::to_owned));
                    let diff = edit
                        .as_ref()
                        .and_then(|edit| crate::diff::Diff::from_call(None, edit));
                    let console = if is_shell {
                        command.map(|command| Box::new(Console::new(command).with_max_visible(6)))
                    } else {
                        None
                    };
                    if console.is_some() {
                        last_shell = Some(self.items.len());
                    }
                    self.items.push(Item::Tool {
                        call_id,
                        name: if summary.len() > 40 {
                            let clipped: String = summary.chars().take(39).collect();
                            format!("{clipped}…")
                        } else {
                            summary
                        },
                        target,
                        state: ToolState::Done,
                        meta: None,
                        diff,
                        console,
                    });
                }
                _ => {}
            }
        }
        self.dirty = true;
        self.first_visible = u16::MAX;
        self.follow_tail = true;
    }

    /// Append an item; the view stays pinned to the bottom while following the tail.
    pub(crate) fn push(&mut self, item: Item) {
        self.items.push(item);
        self.dirty = true;
        if self.follow_tail {
            self.first_visible = u16::MAX;
        }
    }

    /// Report a retry; the previous retry line is rewritten in place so a
    /// failure streak stays one line. A streak separated from an older one
    /// by other items starts a new line.
    pub(crate) fn push_retry(&mut self, text: String, tone: Tone) {
        if let Some(Item::Retry {
            text: current,
            tone: current_tone,
        }) = self.items.last_mut()
        {
            *current = text;
            *current_tone = tone;
            self.dirty = true;
            return;
        }
        self.push(Item::Retry { text, tone });
    }

    /// Begin the thinking row.
    pub(crate) fn start_thinking(&mut self) {
        if self.thinking.is_none() {
            self.thinking = Some(Duration::from_millis(1));
        }
    }

    /// End the thinking row.
    pub(crate) fn stop_thinking(&mut self) {
        self.thinking = None;
    }

    /// Flip every `Running` tool block to `Done`.
    pub(crate) fn complete_running_tools(&mut self) {
        for item in &mut self.items {
            if let Item::Tool { state, .. } = item
                && *state == ToolState::Running
            {
                *state = ToolState::Done;
            }
        }
        self.dirty = true;
    }

    /// Append a line of output to the console with the given call id.
    pub(crate) fn append_console_line(&mut self, call_id: &str, line: String) -> bool {
        for item in self.items.iter_mut().rev() {
            if let Item::Tool {
                call_id: id,
                console: Some(console),
                ..
            } = item
                && *id == call_id
            {
                console.push_line(line);
                self.dirty = true;
                return true;
            }
        }
        false
    }

    /// Flip the `Running` tool block with the given call id to `Done`.
    pub(crate) fn finish_tool(&mut self, call_id: &str) {
        for item in self.items.iter_mut().rev() {
            if let Item::Tool {
                call_id: id, state, ..
            } = item
                && *id == call_id
                && *state == ToolState::Running
            {
                *state = ToolState::Done;
                self.dirty = true;
                return;
            }
        }
    }

    /// Append assistant text; merges into the trailing assistant block.
    pub(crate) fn push_assistant_text(&mut self, text: &str) {
        if let Some(Item::Assistant { text: existing }) = self.items.last_mut() {
            existing.push_str(text);
            self.dirty = true;
            return;
        }
        self.push(Item::Assistant {
            text: text.to_string(),
        });
    }

    /// Append reasoning text to the block for this turn.
    ///
    /// Assistant text and tools arrive between reasoning deltas, so the
    /// block is not necessarily the last item; it is the last one before
    /// the next user message.
    pub(crate) fn push_reasoning_text(&mut self, text: &str) {
        let mut target = None;
        for index in (0..self.items.len()).rev() {
            match &self.items[index] {
                Item::User { .. } => break,
                Item::Reasoning { .. } => {
                    target = Some(index);
                    break;
                }
                _ => {}
            }
        }
        if let Some(index) = target
            && let Item::Reasoning { text: existing, .. } = &mut self.items[index]
        {
            existing.push_str(text);
            self.dirty = true;
            return;
        }
        self.push(Item::Reasoning {
            text: text.to_string(),
            expanded: false,
        });
    }

    fn item_height(item: &Item, width: usize, diff_mode: crate::diff::DiffMode) -> u16 {
        match item {
            Item::User { text }
            | Item::Assistant { text }
            | Item::Notice { text, .. }
            | Item::Retry { text, .. } => {
                u16::try_from(wrap(text, width).len()).unwrap_or(u16::MAX) + BLOCK_GAP
            }
            Item::Reasoning { text, expanded } => {
                if *expanded {
                    u16::try_from(wrap(text, width).len() + 1).unwrap_or(u16::MAX) + BLOCK_GAP
                } else {
                    1 + BLOCK_GAP
                }
            }
            Item::Tool { diff, console, .. } => {
                let extra = diff.as_ref().map_or(0, |diff| diff.rows(diff_mode) + 2)
                    + console.as_ref().map_or(0, |console| {
                        console.measure().map_or(2, |(_, height)| height as usize) + 1
                    });
                u16::try_from(extra + 2).unwrap_or(u16::MAX)
            }
        }
    }

    fn compute_heights(&mut self, width: u16) {
        if !self.dirty && self.heights_width == width {
            return;
        }
        let width = usize::from(width);
        self.heights = self
            .items
            .iter()
            .map(|item| Self::item_height(item, width, self.diff_mode))
            .collect();
        self.content_height = self
            .heights
            .iter()
            .fold(0u16, |total, height| total.saturating_add(*height));
        self.heights_width = u16::try_from(width).unwrap_or(u16::MAX);
        self.dirty = false;
        let max = self.max_offset();
        if self.follow_tail {
            self.first_visible = max;
            return;
        }
        if self.first_visible == u16::MAX || self.first_visible > max {
            self.first_visible = max;
        }
    }

    fn max_offset(&self) -> u16 {
        self.content_height.saturating_sub(self.view_height)
    }

    /// Flip the expansion of the reasoning block at `row`, when that row is one.
    pub(crate) fn toggle_reasoning_at(&mut self, row: u16) -> bool {
        self.compute_heights(self.heights_width.max(1));
        let mut top = 0u16;
        for (index, height) in self.heights.iter().enumerate() {
            let end = top.saturating_add(*height);
            if row >= top && row < end {
                if let Some(Item::Reasoning { expanded, .. }) = self.items.get_mut(index) {
                    *expanded = !*expanded;
                    self.dirty = true;
                    return true;
                }
                return false;
            }
            top = end;
        }
        false
    }

    /// Paint one tool row with its diff or its live console underneath.
    fn paint_tool_item(
        item: &mut Item,
        rect: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        clip: u16,
        diff_mode: crate::diff::DiffMode,
        theme: &Theme,
    ) {
        let Item::Tool {
            name,
            target,
            state,
            meta,
            diff,
            console,
            ..
        } = item
        else {
            return;
        };
        paint_tool(
            buf,
            rect,
            ToolRow {
                name,
                target: target.as_deref(),
                state: *state,
                meta: meta.as_deref(),
            },
            clip,
            theme,
        );
        if let Some(diff) = diff {
            let box_rect = Rect {
                y: rect.y + 1,
                height: rect.height.saturating_sub(1),
                ..rect
            };
            paint_diff(
                buf,
                box_rect,
                diff,
                diff_mode,
                target.as_deref().unwrap_or("edit"),
                clip,
                theme,
            );
        }
        if let Some(console) = console {
            let rows = console.measure().map_or(2, |(_, height)| height);
            let taken = u16::try_from(diff.as_ref().map_or(0, |diff| diff.rows(diff_mode) + 2))
                .unwrap_or(0);
            let y = rect.y.saturating_add(1).saturating_add(taken);
            if y < clip {
                console.render(
                    Rect {
                        y,
                        height: rows.min(clip.saturating_sub(y)),
                        ..rect
                    },
                    buf,
                    cx,
                    theme,
                );
            }
        }
    }

    /// Paint the selection over the text cells only.
    ///
    /// Rows with no text or with system content stay untouched, and the
    /// highlight never reaches past the trailing spaces of a line.
    fn paint_selection(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let Some(selection) = self.selection else {
            return;
        };
        let Some((start, end)) = Self::ordered_selection(selection) else {
            return;
        };
        for offset in 0..=(end.row - start.row) {
            let row = start.row + offset;
            let row_index = usize::from(row);
            if !self
                .painted_copyable
                .get(usize::from(row))
                .copied()
                .unwrap_or(true)
            {
                continue;
            }
            let Some(line) = self.painted.get(row_index) else {
                continue;
            };
            let text_end = line.trim_end_matches(' ').chars().count();
            if text_end == 0 {
                continue;
            }
            let from = if row == start.row {
                usize::from(start.col)
            } else {
                0
            };
            let from = if self.painted_user.get(row_index).copied().unwrap_or(false) {
                from.max(2)
            } else {
                from
            };
            let to = if row == end.row {
                usize::from(end.col).saturating_add(1).min(text_end)
            } else {
                text_end
            };
            if from >= to {
                continue;
            }
            let y = area.y + row;
            for x in from..to {
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(
                    area.x + u16::try_from(x).unwrap_or(u16::MAX),
                    y,
                )) {
                    cell.set_bg(theme.accent);
                    cell.set_fg(theme.background);
                }
            }
        }
    }

    /// Which item produced each visible row, if any.
    fn row_items(&self) -> Vec<Option<usize>> {
        let mut map = vec![None; usize::from(self.area.height)];
        if self.heights.len() != self.items.len() {
            return map;
        }
        let mut top = 0u16;
        let visible_start = self.first_visible;
        let visible_end = visible_start.saturating_add(self.area.height);
        for (index, height) in self.heights.iter().enumerate() {
            let end = top.saturating_add(*height);
            let start = top.max(visible_start);
            let stop = end.min(visible_end);
            for row in start..stop {
                map[usize::from(row - visible_start)] = Some(index);
            }
            top = end;
        }
        map
    }

    /// Remember the painted lines for clipboard extraction.
    fn capture_painted(&mut self, area: Rect, buf: &Buffer) {
        let row_items = self.row_items();
        self.painted.clear();
        self.painted_copyable.clear();
        for y in area.y..area.y + area.height {
            let row = usize::from(y - area.y);
            let mut line = String::with_capacity(area.width as usize);
            for x in 0..area.width {
                if let Some(cell) = buf.cell(ratatui::layout::Position::new(area.x + x, y)) {
                    line.push_str(cell.symbol());
                }
            }
            let copyable = row_items.get(row).copied().flatten().is_some_and(|index| {
                matches!(
                    self.items.get(index),
                    Some(
                        Item::User { .. }
                            | Item::Assistant { .. }
                            | Item::Reasoning { .. }
                            | Item::Tool { .. }
                    )
                )
            });
            let user = row_items
                .get(row)
                .copied()
                .flatten()
                .is_some_and(|index| matches!(self.items.get(index), Some(Item::User { .. })));
            self.painted.push(line);
            self.painted_copyable.push(copyable);
            self.painted_user.push(user);
        }
    }

    /// The selected range in row/col, ordered.
    fn ordered_selection(selection: Selection) -> Option<(Pos, Pos)> {
        if selection.anchor == selection.cur {
            return None;
        }
        Some(if selection.anchor <= selection.cur {
            (selection.anchor, selection.cur)
        } else {
            (selection.cur, selection.anchor)
        })
    }

    /// The selected text, rows joined by newlines.
    fn selection_text(&self, selection: Selection) -> Option<String> {
        let (start, end) = Self::ordered_selection(selection)?;
        let mut out = String::new();
        for row in start.row..=end.row {
            let row_index = usize::from(row);
            if !self
                .painted_copyable
                .get(usize::from(row))
                .copied()
                .unwrap_or(true)
            {
                continue;
            }
            let Some(line) = self.painted.get(row_index) else {
                continue;
            };
            let chars: Vec<char> = line.chars().collect();
            let from = if row == start.row {
                usize::from(start.col)
            } else {
                0
            };
            let from = if self.painted_user.get(row_index).copied().unwrap_or(false) {
                from.max(2)
            } else {
                from
            };
            let to = if row == end.row {
                usize::from(end.col).saturating_add(1).min(chars.len())
            } else {
                chars.len()
            };
            if from < to {
                let slice: String = chars[from..to].iter().collect();
                out.push_str(slice.trim_end());
            }
            if row != end.row {
                out.push('\n');
            }
        }
        if out.trim().is_empty() {
            return None;
        }
        Some(out.trim_matches('\n').to_string())
    }

    /// Mouse: scroll, select text to copy on release, toggle thoughts.
    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Outcome {
        use crossterm::event::{MouseButton, MouseEventKind};

        let inside = mouse.row >= self.area.y
            && mouse.row < self.area.y + self.area.height
            && mouse.column >= self.area.x
            && mouse.column < self.area.x + self.area.width;
        let pos = Pos {
            row: mouse.row.saturating_sub(self.area.y),
            col: mouse.column.saturating_sub(self.area.x),
        };
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll(-3);
                Outcome::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.scroll(3);
                Outcome::Consumed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if !inside {
                    return Outcome::Ignored;
                }
                let row = mouse
                    .row
                    .saturating_sub(self.origin_y)
                    .saturating_add(self.first_visible);
                let _ = self.toggle_reasoning_at(row);
                self.selection = Some(Selection {
                    anchor: pos,
                    cur: pos,
                });
                Outcome::Consumed
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(selection) = &mut self.selection else {
                    return Outcome::Ignored;
                };
                selection.cur = pos;
                Outcome::Consumed
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(selection) = self.selection.take()
                    && selection.anchor != selection.cur
                {
                    match self.selection_text(selection) {
                        Some(text) => {
                            let chars = text.chars().count();
                            let _ = terminalui::osc52_write(&text);
                            self.push(Item::Notice {
                                text: format!("copied {chars} chars to clipboard"),
                                tone: Tone::Neutral,
                            });
                        }
                        None => {
                            self.push(Item::Notice {
                                text: "selection produced no text".to_string(),
                                tone: Tone::Warn,
                            });
                        }
                    }
                }
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }

    /// Scroll the stream by `delta` rows; negative scrolls toward the top.
    pub(crate) fn scroll(&mut self, delta: isize) {
        let max = i64::from(self.max_offset());
        let next = i64::from(self.first_visible) + i64::try_from(delta).unwrap_or(0);
        self.first_visible = u16::try_from(next.clamp(0, max)).unwrap_or(u16::MAX);
        self.follow_tail = self.first_visible >= self.max_offset();
    }
}

impl View for Transcript {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "transcript"
    }

    fn children(&self) -> &[terminalui::Node] {
        &[]
    }

    fn children_mut(&mut self) -> &mut [terminalui::Node] {
        &mut []
    }

    fn tick(&mut self, delta: Duration, cx: &mut Context) {
        if let Some(thinking) = self.thinking {
            self.thinking = Some(thinking.saturating_add(delta));
        }
        for item in &mut self.items {
            if let Item::Tool {
                console: Some(console),
                ..
            } = item
            {
                console.tick(delta, cx);
            }
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let _ = cx.is_focused();
        self.view_height = area.height;
        self.origin_y = area.y;
        self.area = area;
        let content = Rect {
            x: area.x + 1,
            width: area.width.saturating_sub(1),
            ..area
        };
        self.compute_heights(content.width);

        let bottom = area.y + area.height;
        let skip = self.first_visible;
        let mut row: u16 = 0;

        for (index, item) in self.items.iter_mut().enumerate() {
            let height = self.heights[index];
            let top = row;
            row += height;
            if top + height <= skip || top >= skip + area.height {
                continue;
            }
            let rect_y = area.y + top.saturating_sub(skip);
            let rect = Rect {
                y: rect_y,
                height,
                ..content
            };
            let clip = bottom;
            match item {
                Item::User { text } => paint_user(buf, rect, text, clip, theme),
                Item::Assistant { text } => paint_assistant(buf, rect, text, clip, theme),
                Item::Tool { .. } => {
                    Self::paint_tool_item(item, rect, buf, cx, clip, self.diff_mode, theme);
                }
                Item::Notice { text, tone } | Item::Retry { text, tone } => {
                    paint_notice(buf, rect, text, *tone, clip, theme);
                }
                Item::Reasoning { text, expanded } => {
                    paint_reasoning(buf, rect, text, *expanded, clip, theme);
                }
            }
        }

        if let Some(thinking) = self.thinking {
            paint_thinking(buf, content, thinking, theme);
        }

        if self.content_height > area.height {
            let scroll_style = ScrollbarStyle {
                track_color: Some(theme.background),
                ..ScrollbarStyle::default()
            };
            let scroll = Scroll {
                offset: usize::from(self.first_visible),
                viewport: usize::from(area.height),
                total: usize::from(self.content_height),
            };
            render_scrollbar(buf, area, &scroll_style, theme, &scroll);
        }

        self.capture_painted(area, buf);
        self.paint_selection(area, buf, theme);
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        let focused = cx.is_focused();
        match event {
            InputEvent::Key(key) => {
                if key.kind != KeyEventKind::Press || !focused {
                    return Outcome::Ignored;
                }
                let half = isize::try_from((self.view_height / 2).max(1)).unwrap_or(1);
                match key.code {
                    KeyCode::Up => {
                        self.scroll(-1);
                        Outcome::Consumed
                    }
                    KeyCode::Down => {
                        self.scroll(1);
                        Outcome::Consumed
                    }
                    KeyCode::PageUp => {
                        self.scroll(-half);
                        Outcome::Consumed
                    }
                    KeyCode::PageDown => {
                        self.scroll(half);
                        Outcome::Consumed
                    }
                    KeyCode::Home => {
                        self.first_visible = 0;
                        Outcome::Consumed
                    }
                    KeyCode::End => {
                        self.first_visible = self.max_offset();
                        self.follow_tail = true;
                        Outcome::Consumed
                    }
                    _ => Outcome::Ignored,
                }
            }
            InputEvent::Mouse(mouse) => self.handle_mouse(*mouse),
            _ => Outcome::Ignored,
        }
    }

    fn focusable(&self) -> bool {
        true
    }
}

fn paint_user(buf: &mut Buffer, area: Rect, text: &str, clip: u16, theme: &Theme) {
    let lines = wrap(text, usize::from(area.width));
    for (row, line) in lines.iter().enumerate() {
        let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
        if y >= clip {
            break;
        }
        buf.set_stringn(
            area.x,
            y,
            "▌",
            1,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        buf.set_stringn(
            area.x + 2,
            y,
            line,
            line.chars().count(),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        );
    }
}

fn paint_assistant(buf: &mut Buffer, area: Rect, text: &str, clip: u16, theme: &Theme) {
    let lines = wrap(text, usize::from(area.width));
    for (row, line) in lines.iter().enumerate() {
        let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
        if y >= clip {
            break;
        }
        paint_inline_code(buf, area.x, y, line, theme);
    }
}

fn paint_inline_code(buf: &mut Buffer, x: u16, y: u16, line: &str, theme: &Theme) {
    let normal = Style::default().fg(theme.foreground);
    let code = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let mut in_code = false;
    let mut col = x;
    for segment in line.split('`') {
        if segment.is_empty() {
            in_code = !in_code;
            continue;
        }
        let style = if in_code { code } else { normal };
        buf.set_stringn(col, y, segment, segment.chars().count(), style);
        col += u16::try_from(display_width(segment)).unwrap_or(u16::MAX);
        in_code = !in_code;
    }
}

#[derive(Clone, Copy)]
struct ToolRow<'a> {
    name: &'a str,
    target: Option<&'a str>,
    state: ToolState,
    meta: Option<&'a str>,
}

fn paint_tool(buf: &mut Buffer, area: Rect, row: ToolRow<'_>, clip: u16, theme: &Theme) {
    let y = area.y;
    if y >= clip {
        return;
    }
    let ToolRow {
        name,
        target,
        state,
        meta,
    } = row;
    let (mark, mark_color) = match state {
        ToolState::Done => ("✓", theme.success),
        ToolState::Running => ("◌", theme.accent),
    };
    buf.set_stringn(area.x, y, mark, 1, Style::default().fg(mark_color));
    buf.set_stringn(
        area.x + 2,
        y,
        name,
        name.chars().count(),
        Style::default()
            .fg(theme.foreground)
            .add_modifier(Modifier::BOLD),
    );
    let mut col = area.x + 2 + u16::try_from(name.chars().count()).unwrap_or(u16::MAX) + 2;
    if let Some(target) = target {
        buf.set_stringn(
            col,
            y,
            target,
            target.chars().count(),
            Style::default().fg(theme.muted),
        );
        col += u16::try_from(target.chars().count()).unwrap_or(u16::MAX) + 2;
    }
    if let Some(meta) = meta {
        buf.set_stringn(
            col,
            y,
            meta,
            meta.chars().count(),
            Style::default().fg(theme.success),
        );
    }
}

fn paint_diff(
    buf: &mut Buffer,
    area: Rect,
    diff: &crate::diff::Diff,
    mode: crate::diff::DiffMode,
    path: &str,
    clip: u16,
    theme: &Theme,
) {
    if area.height < 3 || area.width < 4 || area.y >= clip {
        return;
    }
    paint_diff_frame(buf, area, diff, path, clip, theme);
    let inner_x = area.x + 2;
    let inner_w = area.width.saturating_sub(4);
    match mode {
        crate::diff::DiffMode::Unified => {
            paint_diff_unified(buf, area, diff, inner_x, inner_w, clip, theme);
        }
        crate::diff::DiffMode::Split => {
            paint_diff_split(buf, area, diff, inner_x, inner_w, clip, theme);
        }
    }
}

fn paint_diff_frame(
    buf: &mut Buffer,
    area: Rect,
    diff: &crate::diff::Diff,
    path: &str,
    clip: u16,
    theme: &Theme,
) {
    let (removed, added) = diff.counts();
    let title = format!(" {path}  −{removed} +{added} ");
    let top = format!("╭─{title}");
    let head_len = u16::try_from(top.chars().count())
        .unwrap_or(u16::MAX)
        .saturating_add(1);
    let top = format!(
        "{top}{}╮",
        "─".repeat(usize::from(area.width.saturating_sub(head_len)))
    );
    buf.set_stringn(
        area.x,
        area.y,
        &top,
        usize::from(area.width),
        Style::default().fg(theme.border),
    );
    for row in 1..area.height - 1 {
        let y = area.y + row;
        if y < clip {
            buf.set_stringn(area.x, y, "│", 1, Style::default().fg(theme.border));
            buf.set_stringn(
                area.right() - 1,
                y,
                "│",
                1,
                Style::default().fg(theme.border),
            );
        }
    }
    if area.y + area.height - 1 < clip {
        let bottom = format!(
            "╰{}╯",
            "─".repeat(usize::from(area.width.saturating_sub(2)))
        );
        buf.set_stringn(
            area.x,
            area.y + area.height - 1,
            &bottom,
            usize::from(area.width),
            Style::default().fg(theme.border),
        );
    }
}

fn paint_diff_unified(
    buf: &mut Buffer,
    area: Rect,
    diff: &crate::diff::Diff,
    x: u16,
    width: u16,
    clip: u16,
    theme: &Theme,
) {
    for (offset, line) in diff.lines.iter().enumerate() {
        let y = area.y + 1 + u16::try_from(offset).unwrap_or(u16::MAX);
        if y >= area.y + area.height - 1 || y >= clip {
            return;
        }
        let (sign, color) = sign_and_color(line.kind, theme);
        paint_diff_line(
            buf,
            x,
            y,
            width,
            sign,
            &line.text,
            Style::default().fg(color).bg(theme.secondary),
        );
    }
}

fn paint_diff_split(
    buf: &mut Buffer,
    area: Rect,
    diff: &crate::diff::Diff,
    x: u16,
    width: u16,
    clip: u16,
    theme: &Theme,
) {
    let mut y = area.y + 1;
    let mut index = 0usize;
    let half = width.saturating_sub(1) / 2;
    while index < diff.lines.len() && y < area.y + area.height - 1 && y < clip {
        if diff.lines[index].kind == LineKind::Context {
            let line = &diff.lines[index];
            paint_diff_line(
                buf,
                x,
                y,
                width,
                " ",
                &line.text,
                Style::default().fg(theme.muted).bg(theme.secondary),
            );
            y += 1;
            index += 1;
            continue;
        }
        let start = index;
        while index < diff.lines.len() && diff.lines[index].kind != LineKind::Context {
            index += 1;
        }
        let removed: Vec<_> = diff.lines[start..index]
            .iter()
            .filter(|line| line.kind == LineKind::Removed)
            .collect();
        let added: Vec<_> = diff.lines[start..index]
            .iter()
            .filter(|line| line.kind == LineKind::Added)
            .collect();
        for pair in 0..removed.len().max(added.len()) {
            if y >= area.y + area.height - 1 || y >= clip {
                return;
            }
            if let Some(line) = removed.get(pair) {
                paint_diff_line(
                    buf,
                    x,
                    y,
                    half,
                    "−",
                    &line.text,
                    Style::default().fg(theme.destructive).bg(theme.secondary),
                );
            }
            if let Some(line) = added.get(pair) {
                paint_diff_line(
                    buf,
                    x + half + 1,
                    y,
                    width.saturating_sub(half + 1),
                    "+",
                    &line.text,
                    Style::default().fg(theme.success).bg(theme.secondary),
                );
            }
            y += 1;
        }
    }
}

fn sign_and_color(kind: LineKind, theme: &Theme) -> (&'static str, ratatui::style::Color) {
    match kind {
        LineKind::Context => (" ", theme.muted),
        LineKind::Removed => ("−", theme.destructive),
        LineKind::Added => ("+", theme.success),
    }
}

fn paint_diff_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    sign: &str,
    line: &str,
    style: Style,
) {
    buf.set_stringn(x, y, sign, 1, style);
    buf.set_stringn(x + 2, y, line, usize::from(width.saturating_sub(2)), style);
}

fn paint_notice(buf: &mut Buffer, area: Rect, text: &str, tone: Tone, clip: u16, theme: &Theme) {
    let (glyph, color) = match tone {
        Tone::Neutral => ("·", theme.muted),
        Tone::Warn => ("!", theme.warning),
        Tone::Error => ("✗", theme.destructive),
    };
    let budget = usize::from(area.width.saturating_sub(2));
    let prefix_len = format!("{glyph} notice · ").chars().count();
    let text_budget = budget.saturating_sub(prefix_len).max(1);
    let lines = wrap(text, text_budget);
    for (row, line) in lines.iter().enumerate() {
        let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
        if y >= clip {
            break;
        }
        let rendered = if row == 0 {
            format!("{glyph} notice · {line}")
        } else {
            format!("  {line}")
        };
        buf.set_stringn(
            area.x + 2,
            y,
            &rendered,
            budget,
            Style::default().fg(color).add_modifier(Modifier::ITALIC),
        );
    }
}

fn paint_reasoning(
    buf: &mut Buffer,
    area: Rect,
    text: &str,
    expanded: bool,
    clip: u16,
    theme: &Theme,
) {
    if !expanded {
        if area.y >= clip {
            return;
        }
        let line_count = wrap(text, usize::from(area.width)).len();
        let first_line: String = text
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>()
            .join(" ");
        let budget = usize::from(area.width.saturating_sub(2));
        let label = format!("▸ ◌ thought · {line_count} lines · {first_line}");
        buf.set_stringn(
            area.x + 2,
            area.y,
            &label,
            budget,
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC),
        );
        return;
    }
    let header = "▾ ◌ thought";
    buf.set_stringn(
        area.x + 2,
        area.y,
        header,
        header.chars().count(),
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::ITALIC),
    );
    let lines = wrap(text, usize::from(area.width));
    for (row, line) in lines.iter().enumerate() {
        let y = area.y + 1 + u16::try_from(row).unwrap_or(u16::MAX);
        if y >= clip {
            break;
        }
        buf.set_stringn(
            area.x + 2,
            y,
            line,
            line.chars().count(),
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC),
        );
    }
}

fn paint_thinking(buf: &mut Buffer, area: Rect, elapsed: Duration, theme: &Theme) {
    let y = area.y + area.height.saturating_sub(1);
    let secs = elapsed.as_secs();
    let label = format!("thinking {secs}s");
    let frame = SPINNER_FRAMES
        [usize::try_from(elapsed.as_millis() / 80).unwrap_or(0) % SPINNER_FRAMES.len()];
    buf.set_stringn(area.x, y, frame, 1, Style::default().fg(theme.accent));
    let sweep_span = u128::try_from(label.chars().count()).unwrap_or(0) + 12;
    let sweep = i64::try_from((elapsed.as_millis() / 90) % sweep_span).unwrap_or(0) - 6;
    for (index, ch) in label.chars().enumerate() {
        let distance = (i64::try_from(index).unwrap_or(0) - sweep).abs();
        let style = if distance <= 3 {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        buf.set_stringn(
            area.x + 2 + u16::try_from(index).unwrap_or(u16::MAX),
            y,
            ch.to_string(),
            1,
            style,
        );
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
