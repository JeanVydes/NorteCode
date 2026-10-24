//! Interactive terminal input with a line editor pinned below the model output.
//!
//! While a turn streams, typed text stays in the input line and never leaks
//! into the answer. Pressing Enter sends the line to the caller, which steers
//! it into the next turn boundary. The terminal keeps a scroll region above the
//! footer, so streamed output scrolls without touching the pinned rows: the
//! running session totals, the dimmed command suggestion and the input line
//! itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One item in the session's todo list, as the footer renders it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TodoItem {
    /// What the item says, as the model wrote it.
    pub(crate) content: String,
    /// How far along the item is.
    pub(crate) status: TodoStatus,
}

/// The state of one todo item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TodoStatus {
    /// Not started.
    Pending,
    /// Being worked on.
    InProgress,
    /// Done.
    Completed,
}

impl TodoStatus {
    /// The marker the `todo` tool uses for this state.
    fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Completed => "[x]",
        }
    }
}

/// The pinned text above the input line.
///
/// The renderer owns the session totals; the input editor owns the terminal.
/// Both share this handle: the renderer replaces the text, the editor redraws
/// it when it changes.
#[derive(Default)]
pub(crate) struct Pinned {
    inner: Mutex<PinnedInner>,
    dirty: AtomicBool,
    resized: AtomicBool,
}

#[derive(Default)]
struct PinnedInner {
    session: String,
    todos: Vec<TodoItem>,
}

