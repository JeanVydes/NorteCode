//! The conversation stream.
//!
//! Items arrive from the host and are painted bottom-pinned in a windowed
//! view: only visible rows render, and per-item heights are cached instead
//! of measured every frame. While an agent turn is in flight a thinking row
//! is appended after the newest content.

use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEventKind};
use norte_code_client::config::RetentionConfig;

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::scroll_drag::Scrollbar;
use crate::diff::LineKind;
use serde::Serialize;

use terminalui::components::console::Console;
use terminalui::components::scroll::{Scroll, ScrollbarStyle, render_scrollbar};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BLOCK_GAP: u16 = 1;
const NOTICE_PREFIX_CHARS: usize = 11;

#[derive(Clone, Copy)]
struct ItemPaint<'a> {
    clip: u16,
    diff_mode: crate::diff::DiffMode,
    focused: bool,
    streaming: Option<Duration>,
    thinking: Option<Duration>,
    doc: Option<&'a super::markdown::Markdown>,
    theme: &'a Theme,
}

/// Execution state of a tool block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub(crate) enum ToolState {
    /// Completed successfully.
    Done,
    /// In flight right now.
    Running,
    /// The tool ended with an error.
    Failed,
}

/// The activity summarized by an expandable group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActivityStatus {
    Thinking,
    Tool {
        name: String,
        context: Option<String>,
        meta: Option<String>,
    },
    Complete,
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
        /// The turn this message belongs to, when known.
        turn: Option<u64>,
    },
    /// The assistant answered; inline code draws as accented spans.
    Assistant {
        /// The raw text.
        text: String,
        /// The turn this message belongs to, when known.
        turn: Option<u64>,
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
        /// Whether the tool's detailed output is shown.
        expanded: bool,
        /// Optional meta column, e.g. `+41 −12`.
        meta: Option<String>,
        /// The change an edit made, drawn under its header.
        diff: Option<crate::diff::Diff>,
        /// Final text returned by the tool.
        output: Option<String>,
        /// Error returned by the tool.
        error: Option<String>,
        /// A shell call's live output, under its header.
        console: Option<Box<Console>>,
    },
    /// A contiguous run of reasoning and tool activity.
    Activity {
        /// Ordered activity items.
        entries: Vec<Item>,
        /// Whether the group header is visible.
        grouped: bool,
        /// Whether the group's entries are visible.
        expanded: bool,
        /// The live status displayed while the group is active.
        status: ActivityStatus,
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

pub(crate) fn parse_turn(turn_id: Option<&str>) -> Option<u64> {
    turn_id.and_then(|id| id.strip_prefix("turn_").unwrap_or(id).parse().ok())
}

fn provenance_label(provenance: Option<&serde_json::Value>) -> Option<String> {
    let plugin_id = provenance?.get("plugin_id")?.as_str()?;
    Some(format!("from plugin {plugin_id}"))
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
    retention: RetentionConfig,
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
    #[serde(skip)]
    streaming: Option<Duration>,
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
    painted: Vec<Vec<String>>,
    /// Whether the row's source item may be copied or highlighted.
    #[serde(skip)]
    painted_copyable: Vec<bool>,
    /// Whether a row carries the user message's leading bar.
    #[serde(skip)]
    painted_user: Vec<bool>,
    /// Which item owns each visible row, cached by the last render.
    #[serde(skip)]
    row_map: Vec<Option<usize>>,
    /// The scrollbar drawn in the last frame, when the content overflows.
    #[serde(skip)]
    scrollbar: Option<Scrollbar>,
    /// Whether the user is dragging the scrollbar thumb.
    #[serde(skip)]
    dragging_thumb: bool,
    /// The open message menu, if any.
    #[serde(skip)]
    menu: Option<MessageMenu>,
    /// The message the user last clicked, highlighted in the stream.
    #[serde(skip)]
    focused_item: Option<usize>,
    /// Message actions waiting for the host.
    #[serde(skip)]
    message_actions: Vec<MessageAction>,
    /// Toast requests waiting for the host.
    #[serde(skip)]
    toasts: Vec<ToastRequest>,
    /// How edit diffs are drawn.
    #[serde(skip)]
    diff_mode: crate::diff::DiffMode,
    #[serde(skip)]
    tool_preferences: ToolPreferences,
    /// Rendered Markdown for message items, indexed like `items`.
    #[serde(skip)]
    docs: Vec<Option<super::markdown::Markdown>>,
    /// The palette the cached documents were rendered with.
    #[serde(skip)]
    doc_theme: Option<Theme>,
    /// The one clipboard all copy paths write to.
    #[serde(skip)]
    clipboard: Arc<dyn crate::clipboard::Clipboard>,
}

#[derive(Default)]
struct ToolPreferences {
    output_expanded: bool,
    grouped_activity: bool,
}

/// What the message menu was asked to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MessageAction {
    /// Fork the session at this message's turn.
    Fork { turn: Option<u64> },
    /// Rewind the session to this message's turn.
    Rewind { turn: Option<u64> },
}

/// A transient message the view asks the host to display.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ToastRequest {
    pub(crate) text: String,
    pub(crate) tone: Tone,
}

/// An open message menu.
#[derive(Clone, Copy, Debug)]
struct MessageMenu {
    /// The item the menu belongs to.
    item: usize,
    /// Screen column where the click happened.
    column: u16,
    /// Screen row where the click happened.
    row: u16,
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
    /// Creates a bottom-pinned empty stream with default retention.
    pub(crate) fn new() -> Self {
        Self::with_retention(RetentionConfig::default())
    }

    /// Creates a bottom-pinned empty stream with the host's retention limits.
    pub(crate) fn with_retention(retention: RetentionConfig) -> Self {
        Self {
            items: Vec::new(),
            retention,
            first_visible: u16::MAX,
            view_height: 0,
            heights: Vec::new(),
            content_height: 0,
            heights_width: 0,
            dirty: true,
            thinking: None,
            streaming: None,
            origin_y: 0,
            area: Rect::new(0, 0, 0, 0),
            selection: None,
            painted: Vec::new(),
            painted_copyable: Vec::new(),
            painted_user: Vec::new(),
            row_map: Vec::new(),
            scrollbar: None,
            dragging_thumb: false,
            menu: None,
            focused_item: None,
            message_actions: Vec::new(),
            toasts: Vec::new(),
            diff_mode: crate::diff::DiffMode::Unified,
            tool_preferences: ToolPreferences::default(),
            docs: Vec::new(),
            doc_theme: None,
            follow_tail: true,
            clipboard: crate::clipboard::system(),
        }
    }

    /// Use a test clipboard instead of the platform one.
    #[cfg(test)]
    pub(crate) fn set_clipboard(&mut self, clipboard: Arc<dyn crate::clipboard::Clipboard>) {
        self.clipboard = clipboard;
    }

    /// Set the diff layout; a change rebuilds the cached row heights.
    pub(crate) fn set_diff_mode(&mut self, mode: crate::diff::DiffMode) {
        if self.diff_mode != mode {
            self.diff_mode = mode;
            self.dirty = true;
        }
    }

