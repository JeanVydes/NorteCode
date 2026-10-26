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

use crate::tabs::SessionTab;

/// A file pasted into the draft, as the message carries it.
///
/// The bytes are not here: a paste carries a path, and the service is what reads
/// the file and files it in the blob store both ends share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attachment {
    /// Where the file is.
    pub(crate) path: String,
    /// Its media type, from its extension.
    pub(crate) mime: String,
    /// What to call it, for the line and for the model.
    pub(crate) name: String,
}

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

impl Default for PinnedInner {
    fn default() -> Self {
        Self {
            session: String::new(),
            todos: Vec::new(),
            tab: SessionTab::Chat,
            debug: Vec::new(),
            pending_steers: Vec::new(),
        }
    }
}

struct PinnedInner {
    session: String,
    todos: Vec<TodoItem>,
    /// Which tab is showing. The conversation is the default because that is
    /// what a client opens on.
    tab: SessionTab,
    /// The startup information, kept to be shown as a tab instead of printed
    /// once and scrolled away.
    debug: Vec<String>,
    /// The messages that reached the running request but are not in the
    /// transcript yet, oldest first.
    ///
    /// A message the human types while a turn runs is not in the history until
    /// the engine applies it, so showing it as sent would claim something that
    /// has not happened, and showing it nowhere would hide that the typing
    /// landed. It waits here instead, above the input, until the engine reports
    /// it applied and the transcript prints it where it belongs.
    pending_steers: Vec<String>,
}
impl Pinned {
    /// A shared handle for the input editor and the event renderer.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Records the messages waiting to reach the running request.
    ///
    /// They are shown above the input until the engine reports them applied,
    /// which is when they start being part of the conversation.
    pub(crate) fn set_pending_steers(&self, steers: Vec<String>) {
        let mut inner = self.lock();
        if inner.pending_steers != steers {
            inner.pending_steers = steers;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// The pending steering messages, oldest first.
    #[cfg(test)]
    pub(crate) fn pending_steers(&self) -> Vec<String> {
        self.lock().pending_steers.clone()
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

    /// The tab that is showing.
    pub(crate) fn tab(&self) -> SessionTab {
        self.lock().tab
    }

    /// Shows another tab, and marks the footer dirty so it repaints.
    pub(crate) fn set_tab(&self, tab: SessionTab) {
        let mut inner = self.lock();
        if inner.tab != tab {
            inner.tab = tab;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Keeps the startup information for the debug tab.
    ///
    /// It used to be printed before the first prompt and scrolled away by the
    /// first answer, so the one moment it was wanted — working out which model
    /// or which path is in play — was the moment it was gone.
    pub(crate) fn set_debug(&self, lines: Vec<String>) {
        let mut inner = self.lock();
        if inner.debug != lines {
            inner.debug = lines;
        }
    }

    /// The startup information, as the debug tab shows it.
    pub(crate) fn debug(&self) -> Vec<String> {
        self.lock().debug.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PinnedInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn snapshot(&self) -> (String, Vec<TodoItem>, SessionTab) {
        let inner = self.lock();
        (inner.session.clone(), inner.todos.clone(), inner.tab)
    }

    /// The messages waiting to reach the running request.
    fn pending(&self) -> Vec<String> {
        self.lock().pending_steers.clone()
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
    use super::Attachment;

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
        ECHO, ECHOE, ECHOK, ECHONL, ICANON, ICRNL, ISIG, IXON, TCSANOW, Termios, VMIN, VTIME,
        tcsetattr,
    };
    use tokio::sync::mpsc;

    use super::{Pinned, TodoItem, TodoStatus};
    use crate::interaction::{CommandSuggestion, command_suggestion};
    use crate::style::Style;
    use crate::tabs::SessionTab;

    /// The most rows the input line may occupy, scrolling inside them.
    ///
    /// A draft rarely needs more; one that does keeps the row the cursor is on
    /// in view rather than pushing the transcript off the screen.
    const INPUT_ROWS: u16 = 6;

    /// Rows reserved below the scroll region.
    ///
    /// The layout is built up from the bottom: the command suggestion, the
    /// session status, the todo widget, the blank row under the todo widget,
    /// and then the input block of up to [`INPUT_ROWS`]. The count has to cover
    /// all of them, and it used to stop at the suggestion row: the region ended
    /// two rows inside the input block, so every keypress cleared two rows of
    /// the transcript to redraw the draft. That is what made the log vanish and
    /// leave a hole while a tool was printing.
    const FOOTER_ROWS: u16 = INPUT_ROWS + 4;

    /// How many rows the waiting steering messages may take above the draft.
    ///
    /// A handful, not all of them: past this the draft itself would have nowhere
    /// to go, and the count beside the last line says how many did not fit.
    const MAX_STEER_ROWS: u16 = 3;

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

    /// What the editor handed the repl: a line, or something a keystroke asked
    /// for that is not a line.
    ///
    /// One channel for both, because they cannot be read at the same time: two
    /// readers would each take `&mut self`, which a `select!` over both refuses
    /// to borrow. Reading them in sequence would be worse than this: a keystroke
    /// would wait behind an unfinished line.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum EditorEvent {
        /// A line was submitted, with the files its draft asked to attach.
        Line(String, Vec<Attachment>),
        /// A keystroke asked for something that is not a line.
        Action(EditorAction),
        /// The editor ended: end of input.
        End,
    }

    /// What a keystroke asked the repl to do, when it is not a line.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum EditorAction {
        /// Escape, on a running turn: stop it.
        CancelTurn,
        /// Ctrl-C, when the session has background jobs: stop them.
        CancelJobs,
        /// Ctrl-G, on a running turn: stop waiting for the call, leave the
        /// process running.
        Detach,
    }

    /// A running input line editor and its channel of lines and actions.
    pub(crate) struct InteractiveInput {
        events: mpsc::UnboundedReceiver<EditorEvent>,
        /// The terminal settings to restore. Absent for a test that drives the
        /// channel directly, which owns no terminal to put back.
        _terminal: Option<TerminalGuard>,
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
            // `ISIG` is the line that makes Ctrl-C work. Without it cleared,
            // the terminal turns Ctrl-C into SIGINT and the editor never sees
            // the key at all: the signal handler and the key reader were racing
            // for the same keystroke, and the handler did not always win.
            edited.c_lflag &= !(ICANON | ECHO | ECHOE | ECHOK | ECHONL | ISIG);
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

            let (sender, events) = mpsc::unbounded_channel();
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
                events,
                _terminal: Some(TerminalGuard {
                    original,
                    stop,
                    handle: Some(handle),
                }),
            })
        }

        /// The next thing the editor handed over.
        ///
        /// `None` means the editor is gone, which is the end of input.
        pub(crate) async fn recv(&mut self) -> Option<EditorEvent> {
            self.events.recv().await
        }

        /// A handle over a channel a test fills, with no terminal behind it.
        #[cfg(test)]
        pub(crate) fn from_events(events: mpsc::UnboundedReceiver<EditorEvent>) -> Self {
            Self {
                events,
                _terminal: None,
            }
        }

        /// The next submitted line, ignoring what keystrokes asked for.
        ///
        /// It is for the prompts that want a line and nothing else — an
        /// approval, a question, a budget limit — where an Escape or a Ctrl-C
        /// has no meaning of its own and waiting for a line is the answer.
        pub(crate) async fn recv_line(&mut self) -> Option<Option<String>> {
            loop {
                match self.events.recv().await {
                    Some(EditorEvent::Line(line, _)) => return Some(Some(line)),
                    Some(EditorEvent::Action(_)) => {}
                    Some(EditorEvent::End) | None => return Some(None),
                }
            }
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
        /// The runs that are shown as a label but carry other text.
        ///
        /// A pasted paragraph is one of these: the line shows `[Copied: …]` and
        /// what is sent is the paragraph. Anything that edits the run removes it
        /// whole, because half a paragraph is not a smaller paragraph — it is a
        /// sentence nobody wrote.
        chips: Vec<Chip>,
    }

    /// A run of the buffer that stands for something else.
    #[derive(Debug)]
    struct Chip {
        /// Where it is in `chars`, which holds what is shown.
        range: std::ops::Range<usize>,
        /// What is sent in its place: the pasted text, or the path of a pasted
        /// file, which is what the model is given.
        original: String,
        /// The attachment this chip asks for, when it stands for a file.
        attachment: Option<Attachment>,
    }

    /// How many characters of a pasted text the label shows.
    const CHIP_LABEL_CHARS: usize = 24;

    /// The shortest paste that is worth hiding behind a label.
    ///
    /// Below this the pasted text is its own label: `[Copied: hello]` is longer
    /// than `hello`, and a chip that saves nothing costs a keystroke to remove.
    const CHIP_MIN_CHARS: usize = 32;

    impl LineBuffer {
        fn insert(&mut self, character: char) {
            self.settle_cursor();
            self.chars.insert(self.cursor, character);
            self.cursor += 1;
        }

        /// A paste, which may be worth hiding behind a label.
        ///
        /// A long one is shown as `[Copied: …]` and travels whole: the line is
        /// for reading, and a paragraph pasted into it buries the draft the
        /// human is writing. What is sent is what was pasted.
        fn insert_paste(&mut self, text: &str) {
            let cleaned = clean_paste(text);
            // A terminal that pastes a file pastes its path — kitty does, and so
            // does a drag-and-drop. That is the only way an image gets here: the
            // clipboard's bytes never reach the application over the wire.
            if let Some(attachment) = image_attachment(&cleaned) {
                let label = format!("[Image: {}]", attachment.name);
                let path = attachment.path.clone();
                self.insert_attachment(&label, path, Some(attachment));
                return;
            }
            // A long path is not a paragraph: it is a path, and a chip says so
            // without pretending the file is attached. A paste too short to be
            // worth a label is its own label.
            if let Some(name) = existing_path_name(&cleaned) {
                let label = format!("[Path: {name}]");
                self.insert_chip(&label, cleaned);
                return;
            }
            if cleaned.chars().count() < CHIP_MIN_CHARS {
                self.insert_text(&cleaned);
                return;
            }
            let head: String = cleaned
                .chars()
                .take(CHIP_LABEL_CHARS)
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let label = format!("[Copied: {head}…]");
            self.insert_chip(&label, cleaned);
        }

        /// Puts a label in the draft, carrying `original` for the model.
        fn insert_chip(&mut self, label: &str, original: String) {
            self.insert_attachment(label, original, None);
        }

        /// Puts a label in the draft that also asks for a file to be attached.
        fn insert_attachment(
            &mut self,
            label: &str,
            original: String,
            attachment: Option<Attachment>,
        ) {
            self.settle_cursor();
            let start = self.cursor;
            for character in label.chars() {
                self.chars.insert(self.cursor, character);
                self.cursor += 1;
            }
            self.chips.push(Chip {
                range: start..self.cursor,
                original,
                attachment,
            });
        }

        /// The files the draft asks to send beside its text, in order.
        fn attachments(&self) -> Vec<Attachment> {
            self.chips
                .iter()
                .filter_map(|chip| chip.attachment.clone())
                .collect()
        }

        fn insert_text(&mut self, text: &str) {
            for character in clean_paste(text).chars() {
                self.insert(character);
            }
        }

        /// Moves the cursor out of any chip it is inside.
        ///
        /// A chip is not editable: typing in the middle of one would leave the
        /// label and what it stands for disagreeing, which is the state the
        /// whole thing exists to prevent. The cursor lands at its end, so typing
        /// after a chip does what it looks like.
        fn settle_cursor(&mut self) {
            if let Some(chip) = self
                .chips
                .iter()
                .find(|chip| chip.range.start < self.cursor && self.cursor < chip.range.end)
            {
                self.cursor = chip.range.end;
            }
        }

        /// Removes `chars[from..to]`, taking any chip it touches whole.
        fn remove_range(&mut self, from: usize, to: usize) -> bool {
            if from >= to || to > self.chars.len() {
                return false;
            }
            let (mut lo, mut hi) = (from, to);
            let mut kept: Vec<Chip> = Vec::new();
            for chip in std::mem::take(&mut self.chips) {
                if chip.range.start < to && from < chip.range.end {
                    lo = lo.min(chip.range.start);
                    hi = hi.max(chip.range.end);
                } else {
                    kept.push(chip);
                }
            }
            self.chars.drain(lo..hi);
            let removed = hi - lo;
            for chip in &mut kept {
                if chip.range.start >= hi {
                    chip.range.start -= removed;
                    chip.range.end -= removed;
                }
            }
            self.chips = kept;
            self.cursor = if self.cursor > hi {
                self.cursor - removed
            } else {
                lo
            };
            true
        }

        fn backspace(&mut self) -> bool {
            if self.cursor == 0 {
                return false;
            }
            self.remove_range(self.cursor - 1, self.cursor)
        }

        fn delete(&mut self) -> bool {
            if self.cursor >= self.chars.len() {
                return false;
            }
            self.remove_range(self.cursor, self.cursor + 1)
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
            self.remove_range(0, self.cursor)
        }

        fn kill_to_end(&mut self) -> bool {
            if self.cursor >= self.chars.len() {
                return false;
            }
            self.remove_range(self.cursor, self.chars.len())
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
            self.remove_range(start, self.cursor)
        }

        /// The range of the line the cursor is on, and its column in it.
        fn current_line(&self) -> ((usize, usize), usize) {
            let mut start = 0usize;
            for index in 0..self.cursor {
                if self.chars[index] == '\n' {
                    start = index + 1;
                }
            }
            let mut end = self.chars.len();
            for (index, character) in self.chars.iter().enumerate().skip(start) {
                if *character == '\n' {
                    end = index;
                    break;
                }
            }
            ((start, end), self.cursor - start)
        }

        /// Moves the cursor one line up, keeping the column where it can.
        ///
        /// Returns whether it moved: at the first line there is nowhere to go,
        /// which is the caller's signal to walk the history instead.
        fn up(&mut self) -> bool {
            let ((start, _), column) = self.current_line();
            if start == 0 {
                return false;
            }
            // The line above runs from its own newline to the one that ends it.
            let above_end = start - 1;
            let mut above_start = 0usize;
            for index in 0..above_end {
                if self.chars[index] == '\n' {
                    above_start = index + 1;
                }
            }
            let width = above_end - above_start;
            self.cursor = above_start + column.min(width);
            true
        }

        /// Moves the cursor one line down, keeping the column where it can.
        fn down(&mut self) -> bool {
            let ((_, end), column) = self.current_line();
            if end >= self.chars.len() {
                return false;
            }
            let start = end + 1;
            let mut next_end = self.chars.len();
            for (index, character) in self.chars.iter().enumerate().skip(start) {
                if *character == '\n' {
                    next_end = index;
                    break;
                }
            }
            let width = next_end - start;
            self.cursor = start + column.min(width);
            true
        }

        fn clear(&mut self) {
            self.chars.clear();
            self.chips.clear();
            self.cursor = 0;
        }

        fn replace(&mut self, text: &str) {
            self.chars = text.chars().collect();
            self.chips.clear();
            self.cursor = self.chars.len();
        }

        /// The draft as it is drawn: the labels, not what they stand for.
        #[cfg(test)]
        fn text_without_chips(&self) -> String {
            self.chars.iter().collect()
        }

        /// The draft as it is sent: every chip stands for what it carries.
        fn text(&self) -> String {
            let mut out = String::new();
            let mut index = 0usize;
            for chip in &self.chips {
                if chip.range.start > self.chars.len() {
                    continue;
                }
                out.extend(self.chars[index..chip.range.start].iter());
                out.push_str(&chip.original);
                index = chip.range.end.min(self.chars.len());
            }
            out.extend(self.chars[index..].iter());
            out
        }

        fn is_empty(&self) -> bool {
            self.chars.is_empty()
        }
    }

    #[allow(clippy::too_many_lines)]
    fn run_editor(
        style: Style,
        mut cols: u16,
        mut rows: u16,
        pinned: &Pinned,
        sender: &mpsc::UnboundedSender<EditorEvent>,
        stop: &AtomicBool,
    ) {
        let mut buffer = LineBuffer::default();
        let mut history: Vec<String> = Vec::new();
        let mut history_pos: Option<usize> = None;
        let mut draft = String::new();
        // Where the keyboard is. `None` is the input line, which is where a
        // person starts and where every key belongs; `Some(tab)` is the tab row
        // with that tab selected but not yet applied.
        let mut focus: Option<SessionTab> = None;

        let suggestion = command_suggestion(&buffer.text(), buffer.cursor);
        draw_input(
            style,
            cols,
            rows,
            &buffer,
            suggestion.as_ref(),
            &pinned.pending(),
        );
        draw_footer(style, cols, rows, pinned, suggestion.as_ref(), focus);
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
                            // While the focus is on the tab row the key is the
                            // row's, not the draft's. A key the row does not
                            // use drops the focus and falls through, so typing
                            // brings a person back to the input without
                            // swallowing the character that did it.
                            let mut to_input = true;
                            if let Some(selected) = focus {
                                match tab_row_key(key, selected) {
                                    TabRowKey::Select(tab) => {
                                        focus = Some(tab);
                                        redraw = true;
                                        to_input = false;
                                    }
                                    TabRowKey::Apply => {
                                        pinned.set_tab(selected);
                                        focus = None;
                                        redraw = true;
                                        to_input = false;
                                    }
                                    // Up and Escape go back to the input
                                    // without changing anything, which is the
                                    // one thing that makes the row safe to
                                    // walk into by accident.
                                    TabRowKey::Leave => {
                                        focus = None;
                                        redraw = true;
                                        to_input = false;
                                    }
                                    // A key the row does not use belongs to the
                                    // draft: the focus drops and the character
                                    // that did it is still typed, so a person
                                    // who starts typing is back in the input
                                    // without losing the first letter.
                                    TabRowKey::Input => {
                                        focus = None;
                                        redraw = true;
                                    }
                                }
                            }
                            if to_input {
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
                                    // The editor does not act on these: it cannot
                                    // cancel a turn, and it cannot know whether one
                                    // is running. It says what was asked, and the
                                    // reader that has that context does the rest.
                                    KeyOutcome::CancelTurn => {
                                        let _ = sender
                                            .send(EditorEvent::Action(EditorAction::CancelTurn));
                                        redraw = true;
                                    }
                                    KeyOutcome::CancelJobs => {
                                        let _ = sender
                                            .send(EditorEvent::Action(EditorAction::CancelJobs));
                                        redraw = true;
                                    }
                                    KeyOutcome::Detach => {
                                        let _ =
                                            sender.send(EditorEvent::Action(EditorAction::Detach));
                                        redraw = true;
                                    }
                                    // The tab is the editor's to change: it holds
                                    // the shared state and repaints the footer from
                                    // it. Asking the reader would need a round trip
                                    // for something that never leaves this process.
                                    KeyOutcome::NextTab => {
                                        pinned.set_tab(pinned.tab().next());
                                        redraw = true;
                                    }
                                    KeyOutcome::ShowTab(tab) => {
                                        pinned.set_tab(tab);
                                        redraw = true;
                                    }
                                    KeyOutcome::PreviousTab => {
                                        pinned.set_tab(pinned.tab().previous());
                                        redraw = true;
                                    }
                                    // Down, with the draft at its last line and
                                    // the history at its newest entry: the only
                                    // thing under the input is the tab row.
                                    KeyOutcome::FocusTabs => {
                                        focus = Some(pinned.tab());
                                        redraw = true;
                                    }
                                }
                            }
                        }
                        Ok(Event::Paste(text)) => {
                            buffer.insert_paste(&text);
                            redraw = true;
                        }
                        Ok(Event::Resize(width, height)) => {
                            resize.note(width, height, Instant::now());
                        }
                        _ => {}
                    }
                    if redraw {
                        let suggestion = command_suggestion(&buffer.text(), buffer.cursor);
                        draw_input(
                            style,
                            cols,
                            rows,
                            &buffer,
                            suggestion.as_ref(),
                            &pinned.pending(),
                        );
                        draw_footer(style, cols, rows, pinned, suggestion.as_ref(), focus);
                    }
                }
                Ok(false) => {}
                Err(_) => {
                    let _ = sender.send(EditorEvent::End);
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
                draw_footer(style, cols, rows, pinned, suggestion.as_ref(), focus);
                draw_input(
                    style,
                    cols,
                    rows,
                    &buffer,
                    suggestion.as_ref(),
                    &pinned.pending(),
                );
            }
            if pinned.take_dirty() {
                let suggestion = command_suggestion(&buffer.text(), buffer.cursor);
                draw_footer(style, cols, rows, pinned, suggestion.as_ref(), focus);
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum KeyOutcome {
        Continue,
        Exit,
        /// The user asked for the next tab. Which one that is depends on the
        /// current tab, which the editor does not own, so it asks rather than
        /// deciding.
        NextTab,
        /// The user asked for the previous tab.
        PreviousTab,
        /// The user pressed Down with nowhere left to go: the draft has no line
        /// below and the history is already at its newest entry. The focus
        /// moves to the tab row, which is the one thing under the input.
        FocusTabs,
        /// The user asked for one tab by name or number.
        ShowTab(SessionTab),
        /// The user asked to stop the running turn (Escape).
        CancelTurn,
        /// The user asked to stop the running jobs (Ctrl-C).
        CancelJobs,
        /// The user asked to stop waiting for the running call and leave its
        /// process running (Ctrl-G).
        Detach,
    }

    #[allow(clippy::too_many_lines)]
    /// What a key means while the tab row has the focus.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TabRowKey {
        /// Move the selection.
        Select(SessionTab),
        /// Show the selected tab and go back to the input.
        Apply,
        /// Go back to the input without changing the tab.
        Leave,
        /// Not the row's key: drop the focus and let the input handle it.
        Input,
    }

    /// Reads one key while the tab row has the focus.
    ///
    /// Left and right walk the row, Enter applies and leaves, and up goes back
    /// without applying — the opposite of the down that came in. Everything
    /// else is the input's, including Escape and `Ctrl-T`, which keeps the two
    /// ways of changing tabs from fighting over the same key.
    ///
    /// Only the arrows and Tab walk the row: `h` and `l` were here at first,
    /// by vim habit, and they ate the first letter of anyone who started typing
    /// from the row — the row is entered by one keypress too many, so the
    /// letter that follows it is far more likely than an unlabelled j/k.
    fn tab_row_key(key: KeyEvent, selected: SessionTab) -> TabRowKey {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left | KeyCode::BackTab => TabRowKey::Select(selected.previous()),
            KeyCode::Right => TabRowKey::Select(selected.next()),
            KeyCode::Enter => TabRowKey::Apply,
            KeyCode::Up if !control => TabRowKey::Leave,
            KeyCode::Esc => TabRowKey::Leave,
            // Tab cycles the selection like right, so the row is walkable
            // without the arrow keys on a keyboard that has none handy.
            KeyCode::Tab if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                TabRowKey::Select(selected.next())
            }
            _ => TabRowKey::Input,
        }
    }

    #[allow(clippy::too_many_lines)] // one arm per key, which is the keymap.
    fn handle_key(
        key: KeyEvent,
        buffer: &mut LineBuffer,
        history: &mut Vec<String>,
        history_pos: &mut Option<usize>,
        draft: &mut String,
        sender: &mpsc::UnboundedSender<EditorEvent>,
    ) -> KeyOutcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        // The tabs: `Ctrl-T` cycles and `Ctrl-1..3` go straight to one. They are
        // read before the editing keys so a digit never lands in the draft while
        // a modifier is held.
        if control && !alt {
            match key.code {
                // `Ctrl-Shift-T` walks back the way `Ctrl-T` walks forward.
                KeyCode::Char('T') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    return KeyOutcome::PreviousTab;
                }
                KeyCode::Char('t' | 'T') => return KeyOutcome::NextTab,
                KeyCode::Char(digit @ '1'..='3') => {
                    if let Some(tab) = digit
                        .to_digit(10)
                        .and_then(|n| SessionTab::numbered(n as usize))
                    {
                        return KeyOutcome::ShowTab(tab);
                    }
                }
                _ => {}
            }
        }
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
                let attachments = buffer.attachments();
                if sender.send(EditorEvent::Line(line, attachments)).is_err() {
                    return KeyOutcome::Exit;
                }
                buffer.clear();
                KeyOutcome::Continue
            }
            KeyCode::Char('d') if control => {
                if buffer.is_empty() {
                    let _ = sender.send(EditorEvent::End);
                    return KeyOutcome::Exit;
                }
                KeyOutcome::Continue
            }
            // Ctrl-C ends the jobs the session started rather than the turn.
            //
            // It always reports the key: the editor cannot know whether a job
            // is running, and the reader that knows turns a jobless Ctrl-C into
            // nothing at all. Deciding here would need the editor to reach the
            // service, which it cannot.
            KeyCode::Char('c') if control => KeyOutcome::CancelJobs,
            KeyCode::Char('l') if control => KeyOutcome::Continue,
            // Ctrl-G stops waiting for the running call and leaves the process
            // running. Ctrl-Shift-B is what it used to be, and stays an alias.
            //
            // Ctrl-Shift-B did nothing here. It arrived as `Char('b')` carrying
            // SHIFT and CONTROL — the keyboard protocol sends the *unshifted*
            // codepoint — while the branch looked for `Char('B')`, so the key fell
            // through to plain Ctrl-B and moved the cursor while the call kept
            // running. Reading crossterm's parser did not settle whether the
            // terminal or crossterm dropped the shift, and a key that does nothing
            // is not something a person can diagnose from the outside. Ctrl-G is
            // asked for as an ordinary control key, and crossterm reaches
            // `Char('g')` with CONTROL on both of its paths: the legacy byte
            // (`\x07`, matched by `b'\x01'..=b'\x1A'` in
            // `event/sys/unix/parse.rs`) and the keyboard protocol's `CSI 103;5u`.
            // Neither depends on the shift being reported, and neither does the
            // pattern below, which is why `'G'` is in it: a terminal that encodes
            // a modified key with the shifted codepoint reaches the same branch.
            KeyCode::Char('g' | 'G') if control => KeyOutcome::Detach,
            // The original binding, kept for a terminal that reports the shift.
            KeyCode::Char('b' | 'B') if control && key.modifiers.contains(KeyModifiers::SHIFT) => {
                KeyOutcome::Detach
            }
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
                // Inside a multi-line draft the arrows move the cursor; only
                // from the first line do they reach the history.
                if !buffer.up() {
                    history_previous(buffer, history, history_pos, draft);
                }
                KeyOutcome::Continue
            }
            KeyCode::Down => {
                if buffer.down() {
                    return KeyOutcome::Continue;
                }
                // The draft has no line below. Walking down the history comes
                // first, because that is what the key has always done; the tab
                // row is what is left once the history is at its newest entry
                // and there is nowhere further down to go.
                if history_pos.is_some() {
                    history_next(buffer, history, history_pos, draft);
                    KeyOutcome::Continue
                } else {
                    KeyOutcome::FocusTabs
                }
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
            // Escape asks for the running turn to stop, and does nothing
            // otherwise; the reader decides which. It used to clear the draft,
            // which is gone: a key that sometimes cancels and sometimes erases
            // is one a user cannot press with confidence.
            KeyCode::Esc => KeyOutcome::CancelTurn,
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

    /// The last row the input line may occupy.
    ///
    /// The three rows under it hold the pinned widgets, which are laid out from
    /// the bottom, so the input grows upward from here.
    fn input_bottom(rows: u16) -> u16 {
        todo_row(rows).saturating_sub(1).max(1)
    }

    fn status_row(rows: u16) -> u16 {
        rows.saturating_sub(2).max(1)
    }

    /// An empty line between the input block and the widgets, so the draft does
    /// not touch the status line.
    fn input_gap(rows: u16) -> u16 {
        rows.saturating_sub(3).max(1)
    }

    fn suggestion_row(rows: u16) -> u16 {
        rows.saturating_sub(1).max(1)
    }

    fn todo_row(rows: u16) -> u16 {
        input_gap(rows)
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
        // Set the region first, so the clears below cannot scroll and the region
        // is never absent: releasing it with `\x1b[r` left the renderer free to
        // write over the footer until the new one was set.
        let mut frame = format!("\x1b[1;{bottom}r");

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

        // Park the cursor on the last row of the region. No newline here: on the
        // bottom row one scrolls the whole region, and every scroll leaves a
        // blank line above the footer, so a drag walked the footer down the
        // screen one row per resize event. The footer rows are repainted by the
        // caller right after this frame.
        let _ = write!(frame, "\x1b[{bottom};1H");
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
        focus: Option<SessionTab>,
    ) {
        let (session, todos, tab) = pinned.snapshot();
        let width = usize::from(cols).saturating_sub(2);
        let suggestion_text = suggestion.map_or_else(String::new, |suggestion| {
            style.faint(suggestion.hint.clone())
        });
        // What the active tab shows. `chat` has no widget of its own: the
        // conversation is the scroll region above, so that row keeps carrying
        // the session status it always did. The other two replace it with their
        // own content, which is what makes them views rather than extras.
        let body = match tab {
            SessionTab::Chat => session.clone(),
            SessionTab::Todo => todo_line(style, &todos, width),
            SessionTab::Debug => debug_line(style, &pinned.debug()),
        };
        let tabs_text = tabs_line(style, focus.unwrap_or(tab), focus.is_some(), width);
        let frame = format!(
            "\x1b7\x1b[{};1H\x1b[2K{}\x1b[{};1H\x1b[2K{}\x1b[{};1H\x1b[2K{}\x1b8",
            todo_row(rows),
            clip_visible(&tabs_text, width),
            status_row(rows),
            clip_visible(&body, width),
            suggestion_row(rows),
            clip_visible(&suggestion_text, width)
        );
        write_frame(&frame);
    }

    /// The row of tabs, with the active one in brackets.
    ///
    /// One dim line, because that is all the footer has: the active tab is the
    /// only name in the foreground and bracketed, so where the person is reads
    /// at a glance and the rest say what is a keystroke away. The key that
    /// cycles is named once, at the end, only when it fits.
    fn tabs_line(style: Style, selected: SessionTab, focused: bool, max: usize) -> String {
        let mut rendered = String::new();
        for (index, tab) in SessionTab::ALL.iter().enumerate() {
            if index > 0 {
                rendered.push_str(&style.faint(" · "));
            }
            let name = tab.name();
            rendered.push_str(&if *tab == selected && focused {
                // The focus is on the row: the selection is what Enter would
                // apply, so it is drawn as the one under the cursor.
                style.bold(format!("›{name}‹"))
            } else if *tab == selected {
                style.bold(format!("[{name}]"))
            } else {
                style.faint(name.to_owned())
            });
        }
        // What the keys do depends on where the keyboard is, so the hint says
        // which row is listening rather than naming a key that would not work.
        let hint = if focused { "  ←→ enter" } else { "  ^T" };
        // What is shown, not what is written: the row carries `style` codes,
        // and counting those made every hint too wide to fit.
        if visible_width(&rendered) + hint.chars().count() <= max {
            rendered.push_str(&style.faint(hint));
        }
        rendered
    }

    /// The debug tab's summary, as the one row the footer has room for.
    ///
    /// The count and nothing else: the panel is a box several rows tall with
    /// its escapes already applied, and `style` escapes what it is given, so a
    /// row of it here would print the escape instead of using it. The panel
    /// itself is printed above, where there is room for it.
    fn debug_line(style: Style, debug: &[String]) -> String {
        if debug.is_empty() {
            return style.faint("no startup information");
        }
        style.dim(format!(
            "{} line(s) of startup information · printed above",
            debug.len()
        ))
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
    /// How many columns a string with style codes occupies.
    fn visible_width(text: &str) -> usize {
        let mut width = 0usize;
        let mut chars = text.chars();
        while let Some(character) = chars.next() {
            if character == '\x1b' {
                for next in chars.by_ref() {
                    if next == 'm' {
                        break;
                    }
                }
                continue;
            }
            width += 1;
        }
        width
    }

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

    /// The buffer as the rows the input line draws.
    ///
    /// A logical line (one separated by `\n`) becomes as many rows as it needs
    /// at `width`, and the window keeps the row the cursor is on in view: a
    /// draft longer than the terminal wraps, it does not lose its tail.
    /// A pasted path that names an image, as the attachment to carry.
    ///
    /// Only a path that exists and whose extension is one a model can look at makes
    /// an attachment: anything else is text, and pasting a path to a script is still
    /// pasting text.
    fn image_attachment(text: &str) -> Option<Attachment> {
        let path = text.trim();
        // A terminal may wrap a path in quotes, and a path with a newline in it is
        // not one a paste can carry.
        let path = path
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .or_else(|| {
                path.strip_prefix('\'')
                    .and_then(|rest| rest.strip_suffix('\''))
            })
            .unwrap_or(path);
        if path.is_empty() || path.contains('\n') {
            return None;
        }
        let mime = image_mime(path)?;
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let name = path.rsplit('/').next().unwrap_or(path).to_owned();
        Some(Attachment {
            path: path.to_owned(),
            mime: mime.to_owned(),
            name,
        })
    }

    /// The file name of a pasted path that exists, when it is long enough to hide.
    ///
    /// A path to a file the model cannot look at is still a path: showing it whole
    /// puts a directory listing in the middle of a sentence, and calling it
    /// `[Copied: …]` says it is text when it is a reference.
    fn existing_path_name(text: &str) -> Option<String> {
        let path = text.trim();
        if path.chars().count() < CHIP_MIN_CHARS || path.contains('\n') || path.contains(' ') {
            return None;
        }
        if !std::fs::metadata(path).is_ok_and(|meta| meta.is_file()) {
            return None;
        }
        Some(path.rsplit('/').next().unwrap_or(path).to_owned())
    }

    /// The media type of an image by its extension, or `None` when it is not one.
    fn image_mime(path: &str) -> Option<&'static str> {
        let extension = path.rsplit('.').next()?.to_ascii_lowercase();
        match extension.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            "bmp" => Some("image/bmp"),
            _ => None,
        }
    }

    /// A pasted text as it enters the draft: line endings to `\n`, a tab to
    /// spaces, and control characters dropped.
    fn clean_paste(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(character) = chars.next() {
            match character {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push('\n');
                }
                '\n' => out.push('\n'),
                '\t' => out.push_str("    "),
                control if control.is_control() => {}
                other => out.push(other),
            }
        }
        out
    }

    fn input_lines(buffer: &LineBuffer, window: usize, width: usize) -> Vec<String> {
        let width = width.max(1);
        // Where each logical line starts and ends in the buffer.
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut start = 0usize;
        for (index, character) in buffer.chars.iter().enumerate() {
            if *character == '\n' {
                spans.push((start, index));
                start = index + 1;
            }
        }
        spans.push((start, buffer.chars.len()));

        // Every visual row, as the buffer range it shows, and which one holds
        // the cursor.
        let mut rows: Vec<(usize, usize)> = Vec::new();
        let mut cursor_row = 0usize;
        for (from, to) in spans {
            let mut row = from;
            loop {
                let end = (row + width).min(to);
                if buffer.cursor >= row && buffer.cursor <= end {
                    cursor_row = rows.len();
                }
                rows.push((row, end));
                if end == to {
                    break;
                }
                row = end;
            }
        }

        let first = cursor_row.saturating_add(1).saturating_sub(window.max(1));
        let mut rendered = Vec::new();
        for (from, to) in rows.iter().skip(first).take(window.max(1)).copied() {
            let mut line = String::new();
            for index in from..to {
                let character = buffer.chars[index];
                if index == buffer.cursor {
                    line.push_str("\x1b[7m");
                    line.push(character);
                    line.push_str("\x1b[27m");
                } else {
                    line.push(character);
                }
            }
            // A cursor at the very end of a row still needs a cell, and an empty
            // logical line still needs one row to itself.
            if buffer.cursor == to {
                line.push_str("\x1b[7m \x1b[27m");
            }
            rendered.push(line);
        }
        rendered
    }

    fn draw_input(
        style: Style,
        cols: u16,
        rows: u16,
        buffer: &LineBuffer,
        suggestion: Option<&CommandSuggestion>,
        pending: &[String],
    ) {
        let prompt = format!("{} ", style.cyan("›"));
        let prompt_width = 2usize;
        let available = usize::from(cols).saturating_sub(prompt_width + 1).max(1);
        let last_row = input_bottom(rows);
        let block_top = last_row.saturating_sub(INPUT_ROWS).saturating_add(1).max(1);

        // A message waiting to reach the running request is shown above the
        // draft, on rows the input reserves and rarely fills. The draft's window
        // shrinks by as many rows as they take, so the two never overlap.
        let shown = pending.len().min(usize::from(MAX_STEER_ROWS));
        let window = usize::from(INPUT_ROWS).saturating_sub(shown).max(1);
        let lines = input_lines(buffer, window, available);
        let first_row = last_row
            .saturating_sub(u16::try_from(lines.len().saturating_sub(1)).unwrap_or(u16::MAX));

        let mut frame = String::from("\x1b7");
        // Every row the input may occupy is cleared first, then the rows it
        // needs are written. Clearing only the rows being drawn left the ones a
        // taller draft had used on screen, so deleting a line left copies of it
        // behind; and the first row moves down as the draft shrinks, so the row
        // it used to start on would not even be addressed.
        for row in block_top..=last_row {
            let _ = write!(frame, "\x1b[{row};1H\x1b[2K");
        }
        // The waiting messages lead, because they are the oldest thing on screen
        // and the one that will move into the transcript first. The last shown
        // carries a count of the ones that did not fit, so a hidden message is
        // never silent.
        for (index, text) in pending.iter().take(shown).enumerate() {
            let row = block_top.saturating_add(u16::try_from(index).unwrap_or(0));
            let tail = if index + 1 == shown && pending.len() > shown {
                format!("  +{}", pending.len() - shown)
            } else {
                String::new()
            };
            let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let label = style.faint(format!("⋯ {one}{tail}"));
            let _ = write!(frame, "\x1b[{row};1H");
            frame.push_str(&clip_visible(
                &label,
                usize::from(cols).saturating_sub(1).max(1),
            ));
        }
        for (offset, line) in lines.iter().enumerate() {
            let row = first_row.saturating_add(u16::try_from(offset).unwrap_or(0));
            let _ = write!(frame, "\x1b[{row};1H");
            // Continuation rows keep the prompt column blank so the text stays
            // aligned under the first line.
            if offset == 0 {
                frame.push_str(&prompt);
            } else {
                frame.push_str(&" ".repeat(prompt_width));
            }
            // Already wrapped to the width; clipping here would cut the rows
            // the wrap just made.
            frame.push_str(line);
        }
        // The completion hint belongs on the cursor row and only makes sense
        // while the draft is a single line.
        if lines.len() == 1 {
            if let Some(suggestion) = suggestion {
                if !suggestion.completion.is_empty() {
                    let typed = buffer.chars.len().min(available);
                    let room = available.saturating_sub(typed + 1);
                    if room > 0 {
                        frame.push_str(&clip_visible(
                            &style.faint(suggestion.completion.clone()),
                            room,
                        ));
                    }
                }
            }
        }
        frame.push_str("\x1b8");
        write_frame(&frame);
    }

    #[cfg(test)]
    mod tests {
        use std::time::{Duration, Instant};

        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::tabs::SessionTab;

        use super::{
            EditorEvent, FOOTER_ROWS, INPUT_ROWS, KeyOutcome, LineBuffer, ResizeDebounce,
            TabRowKey, TodoItem, TodoStatus, clip_visible, handle_key, input_bottom, input_lines,
            resize_frame, scroll_bottom, status_row, suggestion_row, tab_row_key, todo_line,
            todo_row, visible_width,
        };

        #[test]
        fn the_hint_is_measured_without_its_colour_codes() {
            // The width of styled text is what is seen, not what is written:
            // counting the escapes made every hint look too wide to fit, which
            // is why the row always showed the shorter one.
            assert_eq!(visible_width("plain"), 5);
            assert_eq!(
                visible_width("\x1b[38;5;244mtodo\x1b[0m"),
                4,
                "the codes are not columns"
            );
            assert_eq!(visible_width(""), 0);
        }

        #[test]
        fn the_tab_row_reads_its_own_keys() {
            // Left and right walk, Enter applies, up leaves. The row is what a
            // person reaches by pressing down one time too many, so it has to
            // be walkable and it has to be escapable.
            let key = |code, modifiers| KeyEvent::new(code, modifiers);
            assert_eq!(
                tab_row_key(key(KeyCode::Right, KeyModifiers::NONE), SessionTab::Chat),
                TabRowKey::Select(SessionTab::Todo)
            );
            assert_eq!(
                tab_row_key(key(KeyCode::Left, KeyModifiers::NONE), SessionTab::Chat),
                TabRowKey::Select(SessionTab::Debug),
                "left wraps"
            );
            assert_eq!(
                tab_row_key(key(KeyCode::Enter, KeyModifiers::NONE), SessionTab::Todo),
                TabRowKey::Apply
            );
            assert_eq!(
                tab_row_key(key(KeyCode::Up, KeyModifiers::NONE), SessionTab::Todo),
                TabRowKey::Leave
            );
            assert_eq!(
                tab_row_key(key(KeyCode::Esc, KeyModifiers::NONE), SessionTab::Todo),
                TabRowKey::Leave
            );
            // No letter navigates: every one of them drops the focus and is
            // typed, so starting to write from the row does not swallow the
            // first character. `h` and `l` were the aliases that did eat it.
            for character in ['h', 'l', 'j', 'k', 'x', 'q'] {
                assert_eq!(
                    tab_row_key(
                        key(KeyCode::Char(character), KeyModifiers::NONE),
                        SessionTab::Todo
                    ),
                    TabRowKey::Input,
                    "`{character}` belongs to the draft"
                );
            }
            // Tab walks the row, because a keyboard without arrows still has
            // to be able to move; backwards is `Shift-Tab`.
            assert_eq!(
                tab_row_key(key(KeyCode::Tab, KeyModifiers::NONE), SessionTab::Chat),
                TabRowKey::Select(SessionTab::Todo)
            );
            assert_eq!(
                tab_row_key(key(KeyCode::BackTab, KeyModifiers::NONE), SessionTab::Chat),
                TabRowKey::Select(SessionTab::Debug)
            );
        }

        #[test]
        fn down_reaches_the_tab_row_only_with_nowhere_else_to_go() {
            // Down is the history key first and the tab row second: a draft
            // with a line below moves the cursor, a history still walking keeps
            // walking, and only the end of both reaches the row.
            let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut history = vec!["one".to_owned(), "two".to_owned()];
            let mut draft = String::new();

            // Walking back through the history: down is still the history's.
            let mut buffer = LineBuffer::default();
            let mut position = None;
            let _ = handle_key(
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                &mut buffer,
                &mut history,
                &mut position,
                &mut draft,
                &sender,
            );
            assert!(position.is_some(), "up entered the history");
            assert_eq!(
                handle_key(
                    KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                    &mut buffer,
                    &mut history,
                    &mut position,
                    &mut draft,
                    &sender,
                ),
                KeyOutcome::Continue,
                "the history is still walking"
            );
            assert!(position.is_none(), "and it reached the newest entry");

            // Now there is nowhere down: the row is what is left.
            assert_eq!(
                handle_key(
                    KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                    &mut buffer,
                    &mut history,
                    &mut position,
                    &mut draft,
                    &sender,
                ),
                KeyOutcome::FocusTabs
            );
        }

        #[test]
        fn the_tab_keys_ask_for_a_tab_without_touching_the_draft() {
            // A modified digit must not land in the draft: the branch is read
            // before the editing keys for exactly that reason.
            let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut history = Vec::new();
            let mut history_pos = None;
            let mut draft = String::new();

            let mut buffer = LineBuffer::default();
            buffer.insert_text("hola");
            let outcome = handle_key(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
                &mut buffer,
                &mut history,
                &mut history_pos,
                &mut draft,
                &sender,
            );
            assert_eq!(outcome, KeyOutcome::NextTab);
            assert_eq!(buffer.text(), "hola", "the draft keeps its text");

            let mut buffer = LineBuffer::default();
            let outcome = handle_key(
                KeyEvent::new(KeyCode::Char('2'), KeyModifiers::CONTROL),
                &mut buffer,
                &mut history,
                &mut history_pos,
                &mut draft,
                &sender,
            );
            assert_eq!(outcome, KeyOutcome::ShowTab(SessionTab::Todo));
            assert_eq!(buffer.text(), "", "the digit never reaches the draft");

            // Without the modifier a digit is just a digit.
            let mut buffer = LineBuffer::default();
            let _ = handle_key(
                KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
                &mut buffer,
                &mut history,
                &mut history_pos,
                &mut draft,
                &sender,
            );
            assert_eq!(buffer.text(), "2");
        }

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
            let bottom = scroll_bottom(24);
            // The region is set, never released: `\x1b[r` left the renderer free
            // to write over the footer until the new one was set.
            assert!(!frame.contains("\x1b[r"), "{frame:?}");
            assert!(frame.starts_with(&format!("\x1b[1;{bottom}r")), "{frame:?}");
            for row in bottom + 1..=24 {
                assert_eq!(
                    frame.matches(&format!("\x1b[{row};1H\x1b[2K")).count(),
                    1,
                    "row {row} should be cleared once"
                );
            }
            assert!(frame.ends_with(&format!("\x1b[{bottom};1H")), "{frame:?}");
        }

        #[test]
        fn resize_frame_never_scrolls() {
            // A newline on the bottom row of the region scrolls it, and each
            // scroll leaves a blank line above the footer. A drag fires the
            // resize many times, so the footer walked down the screen.
            for (rows, old_rows) in [(24, 24), (30, 24), (24, 30), (5, 5)] {
                let frame = resize_frame(rows, old_rows);
                assert!(
                    !frame.contains('\n'),
                    "the frame for {rows}/{old_rows} must not scroll: {frame:?}"
                );
                assert!(!frame.contains('\r'), "{frame:?}");
            }
        }

        #[test]
        fn resize_frame_clears_old_and_new_footer_rows() {
            let grown = resize_frame(30, 24);
            let old_bottom = scroll_bottom(24);
            let new_bottom = scroll_bottom(30);
            let mut cleared: Vec<u16> = (old_bottom + 1..=24).collect();
            cleared.extend(new_bottom + 1..=30);
            for row in cleared {
                assert_eq!(
                    grown.matches(&format!("\x1b[{row};1H\x1b[2K")).count(),
                    1,
                    "row {row} should be cleared once"
                );
            }
            // Every row that is neither an old footer row nor a new one is the
            // scroll area and is left alone.
            for row in 1..=30u16 {
                let old_footer = row > old_bottom && row <= 24;
                let new_footer = row > new_bottom;
                if !old_footer && !new_footer {
                    assert!(
                        !grown.contains(&format!("\x1b[{row};1H\x1b[2K")),
                        "scroll row {row} must be preserved"
                    );
                }
            }
            assert!(
                grown.ends_with(&format!("\x1b[{new_bottom};1H")),
                "{grown:?}"
            );

            let shrunk = resize_frame(24, 30);
            let shrunk_bottom = scroll_bottom(24);
            for row in shrunk_bottom + 1..=24 {
                assert_eq!(
                    shrunk.matches(&format!("\x1b[{row};1H\x1b[2K")).count(),
                    1,
                    "row {row} should be cleared once"
                );
            }
            for row in 25..=30 {
                assert!(
                    !shrunk.contains(&format!("\x1b[{row};1H\x1b[2K")),
                    "rows beyond the new height must not be addressed"
                );
            }
            assert!(
                shrunk.ends_with(&format!("\x1b[{shrunk_bottom};1H")),
                "{shrunk:?}"
            );
        }

        #[test]
        fn footer_rows_stay_below_the_scroll_region() {
            // The widget rows sit at the bottom, the todo row and its gap
            // above them, and the input's own rows above those. The scroll
            // region ends below all of it.
            assert_eq!(suggestion_row(24), 23);
            assert_eq!(status_row(24), 22);
            assert_eq!(todo_row(24), 21);
            assert_eq!(input_bottom(24), 20);
            assert_eq!(scroll_bottom(24), 14);
            // The first row the input clears is the top of its own block, and it
            // must be *below* the region: the region is where the transcript is
            // written, so a repaint that reached into it erased streamed output
            // on every keypress.
            for rows in 11..=60u16 {
                let block_top = input_bottom(rows)
                    .saturating_sub(INPUT_ROWS)
                    .saturating_add(1);
                assert!(
                    scroll_bottom(rows) < block_top,
                    "rows {rows}: the region ends at {} but the input clears from {block_top}",
                    scroll_bottom(rows)
                );
                assert!(input_bottom(rows) < todo_row(rows), "rows {rows}");
                assert!(todo_row(rows) < status_row(rows), "rows {rows}");
                assert!(status_row(rows) < suggestion_row(rows), "rows {rows}");
            }
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
        fn ctrl_g_detaches_and_the_keys_around_it_do_not() {
            // Detaching sits on a plain Ctrl-G. Ctrl-Shift-B is what it was, and
            // did nothing: it arrived as `Char('b')` with SHIFT and CONTROL — the
            // keyboard protocol sends the *unshifted* codepoint — while the branch
            // looked for `Char('B')`, so it fell through to plain Ctrl-B and moved
            // the cursor while the call kept running.
            let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut buffer = LineBuffer::default();
            buffer.insert_text("abc");
            let mut history = Vec::new();
            let mut history_pos = None;
            let mut draft = String::new();

            // Every one of these is a key a terminal can send for Ctrl-G, and
            // all of them mean the same thing here. The shift is not part of the
            // request: the legacy byte `\x07` cannot carry one.
            let detaching = [
                (KeyCode::Char('g'), KeyModifiers::CONTROL),
                (
                    KeyCode::Char('g'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                ),
                (
                    KeyCode::Char('G'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                ),
            ];
            for (code, modifiers) in detaching {
                assert_eq!(
                    handle_key(
                        KeyEvent::new(code, modifiers),
                        &mut buffer,
                        &mut history,
                        &mut history_pos,
                        &mut draft,
                        &sender,
                    ),
                    KeyOutcome::Detach,
                    "{code:?} with {modifiers:?} asks to detach"
                );
            }
            // Nothing was typed and the cursor did not move: that is what the
            // keys below are for, and a detach that also edited the line would
            // lose the draft the human was in the middle of.
            assert_eq!(buffer.text(), "abc", "detaching types no g");
            assert_eq!(buffer.cursor, 3, "detaching is not a cursor move");

            // The alias still detaches, and still does not move the cursor.
            assert_eq!(
                handle_key(
                    KeyEvent::new(
                        KeyCode::Char('b'),
                        KeyModifiers::CONTROL | KeyModifiers::SHIFT
                    ),
                    &mut buffer,
                    &mut history,
                    &mut history_pos,
                    &mut draft,
                    &sender,
                ),
                KeyOutcome::Detach,
                "ctrl-shift-b stays an alias"
            );
            assert_eq!(buffer.cursor, 3, "detaching is not a cursor move");

            // The keys the rebinding must not have swallowed.
            assert_eq!(
                handle_key(
                    KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
                    &mut buffer,
                    &mut history,
                    &mut history_pos,
                    &mut draft,
                    &sender,
                ),
                KeyOutcome::Continue,
                "plain ctrl-b stays where readline put it"
            );
            assert_eq!(buffer.cursor, 2, "and it moves the cursor back");

            assert_eq!(
                handle_key(
                    KeyEvent::new(KeyCode::Char('g'), KeyModifiers::empty()),
                    &mut buffer,
                    &mut history,
                    &mut history_pos,
                    &mut draft,
                    &sender,
                ),
                KeyOutcome::Continue,
                "plain g is still a letter"
            );
            assert_eq!(buffer.text(), "abgc", "and it is typed at the cursor");
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
        fn a_pasted_path_to_an_image_becomes_an_attachment() {
            // A terminal pastes a file's *path* — kitty does, and so does a
            // drag-and-drop — so this is the only way an image can arrive. What
            // the draft shows is a label; what the message carries is the file.
            let dir = std::env::temp_dir().join(format!("norte-paste-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            let photo = dir.join("holiday.png");
            std::fs::write(&photo, b"\x89PNG\r\n\x1a\n").expect("png");

            let mut buffer = LineBuffer::default();
            buffer.insert_paste(&photo.display().to_string());

            let shown = buffer.text_without_chips();
            assert_eq!(
                shown, "[Image: holiday.png]",
                "the draft shows a label, not a path"
            );
            let attachments = buffer.attachments();
            assert_eq!(attachments.len(), 1, "and it asks for the file");
            assert_eq!(attachments[0].mime, "image/png");
            assert_eq!(attachments[0].path, photo.display().to_string());
            assert_eq!(
                buffer.text(),
                photo.display().to_string(),
                "the path is what the model is given"
            );

            // One keystroke takes the label and the attachment together.
            assert!(buffer.backspace());
            assert_eq!(buffer.text(), "");
            assert!(
                buffer.attachments().is_empty(),
                "and the attachment goes with it"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_path_that_is_not_an_image_is_a_path_and_not_an_attachment() {
            // A file the model cannot look at is still a file, and a long path
            // is shown as one: `[Path: build.sh]`, not eighty characters of
            // directory and not `[Copied: …]`, which would call a reference
            // text. Nothing is attached — the path is what the model gets, and
            // it can open it itself with `read`.
            let dir = std::env::temp_dir().join(format!("norte-paste-txt-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            let script = dir.join("build.sh");
            std::fs::write(&script, b"echo hi\n").expect("script");

            let mut buffer = LineBuffer::default();
            buffer.insert_paste(&script.display().to_string());
            assert_eq!(
                buffer.text_without_chips(),
                "[Path: build.sh]",
                "a long path is shown by its name"
            );
            assert_eq!(
                buffer.text(),
                script.display().to_string(),
                "and the path is what the draft sends"
            );
            assert!(buffer.attachments().is_empty(), "nothing is attached");

            // A missing file is text: a path that is not there may arrive later,
            // and the draft must not claim a file it cannot see.
            let mut buffer = LineBuffer::default();
            buffer.insert_paste("/tmp/not-there-at-all.png");
            assert!(
                buffer.attachments().is_empty(),
                "a missing file is not an attachment"
            );
            assert_eq!(buffer.text_without_chips(), "/tmp/not-there-at-all.png");

            // A short path is short enough to show whole.
            let mut buffer = LineBuffer::default();
            buffer.insert_paste("/tmp/a.sh");
            assert_eq!(buffer.text_without_chips(), "/tmp/a.sh");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_long_paste_becomes_a_label_and_sends_what_it_hid() {
            // What the human sees is a label; what the model gets is the text.
            // That is the whole point: a paragraph pasted into the draft buries
            // the sentence being written, and the model must not receive the
            // label in its place.
            let mut buffer = LineBuffer::default();
            let pasted = "Lorem ipsum dolor sit amet, consectetur adipiscing elit sed do";
            buffer.insert_paste(pasted);
            let shown = buffer.text_without_chips();
            assert!(
                shown.starts_with("[Copied: "),
                "the draft shows a label: {shown:?}"
            );
            assert!(shown.ends_with(']'), "and the label closes: {shown:?}");
            assert!(
                !shown.contains("consectetur"),
                "the pasted text is not in the draft: {shown:?}"
            );
            assert_eq!(buffer.text(), pasted, "but it is what the draft sends");
        }

        #[test]
        fn a_short_paste_is_its_own_label() {
            // A chip that saves nothing costs a keystroke to remove, and
            // `[Copied: hi]` is longer than `hi`.
            let mut buffer = LineBuffer::default();
            buffer.insert_paste("hi");
            assert_eq!(buffer.text(), "hi");
            assert_eq!(buffer.text_without_chips(), "hi");
        }

        #[test]
        fn a_keystroke_on_a_label_takes_the_whole_paste() {
            // `]` removes the chip, and with it everything the chip stands for.
            // Half a paragraph is not a smaller paragraph.
            let mut buffer = LineBuffer::default();
            let pasted = "Lorem ipsum dolor sit amet, consectetur adipiscing elit sed do";
            buffer.insert_paste(pasted);
            let label_chars = buffer.text_without_chips().chars().count();
            assert_eq!(buffer.cursor, label_chars, "the cursor is after the label");

            assert!(buffer.backspace(), "a backspace removed something");
            assert_eq!(buffer.text(), "", "the whole paste went with the label");
            assert_eq!(buffer.text_without_chips(), "");
        }

        #[test]
        fn deleting_a_label_from_the_left_takes_it_whole() {
            let mut buffer = LineBuffer::default();
            let pasted = "Lorem ipsum dolor sit amet, consectetur adipiscing elit sed do";
            buffer.insert_paste(pasted);
            assert!(buffer.home());
            assert!(buffer.delete(), "the delete removed something");
            assert_eq!(buffer.text(), "", "the whole paste went with the label");
        }

        #[test]
        fn killing_the_line_takes_the_chips_whole() {
            // Ctrl-U and Ctrl-K are how a draft is cleared, and a chip in the
            // way must not survive them half-formed: what is removed is either
            // the chip or nothing, never a piece of the label with its text
            // still attached.
            let pasted = "Lorem ipsum dolor sit amet, consectetur adipiscing elit sed do";

            // Ctrl-K from the top: the chip and the tail go together.
            let mut buffer = LineBuffer::default();
            buffer.insert_paste(pasted);
            buffer.insert_text("tail");
            assert!(buffer.home());
            assert!(buffer.kill_to_end());
            assert_eq!(buffer.text(), "", "the chip went whole with the rest");
            assert_eq!(buffer.text_without_chips(), "");

            // Ctrl-U with the cursor after the chip: the same.
            let mut buffer = LineBuffer::default();
            buffer.insert_paste(pasted);
            buffer.insert_text("tail");
            assert!(buffer.kill_to_start());
            assert_eq!(buffer.text(), "", "the chip and the tail both went");
            assert_eq!(buffer.text_without_chips(), "");
        }

        #[test]
        fn typing_inside_a_label_lands_after_it() {
            // A chip is not editable: a character inserted in the middle would
            // leave the label and what it stands for disagreeing.
            let mut buffer = LineBuffer::default();
            let pasted = "Lorem ipsum dolor sit amet, consectetur adipiscing elit sed do";
            buffer.insert_paste(pasted);
            assert!(buffer.home());
            buffer.right();
            buffer.insert('X');
            let shown = buffer.text_without_chips();
            assert!(
                !shown.contains("[X"),
                "nothing lands inside the label: {shown:?}"
            );
            assert!(
                shown.starts_with("[Copied: ") && shown.ends_with("]X"),
                "it lands after it: {shown:?}"
            );
            assert_eq!(buffer.text(), format!("{pasted}X"));
        }

        #[test]
        fn two_pastes_are_two_chips() {
            let mut buffer = LineBuffer::default();
            let first = "Lorem ipsum dolor sit amet, consectetur adipiscing elit sed do";
            let second = "Sed ut perspiciatis unde omnis iste natus error sit voluptatem";
            buffer.insert_paste(first);
            buffer.insert_text(" and ");
            buffer.insert_paste(second);
            assert_eq!(
                buffer.text(),
                format!("{first} and {second}"),
                "both pastes reach the model, in order"
            );
            assert_eq!(
                buffer.text_without_chips().matches("[Copied: ").count(),
                2,
                "and the draft shows two labels: {:?}",
                buffer.text_without_chips()
            );
        }

        #[test]
        fn removing_one_chip_leaves_the_other_where_it_was() {
            let mut buffer = LineBuffer::default();
            let first = "Lorem ipsum dolor sit amet, consectetur adipiscing elit sed do";
            let second = "Sed ut perspiciatis unde omnis iste natus error sit voluptatem";
            buffer.insert_paste(first);
            buffer.insert_text(" and ");
            buffer.insert_paste(second);
            // The cursor is already at the end, so the backspace takes the
            // second chip from its right edge.
            let shown = buffer.text_without_chips();
            assert!(buffer.backspace());
            let left = buffer.text_without_chips();
            assert!(
                left.starts_with("[Copied: "),
                "the first chip is intact: {left:?}"
            );
            assert_eq!(
                buffer.text(),
                format!("{first} and "),
                "and the second one's text went with it ({shown:?} -> {left:?})"
            );
        }

        #[test]
        fn pasted_newlines_become_line_breaks() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("one\r\ntwo\tthree");
            assert_eq!(buffer.text(), "one\ntwo    three");
        }

        #[test]
        fn a_multi_line_draft_is_drawn_on_its_own_rows() {
            // Shift+Enter adds a newline; before this the editor drew one row
            // and ignored the break, so the extra lines were invisible.
            let mut buffer = LineBuffer::default();
            buffer.insert_text("one\ntwo");
            let lines = input_lines(&buffer, 4, 80);
            assert_eq!(lines.len(), 2, "each logical line gets a row: {lines:?}");
            assert!(lines[0].starts_with("one"), "{lines:?}");
            assert!(lines[1].starts_with("two"), "{lines:?}");
            // The cursor is at the end of the second line and must be visible.
            assert!(lines[1].contains("\x1b[7m"), "{lines:?}");
        }

        #[test]
        fn a_draft_taller_than_the_window_scrolls_to_the_cursor_line() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("a\nb\nc\nd\ne");
            let lines = input_lines(&buffer, 2, 80);
            assert_eq!(lines.len(), 2, "only the window is drawn: {lines:?}");
            assert!(
                lines[1].contains('e'),
                "the window follows the cursor to the last line: {lines:?}"
            );
        }

        #[test]
        fn the_arrows_move_inside_the_draft_before_the_history() {
            let mut buffer = LineBuffer::default();
            // Two lines, the cursor at the end of the second.
            buffer.insert_text("uno\ndos");
            assert!(buffer.up(), "there is a line above");
            assert_eq!(buffer.cursor, 3, "the cursor lands on the line above");
            assert!(!buffer.up(), "the first line has nowhere above it");
            assert!(buffer.down(), "there is a line below");
            assert_eq!(buffer.cursor, 7, "back down to the second line");
            assert!(!buffer.down(), "the last line has nowhere below it");
        }

        #[test]
        fn the_arrows_keep_the_column_where_they_can() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("abcdef\nxy");
            // Move into the first line, column five.
            buffer.cursor = 5;
            assert!(buffer.down());
            // The second line is two wide, so the column is clamped to its end.
            assert_eq!(buffer.text(), "abcdef\nxy");
            assert_eq!(buffer.cursor, 9, "clamped to the shorter line");
        }

        #[test]
        fn the_whole_input_block_is_cleared_every_frame() {
            // The rows a taller draft used stay on screen unless the frame
            // clears them: deleting a line left copies of it above the prompt.
            // The block is the input's reserved rows, whatever the draft needs.
            let mut frame = String::from("\x1b7");
            let last_row = input_bottom(24);
            let block_top = last_row.saturating_sub(INPUT_ROWS) + 1;
            for row in block_top..=last_row {
                use std::fmt::Write as _;
                let _ = write!(frame, "\x1b[{row};1H\x1b[2K");
            }
            for row in block_top..=last_row {
                assert!(
                    frame.contains(&format!("\x1b[{row};1H\x1b[2K")),
                    "row {row} must be cleared"
                );
            }
            assert_eq!(
                block_top,
                last_row - INPUT_ROWS + 1,
                "the block is the input's own rows"
            );
        }

        #[test]
        fn a_long_draft_wraps_instead_of_losing_its_tail() {
            // It used to be clipped to the width with a `…`, so the end of a
            // long message was simply not on screen.
            let mut buffer = LineBuffer::default();
            buffer.insert_text("abcdefghij");
            let lines = input_lines(&buffer, 8, 4);
            assert_eq!(lines.len(), 3, "ten characters at width four: {lines:?}");
            assert!(lines[0].contains('a'), "{lines:?}");
            assert!(lines[1].contains('e'), "{lines:?}");
            assert!(
                lines[2].contains('i'),
                "the tail is on its own row: {lines:?}"
            );
            assert!(
                !lines.iter().any(|line| line.contains('…')),
                "nothing is elided: {lines:?}"
            );
        }

        #[test]
        fn the_wrapped_window_follows_the_cursor() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text(&"x".repeat(40));
            // Two rows of window, width ten: the cursor is on the last row, so
            // the window shows the last two rows and not the first ones.
            let lines = input_lines(&buffer, 2, 10);
            assert_eq!(lines.len(), 2, "{lines:?}");
            assert!(
                lines[1].contains("\x1b[7m"),
                "the cursor row is drawn: {lines:?}"
            );
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
                receiver.try_recv().expect("the message was sent"),
                EditorEvent::Line("first\nsecond".to_owned(), Vec::new()),
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

#[cfg(not(unix))]
mod fallback {
    use super::Attachment;

    use std::io;
    use std::sync::Arc;

    use tokio::sync::mpsc;

    use crate::style::Style;

    /// A plain line reader for platforms without the Unix terminal editor.
    pub(crate) struct InteractiveInput {
        events: mpsc::UnboundedReceiver<EditorEvent>,
    }

    /// What the editor handed the repl; the same shape as the Unix build.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum EditorEvent {
        /// A line was submitted, with the files its draft asked to attach.
        Line(String, Vec<Attachment>),
        /// A keystroke asked for something that is not a line.
        Action(EditorAction),
        /// The editor ended: end of input.
        End,
    }

    /// The same actions, for a build without a terminal to read keys from.
    ///
    /// Nothing ever sends one — there is no key to intercept — but the repl is
    /// one body for both builds, so the type has to exist.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum EditorAction {
        /// Escape, on a running turn: stop it.
        CancelTurn,
        /// Ctrl-C, when the session has background jobs: stop them.
        CancelJobs,
        /// Ctrl-G, on a running turn: stop waiting for the call.
        Detach,
    }

    impl InteractiveInput {
        /// Starts a blocking line reader on stdin.
        pub(crate) fn start(_style: Style, _pinned: Arc<super::Pinned>) -> Result<Self, String> {
            let (sender, events) = mpsc::unbounded_channel();
            std::thread::spawn(move || {
                loop {
                    let mut line = String::new();
                    match io::stdin().read_line(&mut line) {
                        Ok(0) | Err(_) => {
                            let _ = sender.send(EditorEvent::End);
                            break;
                        }
                        Ok(_) => {
                            if sender
                                .send(EditorEvent::Line(line.trim_end().to_owned(), Vec::new()))
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            });
            Ok(Self { events })
        }

        /// A handle over a channel a test fills.
        #[cfg(test)]
        pub(crate) fn from_events(events: mpsc::UnboundedReceiver<EditorEvent>) -> Self {
            Self { events }
        }

        /// The next thing the editor handed over.
        pub(crate) async fn recv(&mut self) -> Option<EditorEvent> {
            self.events.recv().await
        }

        /// The next submitted line, ignoring what keystrokes asked for.
        pub(crate) async fn recv_line(&mut self) -> Option<Option<String>> {
            loop {
                match self.events.recv().await {
                    Some(EditorEvent::Line(line, _)) => return Some(Some(line)),
                    Some(EditorEvent::Action(_)) => continue,
                    Some(EditorEvent::End) | None => return Some(None),
                }
            }
        }
    }
}

#[cfg(not(unix))]
pub(crate) use fallback::{EditorAction, EditorEvent, InteractiveInput};
#[cfg(unix)]
pub(crate) use unix::{EditorAction, EditorEvent, InteractiveInput};

#[cfg(test)]
mod pinned_tests {
    use super::{Pinned, TodoItem, TodoStatus};

    use crate::tabs::SessionTab;

    #[test]
    fn the_pinned_footer_keeps_the_session_and_todos() {
        let pinned = Pinned::new();
        pinned.set_session("session".to_owned());
        pinned.set_todos(vec![TodoItem {
            content: "task".to_owned(),
            status: TodoStatus::Pending,
        }]);
        let (session, todos, tab) = pinned.snapshot();
        assert_eq!(session, "session");
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].content, "task");
        assert_eq!(tab, SessionTab::Chat, "a session opens on the conversation");
    }

    #[test]
    fn the_active_tab_is_shared_and_marks_the_footer_dirty() {
        // The editor changes it and the footer draws it, and they are two
        // threads: the shared handle is what keeps them agreeing.
        let pinned = Pinned::new();
        assert_eq!(pinned.tab(), SessionTab::Chat);
        pinned.set_tab(SessionTab::Debug);
        assert_eq!(pinned.tab(), SessionTab::Debug);
        assert_eq!(pinned.snapshot().2, SessionTab::Debug);
    }

    #[test]
    fn the_debug_tab_keeps_what_used_to_be_printed() {
        // It was printed at startup and scrolled away by the first answer, so
        // keeping it is the whole point of the tab.
        let pinned = Pinned::new();
        assert!(pinned.debug().is_empty());
        pinned.set_debug(vec!["model: test/scripted".to_owned()]);
        assert_eq!(pinned.debug(), ["model: test/scripted"]);
    }

    #[test]
    fn the_waiting_steers_are_what_the_footer_draws() {
        // The list the editor draws comes from the same handle the repl writes,
        // so a message stays on screen until it is applied and leaves then.
        let pinned = Pinned::new();
        assert!(pinned.pending_steers().is_empty());
        pinned.set_pending_steers(vec!["first".to_owned(), "second".to_owned()]);
        assert_eq!(pinned.pending_steers(), ["first", "second"]);
        pinned.set_pending_steers(vec!["second".to_owned()]);
        assert_eq!(pinned.pending_steers(), ["second"]);
        pinned.set_pending_steers(Vec::new());
        assert!(pinned.pending_steers().is_empty());
    }
}