impl Pinned {
    /// A shared handle for the input editor and the event renderer.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Replaces the session status line.
    pub(crate) fn set_session(&self, text: String) {
        let mut inner = self.lock();
        if inner.session != text {
            inner.session = text;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Replaces the todo widget. An empty list clears it.
    pub(crate) fn set_todos(&self, todos: Vec<TodoItem>) {
        let mut inner = self.lock();
        if inner.todos != todos {
            inner.todos = todos;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// A snapshot of the todo list, in the model's order.
    #[cfg(test)]
    pub(crate) fn todos(&self) -> Vec<TodoItem> {
        self.lock().todos.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PinnedInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn snapshot(&self) -> (String, Vec<TodoItem>) {
        let inner = self.lock();
        (inner.session.clone(), inner.todos.clone())
    }

    fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    /// Notes that the terminal changed size and the renderer must forget the
    /// column it thought it was on.
    pub(crate) fn mark_resized(&self) {
        self.resized.store(true, Ordering::Release);
    }

    /// Consumes the resize notice, if one is pending.
    pub(crate) fn take_resized(&self) -> bool {
        self.resized.swap(false, Ordering::Acquire)
    }
}

#[cfg(unix)]
mod unix {
    use std::collections::BTreeSet;
    use std::fmt::Write as _;
    use std::io::{self, IsTerminal, Write};
    use std::os::fd::AsRawFd;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use termios::{
        ECHO, ECHOE, ECHOK, ECHONL, ICANON, ICRNL, IXON, TCSANOW, Termios, VMIN, VTIME, tcsetattr,
    };
    use tokio::sync::mpsc;

    use super::{Pinned, TodoItem, TodoStatus};
    use crate::interaction::{CommandSuggestion, command_suggestion};
    use crate::style::Style;

    /// Rows reserved below the scroll region: todo widget, session status,
    /// command suggestion and the input line.
    const FOOTER_ROWS: u16 = 4;

    /// How long a drag must pause before its last size is applied.
    const RESIZE_QUIET: Duration = Duration::from_millis(80);

    /// The longest a drag may postpone applying a size.
    const RESIZE_MAX_DELAY: Duration = Duration::from_millis(150);

    /// Coalesces the stream of resize events a terminal drag produces.
    ///
    /// Resetting the scroll region on every step duplicates and breaks the
    /// transcript, so the editor keeps the latest size and applies it after a
    /// short pause. A maximum delay keeps a slow, continuous drag responsive.
    #[derive(Debug, Default)]
    struct ResizeDebounce {
        pending: Option<(u16, u16)>,
        deadline: Option<Instant>,
        started: Option<Instant>,
    }

    impl ResizeDebounce {
        /// Records the latest size and extends the quiet period.
        fn note(&mut self, width: u16, height: u16, now: Instant) {
            self.pending = Some((width, height.max(FOOTER_ROWS + 1)));
            self.deadline = Some(now + RESIZE_QUIET);
            self.started.get_or_insert(now);
        }

        /// Returns the size to apply once the drag has paused, if any.
        fn take_due(&mut self, now: Instant) -> Option<(u16, u16)> {
            let due = self.pending.is_some_and(|_| {
                self.deadline.is_some_and(|deadline| now >= deadline)
                    || self
                        .started
                        .is_some_and(|started| now.duration_since(started) >= RESIZE_MAX_DELAY)
            });
            if !due {
                return None;
            }
            self.deadline = None;
            self.started = None;
            self.pending.take()
        }
    }

    /// A running input line editor and its channel of completed lines.
    pub(crate) struct InteractiveInput {
        lines: mpsc::UnboundedReceiver<Option<String>>,
        _terminal: TerminalGuard,
    }

    impl InteractiveInput {
        /// Starts the editor on the current terminal.
        pub(crate) fn start(style: Style, pinned: Arc<Pinned>) -> Result<Self, String> {
            if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
                return Err("interactive input needs a terminal on stdin and stderr".to_owned());
            }
            let fd = io::stdin().as_raw_fd();
            let original = Termios::from_fd(fd)
                .map_err(|error| format!("could not read the terminal settings: {error}"))?;
            let mut edited = original;
            edited.c_lflag &= !(ICANON | ECHO | ECHOE | ECHOK | ECHONL);
            edited.c_iflag &= !(ICRNL | IXON);
            edited.c_cc[VMIN] = 1;
            edited.c_cc[VTIME] = 0;
            tcsetattr(fd, TCSANOW, &edited).map_err(|error| {
                format!("could not switch the terminal to line editing: {error}")
            })?;

            let (cols, rows) = match crossterm::terminal::size() {
                Ok(size) => size,
                Err(error) => {
                    let _ = tcsetattr(fd, TCSANOW, &original);
                    return Err(format!("could not measure the terminal: {error}"));
                }
            };
            if rows < FOOTER_ROWS + 1 {
                let _ = tcsetattr(fd, TCSANOW, &original);
                return Err("the terminal is too small for the input line".to_owned());
            }
            if let Err(error) = enter_terminal(cols, rows) {
                let _ = tcsetattr(fd, TCSANOW, &original);
                return Err(error);
            }

            let (sender, lines) = mpsc::unbounded_channel();
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = Arc::clone(&stop);
            let handle = match thread::Builder::new()
                .name("norte-input".to_owned())
                .spawn(move || run_editor(style, cols, rows, &pinned, &sender, &thread_stop))
            {
                Ok(handle) => handle,
                Err(error) => {
                    leave_terminal(rows);
                    let _ = tcsetattr(fd, TCSANOW, &original);
                    return Err(format!("could not start the input reader: {error}"));
                }
            };

            Ok(Self {
                lines,
                _terminal: TerminalGuard {
                    original,
                    stop,
                    handle: Some(handle),
                },
            })
        }

        /// The channel of completed lines. `None` means end of input.
        pub(crate) fn lines(&mut self) -> &mut mpsc::UnboundedReceiver<Option<String>> {
            &mut self.lines
        }
    }

    struct TerminalGuard {
        original: Termios,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            leave_terminal(crossterm::terminal::size().map_or(24, |(_, rows)| rows));
            let fd = io::stdin().as_raw_fd();
            let _ = tcsetattr(fd, TCSANOW, &self.original);
        }
    }

    /// The text being edited and the position of the insertion point.
    #[derive(Debug, Default)]
    struct LineBuffer {
        chars: Vec<char>,
        cursor: usize,
    }

    impl LineBuffer {
        fn insert(&mut self, character: char) {
            self.chars.insert(self.cursor, character);
            self.cursor += 1;
        }

        fn insert_text(&mut self, text: &str) {
            let mut chars = text.chars().peekable();
            while let Some(character) = chars.next() {
                match character {
                    '\r' => {
                        if chars.peek() == Some(&'\n') {
                            chars.next();
                        }
                        self.insert('\n');
                    }
                    '\n' => self.insert('\n'),
                    '\t' => {
                        for _ in 0..4 {
                            self.insert(' ');
                        }
                    }
                    control if control.is_control() => {}
                    other => self.insert(other),
                }
            }
        }

        fn backspace(&mut self) -> bool {
            if self.cursor == 0 {
                return false;
            }
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            true
        }

        fn delete(&mut self) -> bool {
            if self.cursor >= self.chars.len() {
                return false;
            }
            self.chars.remove(self.cursor);
            true
        }

        fn left(&mut self) -> bool {
            if self.cursor == 0 {
                return false;
            }
            self.cursor -= 1;
            true
        }

        fn right(&mut self) -> bool {
            if self.cursor >= self.chars.len() {
                return false;
            }
            self.cursor += 1;
            true
        }

        fn home(&mut self) -> bool {
            let moved = self.cursor != 0;
            self.cursor = 0;
            moved
        }

        fn end(&mut self) -> bool {
            let moved = self.cursor != self.chars.len();
            self.cursor = self.chars.len();
            moved
        }

        fn kill_to_start(&mut self) -> bool {
            if self.cursor == 0 {
                return false;
            }
            self.chars.drain(..self.cursor);
            self.cursor = 0;
            true
        }

        fn kill_to_end(&mut self) -> bool {
            if self.cursor >= self.chars.len() {
                return false;
            }
            self.chars.truncate(self.cursor);
            true
        }

        fn kill_word(&mut self) -> bool {
            if self.cursor == 0 {
                return false;
            }
            let mut start = self.cursor;
            while start > 0 && self.chars[start - 1].is_whitespace() {
                start -= 1;
            }
            while start > 0 && !self.chars[start - 1].is_whitespace() {
                start -= 1;
            }
            self.chars.drain(start..self.cursor);
            self.cursor = start;
            true
        }

        fn clear(&mut self) {
            self.chars.clear();
            self.cursor = 0;
        }

        fn replace(&mut self, text: &str) {
            self.chars = text.chars().collect();
            self.cursor = self.chars.len();
        }

        fn text(&self) -> String {
            self.chars.iter().collect()
        }

        fn is_empty(&self) -> bool {
            self.chars.is_empty()
        }
    }

    fn run_editor(
        style: Style,
        mut cols: u16,
        mut rows: u16,
        pinned: &Pinned,
        sender: &mpsc::UnboundedSender<Option<String>>,
        stop: &AtomicBool,
    ) {
        let mut buffer = LineBuffer::default();
        let mut history: Vec<String> = Vec::new();
        let mut history_pos: Option<usize> = None;
        let mut draft = String::new();

        let suggestion = command_suggestion(&buffer.text(), buffer.cursor);
        draw_input(style, cols, rows, &buffer, suggestion.as_ref());
        draw_footer(style, cols, rows, pinned, suggestion.as_ref());
        let mut resize = ResizeDebounce::default();
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => {
                    let mut redraw = false;
                    match event::read() {
                        Ok(Event::Key(key))
                            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                        {
                            match handle_key(
                                key,
                                &mut buffer,
                                &mut history,
                                &mut history_pos,
                                &mut draft,
                                sender,
                            ) {
                                KeyOutcome::Continue => redraw = true,
                                KeyOutcome::Exit => break,
                            }
                        }
                        Ok(Event::Paste(text)) => {
                            buffer.insert_text(&text);
                            redraw = true;
                        }
                        Ok(Event::Resize(width, height)) => {
                            resize.note(width, height, Instant::now());
                        }
                        _ => {}
                    }
                    if redraw {
                        let suggestion = command_suggestion(&buffer.text(), buffer.cursor);
                        draw_input(style, cols, rows, &buffer, suggestion.as_ref());
                        draw_footer(style, cols, rows, pinned, suggestion.as_ref());
                    }
                }
                Ok(false) => {}
                Err(_) => {
                    let _ = sender.send(None);
                    break;
                }
            }
            if let Some((width, height)) = resize.take_due(Instant::now()) {
                let old_rows = rows;
                cols = width;
                rows = height;
                resize_terminal(rows, old_rows);
                pinned.mark_resized();
                let suggestion = command_suggestion(&buffer.text(), buffer.cursor);
                draw_footer(style, cols, rows, pinned, suggestion.as_ref());
                draw_input(style, cols, rows, &buffer, suggestion.as_ref());
            }
            if pinned.take_dirty() {
                let suggestion = command_suggestion(&buffer.text(), buffer.cursor);
                draw_footer(style, cols, rows, pinned, suggestion.as_ref());
            }
        }
    }

    enum KeyOutcome {
        Continue,
        Exit,
    }

    #[allow(clippy::too_many_lines)]
    fn handle_key(
        key: KeyEvent,
        buffer: &mut LineBuffer,
        history: &mut Vec<String>,
        history_pos: &mut Option<usize>,
        draft: &mut String,
        sender: &mpsc::UnboundedSender<Option<String>>,
    ) -> KeyOutcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter if alt || key.modifiers.contains(KeyModifiers::SHIFT) => {
                buffer.insert('\n');
                KeyOutcome::Continue
            }
            KeyCode::Enter => {
                let line = buffer.text();
                if !line.trim().is_empty() {
                    if history.last() != Some(&line) {
                        history.push(line.clone());
                    }
                    history_pos.take();
                    draft.clear();
                }
                if sender.send(Some(line)).is_err() {
                    return KeyOutcome::Exit;
                }
                buffer.clear();
                KeyOutcome::Continue
            }
            KeyCode::Char('d') if control => {
                if buffer.is_empty() {
                    let _ = sender.send(None);
                    return KeyOutcome::Exit;
                }
                KeyOutcome::Continue
            }
            KeyCode::Char('c' | 'l') if control => KeyOutcome::Continue,
            KeyCode::Char('a') if control => {
                buffer.home();
                KeyOutcome::Continue
            }
            KeyCode::Char('e') if control => {
                buffer.end();
                KeyOutcome::Continue
            }
            KeyCode::Char('u') if control => {
                buffer.kill_to_start();
                KeyOutcome::Continue
            }
            KeyCode::Char('k') if control => {
                buffer.kill_to_end();
                KeyOutcome::Continue
            }
            KeyCode::Char('w') if control => {
                buffer.kill_word();
                KeyOutcome::Continue
            }
            KeyCode::Char('b') if control => {
                buffer.left();
                KeyOutcome::Continue
            }
            KeyCode::Char('f') if control => {
                buffer.right();
                KeyOutcome::Continue
            }
            KeyCode::Char('p') if control => {
                history_previous(buffer, history, history_pos, draft);
                KeyOutcome::Continue
            }
            KeyCode::Char('n') if control => {
                history_next(buffer, history, history_pos, draft);
                KeyOutcome::Continue
            }
            KeyCode::Char(character) if !control && !alt => {
                buffer.insert(character);
                *history_pos = None;
                KeyOutcome::Continue
            }
            KeyCode::Backspace => {
                buffer.backspace();
                KeyOutcome::Continue
            }
            KeyCode::Delete => {
                buffer.delete();
                KeyOutcome::Continue
            }
            KeyCode::Left => {
                buffer.left();
                KeyOutcome::Continue
            }
            KeyCode::Right => {
                buffer.right();
                KeyOutcome::Continue
            }
            KeyCode::Home => {
                buffer.home();
                KeyOutcome::Continue
            }
            KeyCode::End => {
                buffer.end();
                KeyOutcome::Continue
            }
            KeyCode::Up => {
                history_previous(buffer, history, history_pos, draft);
                KeyOutcome::Continue
            }
            KeyCode::Down => {
                history_next(buffer, history, history_pos, draft);
                KeyOutcome::Continue
            }
            KeyCode::Tab => {
                match command_suggestion(&buffer.text(), buffer.cursor) {
                    Some(suggestion) if !suggestion.completion.is_empty() => {
                        buffer.insert_text(&suggestion.completion);
                        *history_pos = None;
                    }
                    _ => {
                        for _ in 0..4 {
                            buffer.insert(' ');
                        }
                    }
                }
                KeyOutcome::Continue
            }
            KeyCode::Esc => {
                buffer.clear();
                *history_pos = None;
                KeyOutcome::Continue
            }
            _ => KeyOutcome::Continue,
        }
    }

    fn history_previous(
        buffer: &mut LineBuffer,
        history: &[String],
        position: &mut Option<usize>,
        draft: &mut String,
    ) {
        if history.is_empty() {
            return;
        }
        match *position {
            None => {
                *draft = buffer.text();
                *position = Some(history.len() - 1);
            }
            Some(index) if index > 0 => *position = Some(index - 1),
            Some(_) => return,
        }
        if let Some(index) = *position {
            buffer.replace(&history[index]);
        }
    }

    fn history_next(
        buffer: &mut LineBuffer,
        history: &[String],
        position: &mut Option<usize>,
        draft: &mut String,
    ) {
        let Some(index) = *position else {
            return;
        };
        if index + 1 < history.len() {
            *position = Some(index + 1);
            buffer.replace(&history[index + 1]);
        } else {
            *position = None;
            let text = std::mem::take(draft);
            buffer.replace(&text);
        }
    }

    /// The bottom row of the scroll region; the footer lives below it.
    fn scroll_bottom(rows: u16) -> u16 {
        rows.saturating_sub(FOOTER_ROWS).max(1)
    }

    fn status_row(rows: u16) -> u16 {
        rows.saturating_sub(2).max(1)
    }

    fn suggestion_row(rows: u16) -> u16 {
        rows.saturating_sub(1).max(1)
    }

    fn todo_row(rows: u16) -> u16 {
        rows.saturating_sub(3).max(1)
    }

    fn enter_terminal(cols: u16, rows: u16) -> Result<(), String> {
        let mut out = io::stderr().lock();
        // Hide the cursor, enable bracketed paste, and push the keyboard
        // protocol. The last one is what makes a modified Enter distinguishable:
        // a terminal that does not report it sends the same carriage return for
        // Enter and Shift+Enter, so the newline branch never fires. A terminal
        // that does not support the sequence ignores it, and the pop on the way
        // out is then a no-op.
        write!(out, "\x1b[?25l\x1b[?2004h\x1b[>1u").map_err(|error| error.to_string())?;
        // The startup and `--debug` blocks are printed before this point. Reset
        // the scroll region, drop to the last row and scroll the footer height
        // so those rows are blank; otherwise the first streamed lines would
        // overwrite the last debug line.
        write!(out, "\x1b[r\x1b[{rows};1H").map_err(|error| error.to_string())?;
        for _ in 0..FOOTER_ROWS {
            writeln!(out).map_err(|error| error.to_string())?;
        }
        set_region(&mut out, cols, rows).map_err(|error| error.to_string())?;
        out.flush().map_err(|error| error.to_string())
    }

    fn leave_terminal(rows: u16) {
        let mut out = io::stderr().lock();
        let _ = write!(out, "\x1b[<u\x1b[?2004l\x1b[r");
        // Only the footer rows: the region still holds the final summary.
        for row in scroll_bottom(rows).saturating_add(1)..=rows.max(1) {
            let _ = write!(out, "\x1b[{row};1H\x1b[2K");
        }
        let _ = write!(out, "\x1b[{};1H\x1b[?25h", rows.max(1));
        let _ = out.flush();
    }

    /// Builds the frame that moves the scroll region after a resize.
    ///
    /// The frame is assembled first and written once: the renderer writes to
    /// the same terminal from another thread, and a cursor move split across
    /// writes is exactly how streamed text ends up duplicated or overwritten.
    fn resize_frame(rows: u16, old_rows: u16) -> String {
        let bottom = scroll_bottom(rows);
        let mut frame = String::from("\x1b[r");

        // Clear each row that was, or is now, part of the footer. A set keeps
        // an unchanged size from emitting the same clear twice.
        let mut cleared = BTreeSet::new();
        for row in scroll_bottom(old_rows).saturating_add(1)..=old_rows.max(1) {
            if row <= rows.max(1) {
                cleared.insert(row);
            }
        }
        for row in bottom.saturating_add(1)..=rows.max(1) {
            cleared.insert(row);
        }
        for row in cleared {
            let _ = write!(frame, "\x1b[{row};1H\x1b[2K");
        }

        // Release the old region above, then set the new one and scroll one
        // fresh line at its bottom. Scrolling (rather than jumping onto the
        // last line) preserves whatever the terminal reflowed instead of
        // writing over it.
        let _ = write!(frame, "\x1b[{bottom};1H\x1b[1;{bottom}r\r\n");
        frame
    }

    fn resize_terminal(rows: u16, old_rows: u16) {
        write_frame(&resize_frame(rows, old_rows));
    }

    /// Writes one complete terminal frame with a single flush.
    fn write_frame(frame: &str) {
        let mut out = io::stderr().lock();
        let _ = out.write_all(frame.as_bytes());
        let _ = out.flush();
    }

    fn set_region(out: &mut impl Write, cols: u16, rows: u16) -> io::Result<()> {
        let _ = cols;
        let bottom = scroll_bottom(rows);
        write!(out, "\x1b[1;{bottom}r\x1b[{bottom};1H")
    }

    /// Draws the pinned todo widget, session line and command suggestion.
    ///
    /// All rows are below the scroll region, so streamed output never touches
    /// them. The cursor is saved and restored so the main thread's output
    /// position is preserved.
    fn draw_footer(
        style: Style,
        cols: u16,
        rows: u16,
        pinned: &Pinned,
        suggestion: Option<&CommandSuggestion>,
    ) {
        let (session, todos) = pinned.snapshot();
        let width = usize::from(cols).saturating_sub(2);
        let suggestion_text = suggestion.map_or_else(String::new, |suggestion| {
            style.faint(suggestion.hint.clone())
        });
        let todo_text = todo_line(style, &todos, width);
        let frame = format!(
            "\x1b7\x1b[{};1H\x1b[2K{}\x1b[{};1H\x1b[2K{}\x1b[{};1H\x1b[2K{}\x1b8",
            todo_row(rows),
            clip_visible(&todo_text, width),
            status_row(rows),
            clip_visible(&session, width),
            suggestion_row(rows),
            clip_visible(&suggestion_text, width)
        );
        write_frame(&frame);
    }

    /// Builds the one-line todo widget: progress, the current item first, then
    /// pending items that fit. Completed items are represented by the count,
    /// and a `+N` says how many actionable items did not fit.
    fn todo_line(style: Style, todos: &[TodoItem], max: usize) -> String {
        if todos.is_empty() {
            return String::new();
        }
        let done = todos
            .iter()
            .filter(|todo| todo.status == TodoStatus::Completed)
            .count();
        let mut plain = format!("todo {done}/{}", todos.len());
        let mut line = format!(
            "{} {}",
            style.faint("todo"),
            style.cyan(format!("{done}/{}", todos.len()))
        );
        let active: Vec<&TodoItem> = todos
            .iter()
            .filter(|todo| todo.status == TodoStatus::InProgress)
            .chain(
                todos
                    .iter()
                    .filter(|todo| todo.status == TodoStatus::Pending),
            )
            .collect();
        if active.is_empty() {
            let _ = write!(line, " · {}", style.green("all done"));
            return line;
        }
        let mut shown = 0;
        for item in &active {
            let marker = item.status.marker();
            let remaining = active.len().saturating_sub(shown + 1);
            let reserve = if remaining > 0 {
                format!(" · +{remaining}").chars().count()
            } else {
                0
            };
            let prefix = format!(" · {marker} ");
            let available = max.saturating_sub(plain.chars().count() + reserve);
            if available <= prefix.chars().count() {
                break;
            }
            let (mut content, truncated) =
                single_line(&item.content, available - prefix.chars().count());
            if truncated && shown > 0 {
                break;
            }
            if truncated {
                content.pop();
                content.push('…');
            }
            let piece = format!("{prefix}{content}");
            plain.push_str(&piece);
            line.push_str(&style.faint(" · "));
            line.push_str(&match item.status {
                TodoStatus::Pending => style.faint(marker),
                TodoStatus::InProgress => style.yellow(marker),
                TodoStatus::Completed => style.green(marker),
            });
            line.push(' ');
            line.push_str(&match item.status {
                TodoStatus::InProgress => style.bold(&content),
                _ => style.dim(&content),
            });
            shown += 1;
        }
        let hidden = active.len().saturating_sub(shown);
        if shown == 0 {
            let suffix = format!(" · +{hidden}");
            if plain.chars().count() + suffix.chars().count() <= max {
                line.push_str(&style.faint(suffix));
            }
        } else if hidden > 0 {
            line.push_str(&style.faint(format!(" · +{hidden}")));
        }
        line
    }

    /// Folds a todo item onto one footer line, keeping at most `max`
    /// characters and reporting whether anything was dropped.
    ///
    /// The content is model text, so newlines, tabs and control escapes must
    /// not reach the terminal as layout instructions; styling escapes it, and
    /// this collapses whitespace.
    fn single_line(text: &str, max: usize) -> (String, bool) {
        let mut out = String::new();
        let mut used = 0;
        let mut truncated = false;
        'words: for word in text.split_whitespace() {
            if used == max {
                truncated = true;
                break;
            }
            if used > 0 {
                out.push(' ');
                used += 1;
                if used == max {
                    truncated = true;
                    break;
                }
            }
            for character in word.chars() {
                if used == max {
                    truncated = true;
                    break 'words;
                }
                out.push(character);
                used += 1;
            }
        }
        (out, truncated)
    }

    /// Clips a styled line to `max` visible columns, keeping ANSI escapes.
    fn clip_visible(text: &str, max: usize) -> String {
        let mut out = String::new();
        let mut visible = 0usize;
        let mut chars = text.chars();
        while let Some(character) = chars.next() {
            if character == '\x1b' {
                out.push(character);
                for next in chars.by_ref() {
                    out.push(next);
                    if next == 'm' {
                        break;
                    }
                }
                continue;
            }
            if visible >= max {
                out.push('…');
                out.push_str("\x1b[0m");
                break;
            }
            out.push(character);
            visible += 1;
        }
        out
    }

    fn draw_input(
        style: Style,
        cols: u16,
        rows: u16,
        buffer: &LineBuffer,
        suggestion: Option<&CommandSuggestion>,
    ) {
        let prompt = format!("{} ", style.cyan("›"));
        let prompt_width = 2usize;
        let available = usize::from(cols).saturating_sub(prompt_width + 1).max(1);
        let offset = buffer.cursor.saturating_sub(available.saturating_sub(1));
        let end = (offset + available).min(buffer.chars.len());
        let mut visible = String::new();
        for index in offset..end {
            let character = buffer.chars[index];
            if index == buffer.cursor {
                visible.push_str("\x1b[7m");
                visible.push(character);
                visible.push_str("\x1b[27m");
            } else {
                visible.push(character);
            }
        }
        if buffer.cursor >= buffer.chars.len() {
            visible.push_str("\x1b[7m \x1b[27m");
            if let Some(suggestion) = suggestion {
                if !suggestion.completion.is_empty() {
                    let typed = end.saturating_sub(offset);
                    let room = available.saturating_sub(typed + 1);
                    if room > 0 {
                        visible.push_str(&clip_visible(
                            &style.faint(suggestion.completion.clone()),
                            room,
                        ));
                    }
                }
            }
        }
        let row = rows.max(1);
        write_frame(&format!("\x1b7\x1b[{row};1H\x1b[2K{prompt}{visible}\x1b8"));
    }

    #[cfg(test)]
    mod tests {
        use std::time::{Duration, Instant};

        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use super::{
            FOOTER_ROWS, LineBuffer, ResizeDebounce, TodoItem, TodoStatus, clip_visible,
            handle_key, resize_frame, scroll_bottom, status_row, suggestion_row, todo_line,
            todo_row,
        };

        #[test]
        fn resize_debounce_waits_for_the_drag_to_pause() {
            let start = Instant::now();
            let mut resize = ResizeDebounce::default();
            resize.note(80, 24, start);
            assert_eq!(resize.take_due(start + Duration::from_millis(79)), None);
            assert_eq!(
                resize.take_due(start + Duration::from_millis(80)),
                Some((80, 24))
            );
            assert_eq!(resize.take_due(start + Duration::from_millis(200)), None);
        }

        #[test]
        fn resize_debounce_applies_a_long_drag_without_waiting_for_it_to_end() {
            let start = Instant::now();
            let mut resize = ResizeDebounce::default();
            resize.note(80, 24, start);
            resize.note(100, 30, start + Duration::from_millis(100));
            // The quiet period has not elapsed since the last event...
            assert_eq!(resize.take_due(start + Duration::from_millis(149)), None);
            // ...but the maximum delay since the first event wins.
            assert_eq!(
                resize.take_due(start + Duration::from_millis(150)),
                Some((100, 30))
            );
        }

        #[test]
        fn resize_debounce_keeps_room_for_the_footer() {
            let start = Instant::now();
            let mut resize = ResizeDebounce::default();
            resize.note(80, 1, start);
            assert_eq!(
                resize.take_due(start + Duration::from_millis(200)),
                Some((80, FOOTER_ROWS + 1))
            );
        }

        #[test]
        fn resize_frame_clears_each_footer_row_once() {
            let frame = resize_frame(24, 24);
            assert!(frame.starts_with("\x1b[r"));
            for row in 21..=24 {
                assert_eq!(
                    frame.matches(&format!("\x1b[{row};1H\x1b[2K")).count(),
                    1,
                    "row {row} should be cleared once"
                );
            }
            assert!(frame.ends_with("\x1b[20;1H\x1b[1;20r\r\n"));
        }

        #[test]
        fn resize_frame_clears_old_and_new_footer_rows() {
            let grown = resize_frame(30, 24);
            for row in [21, 22, 23, 24, 27, 28, 29, 30] {
                assert_eq!(
                    grown.matches(&format!("\x1b[{row};1H\x1b[2K")).count(),
                    1,
                    "row {row} should be cleared once"
                );
            }
            for row in 25..=26 {
                assert!(
                    !grown.contains(&format!("\x1b[{row};1H\x1b[2K")),
                    "scroll row {row} must be preserved"
                );
            }
            assert!(grown.ends_with("\x1b[26;1H\x1b[1;26r\r\n"));

            let shrunk = resize_frame(24, 30);
            for row in 21..=24 {
                assert_eq!(
                    shrunk.matches(&format!("\x1b[{row};1H\x1b[2K")).count(),
                    1,
                    "row {row} should be cleared once"
                );
            }
            for row in 28..=30 {
                assert!(
                    !shrunk.contains(&format!("\x1b[{row};1H\x1b[2K")),
                    "rows beyond the new height must not be addressed"
                );
            }
            assert!(shrunk.ends_with("\x1b[20;1H\x1b[1;20r\r\n"));
        }

        #[test]
        fn footer_rows_stay_below_the_scroll_region() {
            assert_eq!(scroll_bottom(24), 20);
            assert_eq!(todo_row(24), 21);
            assert_eq!(status_row(24), 22);
            assert_eq!(suggestion_row(24), 23);
            // A terminal at the minimum height still yields valid rows.
            assert_eq!(scroll_bottom(5), 1);
            assert_eq!(todo_row(5), 2);
            assert_eq!(status_row(5), 3);
            assert_eq!(suggestion_row(5), 4);
        }

        fn item(content: &str, status: TodoStatus) -> TodoItem {
            TodoItem {
                content: content.to_owned(),
                status,
            }
        }

        #[test]
        fn the_todo_widget_shows_progress_and_the_current_item_first() {
            let todos = vec![
                item("done", TodoStatus::Completed),
                item("later", TodoStatus::Pending),
                item("now", TodoStatus::InProgress),
            ];
            assert_eq!(
                todo_line(crate::style::Style::plain(), &todos, 80),
                "todo 1/3 · [~] now · [ ] later"
            );
        }

        #[test]
        fn the_todo_widget_counts_completed_items_and_says_when_all_are_done() {
            let todos = vec![
                item("one", TodoStatus::Completed),
                item("two", TodoStatus::Completed),
            ];
            assert_eq!(
                todo_line(crate::style::Style::plain(), &todos, 80),
                "todo 2/2 · all done"
            );
            assert_eq!(todo_line(crate::style::Style::plain(), &[], 80), "");
        }

        #[test]
        fn the_todo_widget_reports_items_that_did_not_fit() {
            let todos = vec![
                item("first", TodoStatus::Pending),
                item("second", TodoStatus::Pending),
            ];
            assert_eq!(
                todo_line(crate::style::Style::plain(), &todos, 30),
                "todo 0/2 · [ ] first · +1"
            );
            assert_eq!(
                todo_line(crate::style::Style::plain(), &todos, 20),
                "todo 0/2 · +2"
            );
        }

        #[test]
        fn the_todo_widget_folds_untrusted_content_onto_one_line() {
            let todos = vec![item("first\nsecond\tthird", TodoStatus::Pending)];
            let line = todo_line(crate::style::Style::plain(), &todos, 80);
            assert_eq!(line, "todo 0/1 · [ ] first second third");
            assert!(!line.contains('\n'));
            assert!(!line.contains('\t'));
        }

        #[test]
        fn the_todo_widget_truncates_a_long_first_item_to_the_footer() {
            let todos = vec![item(
                "a very long item that does not fit on one line",
                TodoStatus::Pending,
            )];
            let line = todo_line(crate::style::Style::plain(), &todos, 24);
            assert!(line.starts_with("todo 0/1 · [ ] "), "{line}");
            assert!(line.ends_with('…'), "{line}");
            assert_eq!(line.chars().count(), 24, "{line}");
        }

        #[test]
        fn arrows_move_the_insertion_point_without_reordering_text() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("abc");
            assert!(buffer.left());
            assert!(buffer.left());
            buffer.insert('X');
            assert_eq!(buffer.text(), "aXbc");
            assert!(buffer.home());
            buffer.insert('0');
            assert_eq!(buffer.text(), "0aXbc");
            assert!(buffer.end());
            buffer.insert('!');
            assert_eq!(buffer.text(), "0aXbc!");
        }

        #[test]
        fn backspace_and_delete_respect_the_cursor() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("abcd");
            assert!(buffer.left());
            assert!(buffer.backspace());
            assert_eq!(buffer.text(), "abd");
            assert!(buffer.home());
            assert!(buffer.delete());
            assert_eq!(buffer.text(), "bd");
            buffer.insert_text("x");
            assert!(buffer.right());
            assert!(buffer.kill_to_end());
            assert_eq!(buffer.text(), "xb");
            buffer.insert_text("rest");
            assert!(buffer.kill_to_start());
            assert!(buffer.is_empty());
        }

        #[test]
        fn control_word_editing_does_not_eat_the_prompt() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("hello brave world");
            assert!(buffer.kill_word());
            assert_eq!(buffer.text(), "hello brave ");
            assert!(buffer.kill_to_start());
            assert_eq!(buffer.text(), "");
        }

        #[test]
        fn pasted_newlines_become_line_breaks() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("one\r\ntwo\tthree");
            assert_eq!(buffer.text(), "one\ntwo    three");
        }

        #[test]
        fn shift_enter_breaks_the_line_and_enter_submits() {
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut history = Vec::new();
            let mut history_pos = None;
            let mut draft = String::new();
            let mut buffer = LineBuffer::default();
            buffer.insert_text("first");

            // Shift+Enter must insert a newline, not send: the terminal only
            // reports the modifier when the keyboard protocol is pushed, which
            // is why this branch used to be unreachable.
            handle_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
                &mut buffer,
                &mut history,
                &mut history_pos,
                &mut draft,
                &sender,
            );
            assert_eq!(buffer.text(), "first\n");
            assert!(
                receiver.try_recv().is_err(),
                "shift+enter must not submit the message"
            );

            buffer.insert_text("second");
            handle_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut buffer,
                &mut history,
                &mut history_pos,
                &mut draft,
                &sender,
            );
            assert_eq!(buffer.text(), "", "a plain enter clears the buffer");
            assert_eq!(
                receiver
                    .try_recv()
                    .expect("the message was sent")
                    .as_deref(),
                Some("first\nsecond"),
                "a plain enter submits the whole multi-line message"
            );
        }

        #[test]
        fn tab_accepts_command_and_argument_suggestions() {
            let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut history = Vec::new();
            let mut history_pos = None;
            let mut draft = String::new();
            let mut buffer = LineBuffer::default();
            buffer.insert_text("/rea");
            handle_key(
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                &mut buffer,
                &mut history,
                &mut history_pos,
                &mut draft,
                &sender,
            );
            assert_eq!(buffer.text(), "/reasoning");

            buffer.insert_text(" ");
            handle_key(
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                &mut buffer,
                &mut history,
                &mut history_pos,
                &mut draft,
                &sender,
            );
            assert_eq!(buffer.text(), "/reasoning auto");
        }

        #[test]
        fn footer_clipping_counts_visible_columns_and_keeps_styles() {
            // Six visible characters plus two escape sequences.
            let text = "\x1b[2mabcdef\x1b[0m";
            assert_eq!(clip_visible(text, 10), text);
            let clipped = clip_visible(text, 3);
            assert_eq!(clipped, "\x1b[2mabc…\x1b[0m");
            assert_eq!(clip_visible("plain", 5), "plain");
        }
    }
}