    /// Set the default expansion and activity grouping preferences.
    pub(crate) fn set_tool_preferences(&mut self, show_output: bool, verbosity: bool) {
        if self.tool_preferences.output_expanded == show_output
            && self.tool_preferences.grouped_activity == verbosity
        {
            return;
        }
        let verbosity_changed = self.tool_preferences.grouped_activity != verbosity;
        self.tool_preferences = ToolPreferences {
            output_expanded: show_output,
            grouped_activity: verbosity,
        };
        if verbosity_changed {
            let items = std::mem::take(&mut self.items);
            self.items = if verbosity {
                group_activity_items(items)
            } else {
                flatten_activity_items(items)
            };
            if verbosity && self.thinking.is_some() {
                self.ensure_activity(ActivityStatus::Thinking);
            }
        }
        for item in &mut self.items {
            if let Item::Activity {
                expanded, status, ..
            } = item
                && verbosity_changed
            {
                *expanded = matches!(
                    status,
                    ActivityStatus::Thinking | ActivityStatus::Tool { .. }
                );
            }
        }
        self.dirty = true;
    }

    fn reset_render_cache(&mut self) {
        self.heights.clear();
        self.docs.clear();
        self.painted.clear();
        self.painted_copyable.clear();
        self.painted_user.clear();
        self.row_map.clear();
        self.content_height = 0;
        self.heights_width = 0;
        self.doc_theme = None;
        self.dirty = true;
    }

    fn trim_items(&mut self) {
        if self.items.len() < self.retention.transcript_max_items {
            return;
        }
        let skip = self.items.len() - self.retention.transcript_keep_items;
        self.items.drain(..skip);
        self.focused_item = self.focused_item.and_then(|item| item.checked_sub(skip));
        if let Some(menu) = self.menu.as_mut() {
            menu.item = menu.item.saturating_sub(skip);
        }
        self.reset_render_cache();
    }

    /// Forget everything and pin the view to the bottom.
    pub(crate) fn clear_all(&mut self) {
        self.items.clear();
        self.thinking = None;
        self.streaming = None;
        self.reset_render_cache();
        self.first_visible = u16::MAX;
        self.follow_tail = true;
    }

    /// Replace the stream with a history page; scroll pins to the bottom.
    pub(crate) fn replace_history(&mut self, items: Vec<norte_code_wire::HistoryItem>) {
        self.items.clear();
        self.thinking = None;
        self.reset_render_cache();
        let mut pending_calls = VecDeque::<String>::new();
        for item in items {
            self.append_history_item(item, &mut pending_calls);
        }
        self.trim_items();
        self.dirty = true;
        self.first_visible = u16::MAX;
        self.follow_tail = true;
    }

