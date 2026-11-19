//! The conversation stream.
//!
//! Items arrive from the host and are painted bottom-pinned in a windowed
//! view: only visible rows render, and per-item heights are cached instead
//! of measured every frame. While an agent turn is in flight a thinking row
//! is appended after the newest content.

use std::any::Any;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEventKind, MouseEventKind};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
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
    },
    /// A unified diff with red removal rows and green addition rows.
    Diff {
        /// Lines removed.
        removed: Vec<String>,
        /// Lines added.
        added: Vec<String>,
    },
    /// A shell session, rendered with the library `Console` view.
    Shell {
        /// The call id the host names; used to match output frames to it.
        call_id: String,
        /// The console block itself.
        console: Console,
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
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let needed = if current.is_empty() {
            word.len()
        } else {
            current.len() + 1 + word.len()
        };
        if needed > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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
        }
    }

    /// Replace the stream with a history page; scroll pins to the bottom.
    pub(crate) fn replace_history(&mut self, items: Vec<norte_code_wire::HistoryItem>) {
        self.items.clear();
        self.thinking = None;
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
                "system_event" | "tool" | "tool_call" | "tool_result" => {
                    let call_id = item.id.clone();
                    let summary = item
                        .content
                        .lines()
                        .next()
                        .unwrap_or("tool")
                        .trim()
                        .to_owned();
                    self.items.push(Item::Tool {
                        call_id,
                        name: if summary.len() > 40 {
                            let clipped: String = summary.chars().take(39).collect();
                            format!("{clipped}…")
                        } else {
                            summary
                        },
                        target: None,
                        state: ToolState::Done,
                        meta: None,
                    });
                }
                _ => {}
            }
        }
        self.dirty = true;
        self.first_visible = u16::MAX;
    }

    /// Append an item; the view stays pinned to the bottom.
    pub(crate) fn push(&mut self, item: Item) {
        self.items.push(item);
        self.dirty = true;
        self.first_visible = u16::MAX;
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
            if let Item::Shell {
                call_id: id,
                console,
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

    /// Append reasoning text; merges into the trailing reasoning block.
    pub(crate) fn push_reasoning_text(&mut self, text: &str) {
        if let Some(Item::Reasoning { text: existing, .. }) = self.items.last_mut() {
            existing.push_str(text);
            self.dirty = true;
            return;
        }
        self.push(Item::Reasoning {
            text: text.to_string(),
            expanded: false,
        });
    }

    fn item_height(item: &Item, width: usize) -> u16 {
        match item {
            Item::User { text } | Item::Assistant { text } => {
                u16::try_from(wrap(text, width).len()).unwrap_or(u16::MAX) + BLOCK_GAP
            }
            Item::Reasoning { text, expanded } => {
                if *expanded {
                    u16::try_from(wrap(text, width).len() + 1).unwrap_or(u16::MAX) + BLOCK_GAP
                } else {
                    1 + BLOCK_GAP
                }
            }
            Item::Tool { .. } | Item::Notice { .. } | Item::Retry { .. } => 2,
            Item::Diff { removed, added } => {
                u16::try_from(removed.len() + added.len() + 1).unwrap_or(u16::MAX)
            }
            Item::Shell { console, .. } => {
                console.measure().map_or(2, |(_, height)| height) + BLOCK_GAP
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
            .map(|item| Self::item_height(item, width))
            .collect();
        self.content_height = self.heights.iter().sum();
        self.heights_width = u16::try_from(width).unwrap_or(u16::MAX);
        self.dirty = false;
        let max = self.max_offset();
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

    /// Scroll the stream by `delta` rows; negative scrolls toward the top.
    pub(crate) fn scroll(&mut self, delta: isize) {
        let max = i64::from(self.max_offset());
        let next = i64::from(self.first_visible) + i64::try_from(delta).unwrap_or(0);
        self.first_visible = u16::try_from(next.clamp(0, max)).unwrap_or(u16::MAX);
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
            if let Item::Shell { console, .. } = item {
                console.tick(delta, cx);
            }
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let _ = cx.is_focused();
        self.view_height = area.height;
        self.origin_y = area.y;
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
                Item::Tool {
                    call_id: _,
                    name,
                    target,
                    state,
                    meta,
                } => paint_tool(
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
                ),
                Item::Diff { removed, added } => paint_diff(buf, rect, removed, added, clip, theme),
                Item::Shell { console, .. } => {
                    let rows = console.measure().map_or(2, |(_, height)| height);
                    let shell_rect = Rect {
                        height: rows,
                        ..rect
                    };
                    console.render(shell_rect, buf, cx, theme);
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
                        Outcome::Consumed
                    }
                    _ => Outcome::Ignored,
                }
            }
            InputEvent::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.scroll(-3);
                    Outcome::Consumed
                }
                MouseEventKind::ScrollDown => {
                    self.scroll(3);
                    Outcome::Consumed
                }
                MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                    let row = mouse
                        .row
                        .saturating_sub(self.origin_y)
                        .saturating_add(self.first_visible);
                    if self.toggle_reasoning_at(row) {
                        Outcome::Consumed
                    } else {
                        Outcome::Ignored
                    }
                }
                _ => Outcome::Ignored,
            },
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
        col += u16::try_from(segment.chars().count()).unwrap_or(u16::MAX);
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
    removed: &[String],
    added: &[String],
    clip: u16,
    theme: &Theme,
) {
    let surface = theme.secondary;
    let mut y = area.y;
    for line in removed {
        if y >= clip {
            return;
        }
        buf.set_style(Rect { height: 1, ..area }, Style::default().bg(surface));
        buf.set_stringn(
            area.x,
            y,
            "−",
            1,
            Style::default().fg(theme.destructive).bg(surface),
        );
        buf.set_stringn(
            area.x + 2,
            y,
            line,
            line.chars().count(),
            Style::default().fg(theme.destructive).bg(surface),
        );
        y += 1;
    }
    for line in added {
        if y >= clip {
            return;
        }
        buf.set_style(Rect { height: 1, ..area }, Style::default().bg(surface));
        buf.set_stringn(
            area.x,
            y,
            "+",
            1,
            Style::default().fg(theme.success).bg(surface),
        );
        buf.set_stringn(
            area.x + 2,
            y,
            line,
            line.chars().count(),
            Style::default().fg(theme.success).bg(surface),
        );
        y += 1;
    }
}

fn paint_notice(buf: &mut Buffer, area: Rect, text: &str, tone: Tone, clip: u16, theme: &Theme) {
    let y = area.y;
    if y >= clip {
        return;
    }
    let (glyph, color) = match tone {
        Tone::Neutral => ("·", theme.muted),
        Tone::Warn => ("!", theme.warning),
        Tone::Error => ("✗", theme.destructive),
    };
    let line = format!("{glyph} notice · {text}");
    buf.set_stringn(
        area.x + 2,
        y,
        &line,
        line.chars().count(),
        Style::default().fg(color).add_modifier(Modifier::ITALIC),
    );
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