#[cfg(unix)]
pub(crate) use unix::InteractiveInput;

#[cfg(not(unix))]
mod fallback {
    use std::io;
    use std::sync::Arc;

    use tokio::sync::mpsc;

    use crate::style::Style;

    /// A plain line reader for platforms without the Unix terminal editor.
    pub(crate) struct InteractiveInput {
        lines: mpsc::UnboundedReceiver<Option<String>>,
    }

    impl InteractiveInput {
        /// Starts a blocking line reader on stdin.
        pub(crate) fn start(_style: Style, _pinned: Arc<super::Pinned>) -> Result<Self, String> {
            let (sender, lines) = mpsc::unbounded_channel();
            std::thread::spawn(move || {
                loop {
                    let mut line = String::new();
                    match io::stdin().read_line(&mut line) {
                        Ok(0) | Err(_) => {
                            let _ = sender.send(None);
                            break;
                        }
                        Ok(_) => {
                            if sender.send(Some(line.trim_end().to_owned())).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            Ok(Self { lines })
        }

        /// The channel of completed lines.
        pub(crate) fn lines(&mut self) -> &mut mpsc::UnboundedReceiver<Option<String>> {
            &mut self.lines
        }
    }
}

#[cfg(not(unix))]
pub(crate) use fallback::InteractiveInput;

#[cfg(test)]
mod pinned_tests {
    use super::{Pinned, TodoItem, TodoStatus};

    #[test]
    fn the_pinned_footer_keeps_the_session_and_todos() {
        let pinned = Pinned::new();
        pinned.set_session("session".to_owned());
        pinned.set_todos(vec![TodoItem {
            content: "task".to_owned(),
            status: TodoStatus::Pending,
        }]);
        let (session, todos) = pinned.snapshot();
        assert_eq!(session, "session");
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].content, "task");
    }
}