    fn append_history_item(
        &mut self,
        item: norte_code_wire::HistoryItem,
        pending_calls: &mut VecDeque<String>,
    ) {
        match item.kind {
            norte_code_wire::HistoryItemKind::User => {
                let text = match provenance_label(item.provenance.as_ref()) {
                    Some(label) if item.content.is_empty() => label,
                    Some(label) => format!("{}\n{label}", item.content),
                    None => item.content,
                };
                self.push(Item::User {
                    text,
                    turn: parse_turn(item.turn_id.as_deref()),
                });
            }
            norte_code_wire::HistoryItemKind::Assistant => self.push(Item::Assistant {
                text: item.content,
                turn: parse_turn(item.turn_id.as_deref()),
            }),
            norte_code_wire::HistoryItemKind::Reasoning => self.push(Item::Reasoning {
                text: item.content,
                expanded: false,
            }),
            norte_code_wire::HistoryItemKind::Refusal => self.push(Item::Notice {
                text: item.content,
                tone: Tone::Warn,
            }),
            norte_code_wire::HistoryItemKind::ToolResult => {
                if let Some(call_id) = pending_calls.pop_front() {
                    self.set_history_tool_result(&call_id, &item.content);
                }
            }
            norte_code_wire::HistoryItemKind::SystemEvent
            | norte_code_wire::HistoryItemKind::Tool
            | norte_code_wire::HistoryItemKind::ToolCall => {
                let mut lines = item.content.lines();
                let first = lines.next().unwrap_or("tool").trim();
                let has_brace = first.ends_with('{');
                let summary = first.strip_suffix('{').unwrap_or(first).trim().to_owned();
                let rest = lines.collect::<Vec<_>>().join("\n");
                let arguments = if has_brace { format!("{{{rest}") } else { rest };
                let edit = serde_json::from_str::<serde_json::Value>(&arguments)
                    .ok()
                    .and_then(|value| serde_json::from_value::<crate::diff::EditArgs>(value).ok());
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
                    command.map(|command| {
                        Box::new(
                            Console::new(command)
                                .with_max_visible(6)
                                .with_output_preview()
                                .with_output_visible(self.tool_preferences.output_expanded)
                                .with_max_retained_lines(Some(self.retention.console_max_lines)),
                        )
                    })
                } else {
                    None
                };
                let call_id = item.id;
                pending_calls.push_back(call_id.clone());
                self.push(Item::Tool {
                    call_id,
                    name: if summary.len() > 40 {
                        let clipped: String = summary.chars().take(39).collect();
                        format!("{clipped}…")
                    } else {
                        summary
                    },
                    target,
                    state: ToolState::Done,
                    expanded: self.tool_preferences.output_expanded,
                    meta: None,
                    diff,
                    output: None,
                    error: None,
                    console,
                });
            }
        }
    }

    fn set_history_tool_result(&mut self, call_id: &str, output: &str) {
        for_each_tool_mut(&mut self.items, &mut |item| {
            if let Item::Tool {
                call_id: id,
                name,
                output: tool_output,
                console,
                ..
            } = item
                && id == call_id
            {
                *tool_output = (!matches!(name.as_str(), "edit" | "shell" | "bash")
                    && !output.is_empty())
                .then(|| output.to_owned());
                if let Some(console) = console {
                    for line in output.lines() {
                        crate::console_log::push_line(
                            console,
                            line.to_owned(),
                            norte_code_wire::OutputStream::Stdout,
                            self.retention,
                        );
                    }
                    console.set_exit(Some(0));
                }
            }
        });
    }

    /// Append an item; the view stays pinned to the bottom while following the tail.
    pub(crate) fn push(&mut self, mut item: Item) {
        match &mut item {
            Item::Tool {
                name,
                target,
                meta,
                state,
                expanded,
                ..
            } => {
                *expanded = self.tool_preferences.output_expanded;
                let running = *state == ToolState::Running;
                if running {
                    set_activity_status(
                        &mut self.items,
                        ActivityStatus::Tool {
                            name: name.clone(),
                            context: target.clone(),
                            meta: meta.clone(),
                        },
                    );
                    if self.tool_preferences.grouped_activity {
                        self.thinking = Some(Duration::ZERO);
                    }
                }
                if self.tool_preferences.grouped_activity {
                    push_activity_entry(&mut self.items, item, true);
                    self.thinking = if running { Some(Duration::ZERO) } else { None };
                } else {
                    self.close_activity();
                    self.items.push(item);
                }
                self.dirty = true;
                if self.follow_tail {
                    self.first_visible = u16::MAX;
                }
                return;
            }
            Item::Reasoning { .. } => {
                if self.tool_preferences.grouped_activity {
                    set_activity_status(&mut self.items, ActivityStatus::Thinking);
                    push_activity_entry(&mut self.items, item, true);
                    self.thinking = Some(Duration::ZERO);
                } else {
                    self.close_activity();
                    self.items.push(item);
                    self.thinking = None;
                }
                self.dirty = true;
                if self.follow_tail {
                    self.first_visible = u16::MAX;
                }
                return;
            }
            Item::Activity { .. } => {}
            _ => self.close_activity(),
        }
        self.items.push(item);
        self.trim_items();
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
        self.streaming = None;
        if self.tool_preferences.grouped_activity {
            self.thinking = Some(Duration::from_millis(1));
            self.ensure_activity(ActivityStatus::Thinking);
        } else if self.thinking.is_none() {
            self.thinking = Some(Duration::from_millis(1));
        }
    }

    /// End the thinking row.
    pub(crate) fn stop_thinking(&mut self) {
        self.thinking = None;
        self.streaming = None;
    }

    /// Flip every `Running` tool block to `Done`.
    pub(crate) fn complete_running_tools(&mut self) {
        for_each_tool_mut(&mut self.items, &mut |item| {
            if let Item::Tool { state, console, .. } = item
                && *state == ToolState::Running
            {
                *state = ToolState::Done;
                if let Some(console) = console {
                    console.set_exit(None);
                }
            }
        });
        self.dirty = true;
    }

    /// Append streaming output to the console with the given call id.
    pub(crate) fn append_console_output(
        &mut self,
        call_id: &str,
        chunk: &str,
        stream: norte_code_wire::OutputStream,
    ) -> bool {
        let mut appended = false;
        for_each_tool_mut(&mut self.items, &mut |item| {
            if let Item::Tool {
                call_id: id,
                console: Some(console),
                ..
            } = item
                && *id == call_id
            {
                crate::console_log::push_chunk(console, chunk, stream, self.retention);
                appended = true;
            }
        });
        if appended {
            self.dirty = true;
        }
        appended
    }

    /// Flip the `Running` tool block with the given call id to `Done`.
    pub(crate) fn finish_tool(
        &mut self,
        call_id: &str,
        is_error: bool,
        output: Option<&str>,
        error: Option<&str>,
    ) {
        let mut finished = false;
        for_each_tool_mut(&mut self.items, &mut |item| {
            if let Item::Tool {
                call_id: id,
                state,
                console,
                output: tool_output,
                error: tool_error,
                name,
                ..
            } = item
                && *id == call_id
                && *state == ToolState::Running
            {
                *state = if is_error {
                    ToolState::Failed
                } else {
                    ToolState::Done
                };
                if let Some(console) = console {
                    console.set_exit(if is_error { Some(1) } else { Some(0) });
                }
                let output = output.filter(|text| !text.is_empty());
                let duplicate_console_output = matches!(name.as_str(), "shell" | "bash")
                    && console.as_ref().is_some_and(|console| {
                        console.lines().join("\n").trim() == output.unwrap_or_default().trim()
                    });
                *tool_output = if matches!(name.as_str(), "edit") || duplicate_console_output {
                    None
                } else {
                    output.map(str::to_owned)
                };
                *tool_error = error.filter(|text| !text.is_empty()).map(str::to_owned);
                finished = true;
            }
        });
        if finished {
            set_activity_status(&mut self.items, ActivityStatus::Thinking);
            self.thinking = Some(Duration::ZERO);
            self.dirty = true;
        }
    }

    /// Append assistant text; merges into the trailing assistant block.
    pub(crate) fn push_assistant_text(
        &mut self,
        text: &str,
        turn: Option<&crate::ui_model::ActiveTurn>,
    ) {
        self.close_activity();
        if self.streaming.is_none() {
            self.streaming = Some(Duration::ZERO);
        }
        if let Some(Item::Assistant { text: existing, .. }) = self.items.last_mut() {
            existing.push_str(text);
            self.dirty = true;
            return;
        }
        self.push(Item::Assistant {
            text: text.to_string(),
            turn: turn.and_then(|turn| crate::ui::parse_turn(Some(&turn.id))),
        });
    }

    /// Append reasoning text to the open reasoning block.
    ///
    /// Consecutive reasoning deltas merge into one block. Assistant text,
    /// a tool, or a user message closes the block, so the next delta starts
    /// a new one instead of being appended to a distant block from earlier
    /// in the turn.
    pub(crate) fn push_reasoning_text(&mut self, text: &str) {
        let current = if self.tool_preferences.grouped_activity {
            last_activity_entries_mut(&mut self.items).and_then(|entries| entries.last_mut())
        } else {
            self.items.last_mut()
        };
        if let Some(Item::Reasoning { text: existing, .. }) = current {
            existing.push_str(text);
            self.dirty = true;
            return;
        }
        self.push(Item::Reasoning {
            text: text.to_string(),
            expanded: false,
        });
    }

    fn ensure_activity(&mut self, status: ActivityStatus) {
        if let Some(Item::Activity {
            status: current,
            expanded,
            ..
        }) = self.items.last_mut()
        {
            *current = status;
            *expanded = true;
            self.dirty = true;
            return;
        }
        self.items.push(Item::Activity {
            entries: Vec::new(),
            grouped: self.tool_preferences.grouped_activity,
            expanded: true,
            status,
        });
        self.dirty = true;
    }

    fn close_activity(&mut self) {
        if let Some(Item::Activity {
            entries,
            grouped,
            expanded,
            status,
        }) = self.items.last_mut()
        {
            if entries.is_empty() {
                self.items.pop();
            } else {
                *status = ActivityStatus::Complete;
                if *grouped {
                    *expanded = false;
                }
            }
            self.dirty = true;
        }
        self.thinking = None;
    }

    /// Finish the current activity block at the end of a turn.
    pub(crate) fn finish_activity(&mut self) {
        self.close_activity();
    }

    fn item_height(
        item: &Item,
        width: usize,
        diff_mode: crate::diff::DiffMode,
        doc_lines: Option<usize>,
    ) -> u16 {
        match item {
            Item::User { .. } | Item::Assistant { .. } => u16::try_from(doc_lines.unwrap_or(1))
                .unwrap_or(u16::MAX)
                .saturating_add(BLOCK_GAP),
            Item::Notice { text, .. } | Item::Retry { text, .. } => {
                let text_width = width.saturating_sub(2 + NOTICE_PREFIX_CHARS).max(1);
                u16::try_from(wrap(text, text_width).len())
                    .unwrap_or(u16::MAX)
                    .saturating_add(BLOCK_GAP)
            }
            Item::Reasoning { text, expanded } => {
                if *expanded {
                    u16::try_from(wrap(text, width).len() + 1)
                        .unwrap_or(u16::MAX)
                        .saturating_add(BLOCK_GAP)
                } else {
                    1 + BLOCK_GAP
                }
            }
            Item::Tool {
                diff,
                output,
                error,
                expanded,
                console,
                ..
            } => {
                let console_width = u16::try_from(width.saturating_sub(2)).unwrap_or(u16::MAX);
                let body = if *expanded {
                    diff.as_ref().map_or_else(
                        || {
                            output.as_deref().or(error.as_deref()).map_or(0, |text| {
                                wrap(text, width.saturating_sub(4).max(1)).len() + 1
                            })
                        },
                        |diff| diff.rows(diff_mode) + 2,
                    )
                } else {
                    0
                };
                let extra = body
                    + console.as_ref().map_or(0, |console| {
                        usize::from(console.wrapped_height(console_width)) + 2
                    });
                u16::try_from(extra + 2).unwrap_or(u16::MAX)
            }
            Item::Activity {
                entries,
                grouped,
                expanded,
                ..
            } => {
                let children = if *grouped && !*expanded {
                    0
                } else {
                    entries.iter().fold(0_u16, |height, entry| {
                        height.saturating_add(Self::item_height(entry, width, diff_mode, None))
                    })
                };
                if *grouped {
                    children.saturating_add(2)
                } else {
                    children
                }
            }
        }
    }

    fn compute_heights(&mut self, width: u16, theme: Theme) {
        if !self.dirty && self.heights_width == width && self.doc_theme == Some(theme) {
            return;
        }
        let cells = usize::from(width);
        self.docs = self
            .items
            .iter()
            .map(|item| match item {
                Item::User { text, .. } => Some(super::markdown::Markdown::render(
                    text,
                    u16::try_from(cells.saturating_sub(1)).unwrap_or(u16::MAX),
                    &theme,
                )),
                Item::Assistant { text, .. } => {
                    Some(super::markdown::Markdown::render(text, width, &theme))
                }
                _ => None,
            })
            .collect();
        self.heights = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let lines = self
                    .docs
                    .get(index)
                    .and_then(Option::as_ref)
                    .map(|doc| doc.lines.len());
                Self::item_height(item, cells, self.diff_mode, lines)
            })
            .collect();
        self.content_height = self
            .heights
            .iter()
            .fold(0u16, |total, height| total.saturating_add(*height));
        self.heights_width = width;
        self.doc_theme = Some(theme);
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
        let theme = self.doc_theme.unwrap_or_default();
        self.compute_heights(self.heights_width.max(1), theme);
        let mut top = 0u16;
        for (index, height) in self.heights.iter().enumerate() {
            let end = top.saturating_add(*height);
            if row >= top && row < end {
                if toggle_reasoning_in_item(
                    &mut self.items[index],
                    row - top,
                    usize::from(self.heights_width.max(1)),
                    self.diff_mode,
                ) {
                    self.dirty = true;
                    return true;
                }
                return false;
            }
            top = end;
        }
        false
    }

    /// Flip the expansion of the shell output at `row`, when that row has one.
    pub(crate) fn toggle_console_at(&mut self, row: u16) -> bool {
        let theme = self.doc_theme.unwrap_or_default();
        self.compute_heights(self.heights_width.max(1), theme);
        let mut top = 0u16;
        for (index, height) in self.heights.iter().enumerate() {
            let end = top.saturating_add(*height);
            if row >= top && row < end {
                if toggle_console_in_item(
                    &mut self.items[index],
                    row - top,
                    usize::from(self.heights_width.max(1)),
                    self.diff_mode,
                ) {
                    self.dirty = true;
                    return true;
                }
                return false;
            }
            top = end;
        }
        false
    }

    /// Flip the details of a tool call at its header row.
    pub(crate) fn toggle_tool_at(&mut self, row: u16) -> bool {
        let theme = self.doc_theme.unwrap_or_default();
        self.compute_heights(self.heights_width.max(1), theme);
        let mut top = 0_u16;
        for (index, height) in self.heights.iter().enumerate() {
            let end = top.saturating_add(*height);
            if row >= top && row < end {
                if toggle_tool_in_item(
                    &mut self.items[index],
                    row - top,
                    usize::from(self.heights_width.max(1)),
                    self.diff_mode,
                ) {
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
        paint: &ItemPaint<'_>,
    ) {
        let Item::Tool {
            name,
            target,
            state,
            expanded,
            meta,
            diff,
            output,
            error,
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
                target: if *expanded && matches!(name.as_str(), "shell" | "bash") {
                    None
                } else {
                    target.as_deref()
                },
                state: *state,
                meta: if *expanded { None } else { meta.as_deref() },
                expanded: *expanded,
                frame: paint.thinking.map_or(0, |elapsed| {
                    usize::try_from(elapsed.as_millis() / 80).unwrap_or(0)
                }),
            },
            paint.clip,
            paint.theme,
        );
        let mut body_rows = 0_u16;
        if *expanded && let Some(diff) = diff {
            body_rows = u16::try_from(diff.rows(paint.diff_mode) + 2).unwrap_or(u16::MAX);
            let box_rect = Rect {
                y: rect.y + 1,
                height: body_rows.min(rect.height.saturating_sub(1)),
                ..rect
            };
            paint_diff(
                buf,
                box_rect,
                diff,
                paint.diff_mode,
                target.as_deref().unwrap_or("edit"),
                paint.clip,
                paint.theme,
            );
        } else if *expanded && let Some(text) = error.as_deref().or(output.as_deref()) {
            body_rows = paint_tool_output(
                buf,
                Rect {
                    x: rect.x.saturating_add(2),
                    y: rect.y.saturating_add(1),
                    width: rect.width.saturating_sub(4),
                    height: rect.height.saturating_sub(1),
                },
                text,
                error.is_some(),
                paint.clip,
                paint.theme,
            );
        }
        if let Some(console) = console {
            let console_width = rect.width.saturating_sub(4);
            let rows = console.wrapped_height(console_width);
            // Keep a visual gutter between the tool summary and its stdout panel.
            let y = rect.y.saturating_add(2).saturating_add(body_rows);
            if y < paint.clip {
                console.render(
                    Rect {
                        x: rect.x.saturating_add(1),
                        y,
                        width: rect.width.saturating_sub(2),
                        height: rows.min(paint.clip.saturating_sub(y)),
                    },
                    buf,
                    cx,
                    paint.theme,
                );
            }
        }
    }

    fn paint_activity_item(
        item: &mut Item,
        rect: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        paint: &ItemPaint<'_>,
    ) {
        let Item::Activity {
            entries,
            grouped,
            expanded,
            status,
        } = item
        else {
            return;
        };
        let mut y = rect.y;
        if *grouped {
            paint_activity_header(
                buf,
                Rect { height: 1, ..rect },
                status,
                entries,
                *expanded,
                paint,
            );
            if !*expanded {
                return;
            }
            y = y.saturating_add(1);
        }
        for entry in entries {
            let height = Self::item_height(entry, usize::from(rect.width), paint.diff_mode, None);
            Self::paint_item(
                entry,
                Rect { y, height, ..rect },
                buf,
                cx,
                &ItemPaint {
                    focused: false,
                    doc: None,
                    ..*paint
                },
            );
            y = y.saturating_add(height);
        }
    }

    fn paint_item(
        item: &mut Item,
        rect: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        paint: &ItemPaint<'_>,
    ) {
        let theme = paint.theme;
        match item {
            Item::User { .. } => {
                if paint.focused {
                    buf.set_style(rect, Style::default().bg(theme.secondary));
                }
                paint_user(buf, rect, paint.doc, paint.clip, theme);
            }
            Item::Assistant { .. } => {
                if paint.focused {
                    buf.set_style(rect, Style::default().bg(theme.secondary));
                }
                paint_assistant(buf, rect, paint.doc, paint.clip, paint.streaming, theme);
            }
            Item::Tool { .. } => {
                Self::paint_tool_item(item, rect, buf, cx, paint);
            }
            Item::Activity { .. } => {
                Self::paint_activity_item(item, rect, buf, cx, paint);
            }
            Item::Notice { text, tone } | Item::Retry { text, tone } => {
                paint_notice(buf, rect, text, *tone, paint.clip, theme);
            }
            Item::Reasoning { text, expanded } => {
                paint_reasoning(buf, rect, text, *expanded, paint.clip, theme);
            }
        }
    }

    fn blit_visible_rows(
        source: &Buffer,
        target: &mut Buffer,
        content: Rect,
        screen_y: u16,
        first_row: u16,
        rows: u16,
    ) {
        for row in 0..rows {
            for column in 0..content.width {
                let Some(cell) = source.cell(ratatui::layout::Position::new(
                    content.x + column,
                    first_row + row,
                )) else {
                    continue;
                };
                if let Some(dst) = target.cell_mut(ratatui::layout::Position::new(
                    content.x + column,
                    screen_y + row,
                )) {
                    *dst = cell.clone();
                }
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
            let text_end = line
                .iter()
                .rposition(|symbol| !symbol.trim().is_empty())
                .map_or(0, |column| column + 1);
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
            let mut to = if row == end.row {
                usize::from(end.col).saturating_add(1).min(text_end)
            } else {
                text_end
            };
            if let Some(bar) = self.scrollbar {
                let rail = usize::from(bar.x.saturating_sub(area.x));
                to = to.min(rail);
            }
            if from >= to {
                continue;
            }
            let y = area.y + row;
            for x in from..to {
                let position = ratatui::layout::Position::new(
                    area.x + u16::try_from(x).unwrap_or(u16::MAX),
                    y,
                );
                if let Some(cell) = buf.cell_mut(position)
                    && !cell.symbol().trim().is_empty()
                {
                    cell.set_bg(theme.accent);
                    cell.set_fg(theme.background);
                }
            }
        }
    }

    /// Which item produced each visible row, if any.
    fn build_row_map(&self) -> Vec<Option<usize>> {
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
    fn capture_painted(&mut self, area: Rect, buf: &Buffer, row_map: &[Option<usize>]) {
        self.painted.clear();
        self.painted_copyable.clear();
        self.painted_user.clear();
        for y in area.y..area.y + area.height {
            let row = usize::from(y - area.y);
            let mut line = Vec::with_capacity(area.width as usize);
            for x in 0..area.width {
                if let Some(cell) = buf.cell(ratatui::layout::Position::new(area.x + x, y)) {
                    line.push(cell.symbol().to_owned());
                }
            }
            let copyable = row_map.get(row).copied().flatten().is_some_and(|index| {
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
            let user = row_map
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
        let mut selected_lines = Vec::new();
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
            let cells = line;
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
            let mut to = if row == end.row {
                usize::from(end.col).saturating_add(1).min(cells.len())
            } else {
                cells.len()
            };
            if let Some(bar) = self.scrollbar {
                to = to.min(usize::from(bar.x.saturating_sub(self.area.x)));
            }
            if from < to {
                let slice = cells[from..to].concat();
                let text = slice.trim_matches(' ');
                if !text.is_empty() {
                    selected_lines.push(text.to_owned());
                }
            }
        }
        (!selected_lines.is_empty()).then(|| selected_lines.join("\n"))
    }

    /// Mouse: scroll, select text to copy on release, toggle thoughts.
    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Outcome {
        use crossterm::event::{MouseButton, MouseEventKind};

        let inside = mouse.row >= self.area.y
            && mouse.row < self.area.y + self.area.height
            && mouse.column >= self.area.x
            && mouse.column < self.area.x + self.area.width;
        let mut pos = Pos {
            row: mouse.row.saturating_sub(self.area.y),
            col: mouse.column.saturating_sub(self.area.x),
        };
        let bar = self.scrollbar.filter(|_| inside);
        if let Some(bar) = bar {
            // The rail is one cell wide; never let text selection cover it.
            pos.col = pos
                .col
                .min(bar.x.saturating_sub(self.area.x).saturating_sub(1));
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll(-3);
                Outcome::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.scroll(3);
                Outcome::Consumed
            }
            MouseEventKind::Down(MouseButton::Left)
                if let Some(bar) = bar
                    && mouse.column == bar.x =>
            {
                let local = mouse.row.saturating_sub(self.area.y);
                if bar.thumb_contains(local) {
                    self.dragging_thumb = true;
                } else {
                    let page = isize::try_from(self.view_height.max(1)).unwrap_or(isize::MAX);
                    self.scroll(bar.page(local) * page);
                }
                Outcome::Consumed
            }
            // Right click opens the per-message menu at the pointer.
            MouseEventKind::Down(MouseButton::Right) => {
                if self.click_menu(mouse.column, mouse.row) {
                    return Outcome::Consumed;
                }
                if !inside {
                    return Outcome::Ignored;
                }
                let local_row = mouse.row.saturating_sub(self.area.y);
                if let Some(index) = self.message_at(usize::from(local_row)) {
                    self.focused_item = Some(index);
                    self.open_menu(mouse.column, local_row);
                    return Outcome::Consumed;
                }
                self.menu = None;
                Outcome::Consumed
            }
            // Left click selects text (and toggles thoughts on a non-message row).
            MouseEventKind::Down(MouseButton::Left) => {
                if self.click_menu(mouse.column, mouse.row) {
                    return Outcome::Consumed;
                }
                if !inside {
                    return Outcome::Ignored;
                }
                if let Some(url) = self.link_at_screen(mouse.row, mouse.column) {
                    super::markdown::open_link(url);
                    return Outcome::Consumed;
                }
                let row = mouse
                    .row
                    .saturating_sub(self.origin_y)
                    .saturating_add(self.first_visible);
                if self.toggle_tool_at(row)
                    || self.toggle_reasoning_at(row)
                    || self.toggle_console_at(row)
                {
                    return Outcome::Consumed;
                }
                self.start_selection(true, pos)
            }
            MouseEventKind::Up(MouseButton::Left | MouseButton::Right) if self.dragging_thumb => {
                self.dragging_thumb = false;
                Outcome::Consumed
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging_thumb => {
                if let Some(bar) = bar {
                    self.drag_thumb(bar, mouse.row.saturating_sub(self.area.y));
                }
                Outcome::Consumed
            }
            MouseEventKind::Drag(MouseButton::Left | MouseButton::Right) => {
                let Some(selection) = &mut self.selection else {
                    return Outcome::Ignored;
                };
                selection.cur = pos;
                Outcome::Consumed
            }
            MouseEventKind::Up(MouseButton::Left | MouseButton::Right) => self.finish_selection(),
            _ => Outcome::Ignored,
        }
    }

    fn paint_menu(&self, buf: &mut Buffer, theme: &Theme) {
        let Some(menu) = self.menu else {
            return;
        };
        let rect = Rect {
            x: menu.column,
            y: menu.row + 1,
            width: 26,
            height: 2,
        };
        // Cover the text under the menu so the buttons stay readable.
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_symbol(" ");
                    cell.set_style(Style::default().bg(theme.secondary));
                }
            }
        }
        for index in 0..3 {
            let button = Self::menu_button_rect(menu, index);
            let label = ["fork", "copy", "rewind"][index];
            buf.set_stringn(
                button.x,
                button.y,
                label,
                label.len(),
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.secondary)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }

    /// Take the message actions queued by the floating menu.
    pub(crate) fn take_message_actions(&mut self) -> Vec<MessageAction> {
        std::mem::take(&mut self.message_actions)
    }

    /// Take the toast requests queued by copy actions.
    pub(crate) fn take_toasts(&mut self) -> Vec<ToastRequest> {
        std::mem::take(&mut self.toasts)
    }

    /// The item occupying a visible row, if it is a message.
    fn message_at(&self, local_row: usize) -> Option<usize> {
        self.row_map
            .get(local_row)
            .copied()
            .flatten()
            .filter(|index| {
                matches!(
                    self.items.get(*index),
                    Some(Item::User { .. } | Item::Assistant { .. })
                )
            })
    }

    /// The link under a screen cell, when a message has one there.
    fn link_at_screen(&self, screen_row: u16, column: u16) -> Option<&str> {
        let local_row = screen_row.checked_sub(self.area.y)?;
        let item = self.row_map.get(usize::from(local_row))?.as_ref()?;
        let content_row = self.first_visible.saturating_add(local_row);
        let top = self
            .heights
            .iter()
            .take(*item)
            .fold(0u16, |total, height| total.saturating_add(*height));
        let line = usize::from(content_row.saturating_sub(top));
        let doc = self.docs.get(*item)?.as_ref()?;
        let text_x = match self.items.get(*item) {
            Some(Item::User { .. }) => self.area.x + 2,
            _ => self.area.x + 1,
        };
        let local = usize::from(column.saturating_sub(text_x));
        doc.link_at(line, local)
    }

    /// Open the menu for a clicked message.
    fn open_menu(&mut self, column: u16, local_row: u16) {
        self.menu = self
            .message_at(usize::from(local_row))
            .map(|item| MessageMenu {
                item,
                column,
                row: local_row + self.area.y,
            });
    }

    fn start_selection(&mut self, inside: bool, pos: Pos) -> Outcome {
        if !inside {
            return Outcome::Ignored;
        }
        self.menu = None;
        self.selection = Some(Selection {
            anchor: pos,
            cur: pos,
        });
        Outcome::Consumed
    }

    fn finish_selection(&mut self) -> Outcome {
        let Some(selection) = self.selection.take() else {
            return Outcome::Consumed;
        };
        if selection.anchor == selection.cur {
            return Outcome::Consumed;
        }
        match self.selection_text(selection) {
            Some(text) => self.copy_text(&text, "selection"),
            None => {
                self.toasts.push(ToastRequest {
                    text: "selection produced no text".to_string(),
                    tone: Tone::Warn,
                });
            }
        }
        Outcome::Consumed
    }

    fn click_menu(&mut self, column: u16, row: u16) -> bool {
        let Some(menu) = self.menu else {
            return false;
        };
        for index in 0..3 {
            if Self::menu_button_rect(menu, index).contains(Position::new(column, row)) {
                self.activate_menu(index);
                return true;
            }
        }
        self.menu = None;
        false
    }

    fn menu_button_rect(menu: MessageMenu, index: usize) -> Rect {
        Rect {
            x: menu.column + u16::try_from(index).unwrap_or(0) * 8,
            y: menu.row + 1,
            width: 8,
            height: 1,
        }
    }

    /// Apply a menu button click.
    fn activate_menu(&mut self, button: usize) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let (turn, source) = match self.items.get(menu.item) {
            Some(Item::User { turn, text, .. } | Item::Assistant { turn, text, .. }) => {
                (*turn, text.clone())
            }
            _ => return,
        };
        let text = self
            .docs
            .get(menu.item)
            .and_then(Option::as_ref)
            .map_or(source, rendered_text);
        match button {
            0 => self.message_actions.push(MessageAction::Fork { turn }),
            1 => self.copy_text(&text, "message text"),
            _ => self.message_actions.push(MessageAction::Rewind { turn }),
        }
    }

    fn copy_text(&mut self, text: &str, description: &str) {
        let request = match self.clipboard.write(text) {
            Ok(()) => ToastRequest {
                text: format!("{description} copied ({} characters)", text.chars().count()),
                tone: Tone::Neutral,
            },
            Err(error) => ToastRequest {
                text: format!("could not copy {description}: {error}"),
                tone: Tone::Warn,
            },
        };
        self.toasts.push(request);
    }

    fn drag_thumb(&mut self, bar: Scrollbar, local: u16) {
        self.first_visible = bar.offset_for_drag(local, self.max_offset());
        self.follow_tail = self.first_visible >= self.max_offset();
    }

    /// Scroll the stream by `delta` rows; negative scrolls toward the top.
    pub(crate) fn scroll(&mut self, delta: isize) {
        let max = i64::from(self.max_offset());
        let next = i64::from(self.first_visible) + i64::try_from(delta).unwrap_or(0);
        self.first_visible = u16::try_from(next.clamp(0, max)).unwrap_or(u16::MAX);
        self.follow_tail = self.first_visible >= self.max_offset();
    }

    fn paint_items(
        &mut self,
        area: Rect,
        content: Rect,
        buf: &mut Buffer,
        cx: &mut Context,
        docs: &[Option<super::markdown::Markdown>],
        theme: &Theme,
    ) {
        let bottom = area.y + area.height;
        let skip = self.first_visible;
        let item_count = self.items.len();
        let streaming = self.streaming;
        let mut row: u16 = 0;
        for (index, item) in self.items.iter_mut().enumerate() {
            let height = self.heights[index];
            let top = row;
            row += height;
            if top + height <= skip || top >= skip + area.height {
                continue;
            }
            let screen_y = area.y + top.saturating_sub(skip);
            let scrolled = skip.saturating_sub(top);
            let visible = height.saturating_sub(scrolled).min(area.height);
            let paint = ItemPaint {
                clip: bottom,
                diff_mode: self.diff_mode,
                focused: self.focused_item == Some(index),
                streaming: (index + 1 == item_count).then_some(streaming).flatten(),
                thinking: self.thinking,
                doc: docs.get(index).and_then(Option::as_ref),
                theme,
            };
            if scrolled == 0 {
                Self::paint_item(
                    item,
                    Rect {
                        y: screen_y,
                        height,
                        ..content
                    },
                    buf,
                    cx,
                    &paint,
                );
                continue;
            }
            let scratch_height = height.min(scrolled.saturating_add(area.height));
            let scratch_area = Rect {
                x: content.x,
                y: 0,
                width: content.width,
                height: scratch_height,
            };
            let mut scratch = Buffer::empty(scratch_area);
            let mut scratch_paint = paint;
            scratch_paint.clip = scratch_height;
            Self::paint_item(
                item,
                Rect {
                    y: 0,
                    height,
                    ..scratch_area
                },
                &mut scratch,
                cx,
                &scratch_paint,
            );
            if let Item::Tool {
                console: Some(console),
                ..
            } = item
            {
                console.state.area = Rect {
                    y: screen_y,
                    height: visible,
                    ..content
                };
            }
            Self::blit_visible_rows(&scratch, buf, content, screen_y, scrolled, visible);
        }
    }
}

fn push_activity_entry(items: &mut Vec<Item>, entry: Item, grouped: bool) {
    if let Some(Item::Activity {
        entries,
        expanded,
        status,
        ..
    }) = items.last_mut()
    {
        entries.push(entry);
        *expanded = true;
        if let Some(last) = entries.last() {
            *status = entry_activity_status(last);
        }
    } else {
        let status = match &entry {
            Item::Tool {
                name,
                target,
                meta,
                state: ToolState::Running,
                ..
            } => ActivityStatus::Tool {
                name: name.clone(),
                context: target.clone(),
                meta: meta.clone(),
            },
            _ => ActivityStatus::Thinking,
        };
        items.push(Item::Activity {
            entries: vec![entry],
            grouped,
            expanded: true,
            status,
        });
    }
}

fn entry_activity_status(entry: &Item) -> ActivityStatus {
    match entry {
        Item::Tool {
            name,
            target,
            meta,
            state: ToolState::Running,
            ..
        } => ActivityStatus::Tool {
            name: name.clone(),
            context: target.clone(),
            meta: meta.clone(),
        },
        _ => ActivityStatus::Thinking,
    }
}

fn group_activity_items(items: Vec<Item>) -> Vec<Item> {
    let mut grouped = Vec::new();
    for item in items {
        if matches!(item, Item::Tool { .. } | Item::Reasoning { .. }) {
            push_activity_entry(&mut grouped, item, true);
        } else {
            complete_activity(&mut grouped);
            grouped.push(item);
        }
    }
    complete_activity(&mut grouped);
    grouped
}

fn flatten_activity_items(items: Vec<Item>) -> Vec<Item> {
    let mut flattened = Vec::new();
    for item in items {
        if let Item::Activity { entries, .. } = item {
            flattened.extend(entries);
        } else {
            flattened.push(item);
        }
    }
    flattened
}

fn complete_activity(items: &mut [Item]) {
    if let Some(Item::Activity {
        status, expanded, ..
    }) = items.last_mut()
        && !matches!(status, ActivityStatus::Tool { .. })
    {
        *status = ActivityStatus::Complete;
        *expanded = false;
    }
}

fn last_activity_entries_mut(items: &mut [Item]) -> Option<&mut Vec<Item>> {
    match items.last_mut()? {
        Item::Activity { entries, .. } => Some(entries),
        _ => None,
    }
}

fn for_each_tool_mut(items: &mut [Item], action: &mut impl FnMut(&mut Item)) {
    for item in items {
        match item {
            Item::Activity { entries, .. } => for_each_tool_mut(entries, action),
            Item::Tool { .. } => action(item),
            _ => {}
        }
    }
}

fn set_activity_status(items: &mut [Item], status: ActivityStatus) {
    if let Some(Item::Activity {
        status: current,
        expanded,
        ..
    }) = items.last_mut()
    {
        *current = status;
        *expanded = true;
    }
}

fn toggle_tool_in_item(
    item: &mut Item,
    row: u16,
    width: usize,
    diff_mode: crate::diff::DiffMode,
) -> bool {
    match item {
        Item::Activity {
            entries,
            grouped,
            expanded,
            ..
        } => {
            if *grouped {
                if row == 0 {
                    *expanded = !*expanded;
                    return true;
                }
                if !*expanded {
                    return false;
                }
            }
            let mut offset = row.saturating_sub(u16::from(*grouped));
            for entry in entries {
                let height = Transcript::item_height(entry, width, diff_mode, None);
                if offset < height {
                    return toggle_tool_in_item(entry, offset, width, diff_mode);
                }
                offset = offset.saturating_sub(height);
            }
            false
        }
        Item::Tool { expanded, .. } if row == 0 => {
            *expanded = !*expanded;
            true
        }
        _ => false,
    }
}

fn toggle_reasoning_in_item(
    item: &mut Item,
    row: u16,
    width: usize,
    diff_mode: crate::diff::DiffMode,
) -> bool {
    match item {
        Item::Activity {
            entries,
            grouped,
            expanded,
            ..
        } => {
            if *grouped && (row == 0 || !*expanded) {
                return false;
            }
            let mut offset = row.saturating_sub(u16::from(*grouped));
            for entry in entries {
                let height = Transcript::item_height(entry, width, diff_mode, None);
                if offset < height {
                    return toggle_reasoning_in_item(entry, offset, width, diff_mode);
                }
                offset = offset.saturating_sub(height);
            }
            false
        }
        Item::Reasoning { expanded, .. } if row == 0 => {
            *expanded = !*expanded;
            true
        }
        _ => false,
    }
}

fn toggle_console_in_item(
    item: &mut Item,
    row: u16,
    width: usize,
    diff_mode: crate::diff::DiffMode,
) -> bool {
    match item {
        Item::Activity {
            entries,
            grouped,
            expanded,
            ..
        } => {
            if *grouped && (row == 0 || !*expanded) {
                return false;
            }
            let mut offset = row.saturating_sub(u16::from(*grouped));
            for entry in entries {
                let height = Transcript::item_height(entry, width, diff_mode, None);
                if offset < height {
                    return toggle_console_in_item(entry, offset, width, diff_mode);
                }
                offset = offset.saturating_sub(height);
            }
            false
        }
        Item::Tool {
            diff,
            output,
            error,
            expanded,
            console: Some(console),
            ..
        } => {
            let body = if *expanded {
                diff.as_ref().map_or_else(
                    || {
                        output.as_deref().or(error.as_deref()).map_or(0, |text| {
                            wrap(text, width.saturating_sub(4).max(1)).len() + 1
                        })
                    },
                    |diff| diff.rows(diff_mode) + 2,
                )
            } else {
                0
            };
            let console_start = body.saturating_add(2);
            match usize::from(row).cmp(&console_start) {
                std::cmp::Ordering::Equal => {
                    console.toggle_output_visible();
                    true
                }
                std::cmp::Ordering::Greater => {
                    console.toggle_expanded();
                    true
                }
                std::cmp::Ordering::Less => false,
            }
        }
        _ => false,
    }
}

fn rendered_text(document: &super::markdown::Markdown) -> String {
    let mut lines: Vec<String> = document
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
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
        if let Some(streaming) = self.streaming {
            self.streaming = Some(streaming.saturating_add(delta));
        }
        for_each_tool_mut(&mut self.items, &mut |item| {
            if let Item::Tool {
                console: Some(console),
                ..
            } = item
            {
                console.tick(delta, cx);
            }
        });
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let _ = cx.is_focused();
        self.view_height = area.height;
        self.origin_y = area.y;
        self.area = area;
        let content = Rect {
            x: area.x.saturating_add(2),
            width: area.width.saturating_sub(3),
            ..area
        };
        self.compute_heights(content.width, *theme);
        let row_map = self.build_row_map();

        let docs = std::mem::take(&mut self.docs);
        self.paint_items(area, content, buf, cx, &docs, theme);
        self.docs = docs;

        if !self.tool_preferences.grouped_activity
            && !has_running_tool(&self.items)
            && self.first_visible >= self.max_offset()
            && let Some(thinking) = self.thinking
        {
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
            let (top, size) = scroll.thumb();
            self.scrollbar = Some(Scrollbar::new(
                area.x + area.width.saturating_sub(1),
                area.y,
                area.height,
                top,
                size,
            ));
            render_scrollbar(buf, area, &scroll_style, theme, &scroll);
        } else {
            self.scrollbar = None;
            self.dragging_thumb = false;
        }

        self.capture_painted(area, buf, &row_map);
        self.row_map = row_map;
        self.paint_selection(area, buf, theme);
        self.paint_menu(buf, theme);
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

fn paint_user(
    buf: &mut Buffer,
    area: Rect,
    doc: Option<&super::markdown::Markdown>,
    clip: u16,
    theme: &Theme,
) {
    let text = Rect {
        x: area.x.saturating_add(1),
        width: area.width.saturating_sub(2),
        ..area
    };
    buf.set_style(text, Style::default().bg(theme.secondary));
    paint_doc(buf, text, doc, clip);
    for y in area.y..(area.y + area.height).min(clip) {
        buf.set_stringn(
            area.x,
            y,
            "▌",
            1,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
    }
}

fn paint_assistant(
    buf: &mut Buffer,
    area: Rect,
    doc: Option<&super::markdown::Markdown>,
    clip: u16,
    streaming: Option<Duration>,
    theme: &Theme,
) {
    paint_doc(buf, area, doc, clip);
    let Some(elapsed) = streaming else {
        return;
    };
    let Some(line) = doc.and_then(|doc| doc.lines.iter().rev().find(|line| !line.spans.is_empty()))
    else {
        return;
    };
    let row = doc
        .and_then(|doc| doc.lines.iter().rposition(|line| !line.spans.is_empty()))
        .unwrap_or(0);
    let Some(y) = area.y.checked_add(u16::try_from(row).unwrap_or(u16::MAX)) else {
        return;
    };
    if y >= clip || y >= area.y.saturating_add(area.height) {
        return;
    }
    let width = line
        .spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    if width >= usize::from(area.width) {
        return;
    }
    let x = area
        .x
        .saturating_add(u16::try_from(width).unwrap_or(u16::MAX));
    let color = if elapsed.as_millis() / 450 % 2 == 0 {
        theme.accent
    } else {
        theme.muted
    };
    buf.set_stringn(
        x,
        y,
        "▍",
        1,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );
}

fn paint_doc(buf: &mut Buffer, area: Rect, doc: Option<&super::markdown::Markdown>, clip: u16) {
    let Some(doc) = doc else {
        return;
    };
    for (row, line) in doc.lines.iter().enumerate() {
        let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
        if y >= clip || y >= area.y.saturating_add(area.height) {
            break;
        }
        let mut column = 0u16;
        for span in &line.spans {
            if column >= area.width {
                break;
            }
            buf.set_stringn(
                area.x + column,
                y,
                span.content.as_ref(),
                usize::from(area.width.saturating_sub(column)),
                span.style,
            );
            column = column.saturating_add(u16::try_from(span.content.width()).unwrap_or(u16::MAX));
        }
    }
}

#[derive(Clone, Copy)]
struct ToolRow<'a> {
    name: &'a str,
    target: Option<&'a str>,
    state: ToolState,
    meta: Option<&'a str>,
    expanded: bool,
    frame: usize,
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
        expanded,
        frame,
    } = row;
    let (mark, mark_color) = match state {
        ToolState::Done => ("✓", theme.success),
        ToolState::Running => (SPINNER_FRAMES[frame % SPINNER_FRAMES.len()], theme.accent),
        ToolState::Failed => ("!", theme.destructive),
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
    let col = area.x + 2 + u16::try_from(name.chars().count()).unwrap_or(u16::MAX) + 2;
    let marker = if expanded { "[-]" } else { "[+]" };
    let marker_width = 4_u16;
    let right = area
        .x
        .saturating_add(area.width.saturating_sub(marker_width));
    let meta_width = meta.map_or(0, |value| {
        u16::try_from(value.chars().count())
            .unwrap_or(u16::MAX)
            .saturating_add(1)
    });
    let target_width = right.saturating_sub(col).saturating_sub(meta_width);
    if let Some(target) = target {
        let clipped = clip_cells(target, usize::from(target_width));
        buf.set_stringn(
            col,
            y,
            &clipped,
            usize::from(target_width),
            Style::default().fg(theme.muted),
        );
    }
    if let Some(meta) = meta {
        let meta_x = right.saturating_sub(u16::try_from(meta.chars().count()).unwrap_or(u16::MAX));
        buf.set_stringn(
            meta_x,
            y,
            meta,
            usize::from(right.saturating_sub(meta_x)),
            Style::default().fg(theme.success),
        );
    }
    buf.set_stringn(
        right,
        y,
        marker,
        usize::from(marker_width),
        Style::default().fg(theme.accent),
    );
}

fn paint_activity_header(
    buf: &mut Buffer,
    area: Rect,
    status: &ActivityStatus,
    entries: &[Item],
    expanded: bool,
    paint: &ItemPaint<'_>,
) {
    if area.y >= paint.clip || area.width == 0 {
        return;
    }
    let (label, color, running) = match status {
        ActivityStatus::Thinking => ("thinking".to_owned(), paint.theme.accent, true),
        ActivityStatus::Tool {
            name,
            context,
            meta,
        } => {
            let mut label = context
                .as_ref()
                .map_or_else(|| name.clone(), |context| format!("{name} · {context}"));
            if let Some(meta) = meta {
                label.push_str(" · ");
                label.push_str(meta);
            }
            (label, paint.theme.accent, true)
        }
        ActivityStatus::Complete => {
            let count = count_tools(entries);
            let label = if count == 0 {
                "reasoning".to_owned()
            } else if count == 1 {
                "1 tool call".to_owned()
            } else {
                format!("{count} tool calls")
            };
            (label, paint.theme.muted, false)
        }
    };
    let marker = if expanded { "[-]" } else { "[+]" };
    let mut column = area.x;
    if running {
        let elapsed = paint.thinking.unwrap_or_default();
        let frame = SPINNER_FRAMES
            [usize::try_from(elapsed.as_millis() / 80).unwrap_or(0) % SPINNER_FRAMES.len()];
        buf.set_stringn(column, area.y, frame, 1, Style::default().fg(color));
        column = column.saturating_add(2);
    }
    let marker_width = 4_u16;
    let label_width = area
        .width
        .saturating_sub(column.saturating_sub(area.x))
        .saturating_sub(marker_width)
        .saturating_sub(1);
    let clipped = clip_cells(&label, usize::from(label_width));
    buf.set_stringn(
        column,
        area.y,
        &clipped,
        usize::from(label_width),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );
    buf.set_stringn(
        area.x
            .saturating_add(area.width.saturating_sub(marker_width)),
        area.y,
        marker,
        usize::from(marker_width),
        Style::default().fg(paint.theme.accent),
    );
}

fn count_tools(items: &[Item]) -> usize {
    items
        .iter()
        .map(|item| match item {
            Item::Tool { .. } => 1,
            Item::Activity { entries, .. } => count_tools(entries),
            _ => 0,
        })
        .sum()
}

fn has_running_tool(items: &[Item]) -> bool {
    items.iter().any(|item| match item {
        Item::Tool { state, .. } => *state == ToolState::Running,
        Item::Activity { entries, .. } => has_running_tool(entries),
        _ => false,
    })
}

fn paint_tool_output(
    buf: &mut Buffer,
    area: Rect,
    text: &str,
    is_error: bool,
    clip: u16,
    theme: &Theme,
) -> u16 {
    if area.width == 0 || area.height == 0 {
        return 0;
    }
    let lines = wrap(text, usize::from(area.width));
    let rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let style = Style::default().fg(if is_error {
        theme.destructive
    } else {
        theme.foreground
    });
    for (index, line) in lines.iter().enumerate() {
        let y = area
            .y
            .saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
        if y >= clip || y >= area.y.saturating_add(area.height) {
            break;
        }
        buf.set_stringn(area.x, y, line, usize::from(area.width), style);
    }
    rows
}

fn clip_cells(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut clipped = String::new();
    let target = width.saturating_sub(1);
    let mut cells = 0usize;
    for character in text.chars() {
        let character_width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if cells.saturating_add(character_width) > target {
            break;
        }
        clipped.push(character);
        cells += character_width;
    }
    clipped.push('…');
    clipped
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
            offset + 1,
            theme,
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
                index + 1,
                theme,
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
                    index + 1,
                    theme,
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
                    index + 1,
                    theme,
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

#[allow(clippy::too_many_arguments)]
fn paint_diff_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    sign: &str,
    line: &str,
    number: usize,
    theme: &Theme,
    style: Style,
) {
    let gutter = 4u16;
    let number_text = format!("{number:>3}");
    buf.set_stringn(
        x,
        y,
        &number_text,
        gutter as usize,
        Style::default().fg(theme.muted).bg(theme.secondary),
    );
    buf.set_stringn(x + gutter, y, sign, 1, style);
    buf.set_stringn(
        x + gutter + 2,
        y,
        line,
        usize::from(width.saturating_sub(gutter + 2)),
        style,
    );
}

fn paint_notice(buf: &mut Buffer, area: Rect, text: &str, tone: Tone, clip: u16, theme: &Theme) {
    let (glyph, color) = match tone {
        Tone::Neutral => ("·", theme.muted),
        Tone::Warn => ("!", theme.warning),
        Tone::Error => ("✗", theme.destructive),
    };
    let budget = usize::from(area.width.saturating_sub(2));
    let prefix_len = NOTICE_PREFIX_CHARS;
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
