//! Interactive terminal input with a line editor pinned below the model output.
//!
//! While a turn streams, typed text stays in the input line and never leaks
//! into the answer. Pressing Enter sends the line to the caller, which steers
//! it into the next turn boundary. The terminal keeps a scroll region above the
//! footer, so streamed output scrolls without touching the pinned rows: the
//! running session totals, the dimmed command suggestion and the input line
//! itself.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

/// A question from the model, waiting for an answer.
///
/// It is drawn where the draft is drawn, because that is the part of the screen
/// the person is already looking at, and the editor is the only thing that can
/// read a key while a turn runs. The choices come first and the input box is
/// last: the box is only the answer once "something else" is chosen, so until
/// then the arrow keys walk the list and the box stays out of the way.
///
/// The question is drawn above the choices, on the rows the draft would use:
/// while a turn runs the transcript is not where the person is looking, and the
/// block the editor owns is the only thing it can draw on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    /// The question itself, on the rows above the choices.
    pub(crate) prompt: String,
    /// The choices, in the order the model offered them.
    pub(crate) choices: Vec<String>,
    /// Where the cursor is: `0..choices.len()` is a choice, `choices.len()` is
    /// the "something else" row the editor adds itself.
    pub(crate) cursor: usize,
    /// Whether more than one choice may be taken.
    pub(crate) multiple: bool,
    /// The choices taken so far, when more than one may be.
    pub(crate) taken: Vec<usize>,
    /// Whether the input box is the answer, which it is once "something else"
    /// was chosen.
    pub(crate) typing: bool,
}

impl Asked {
    /// The index of the "something else" row, which is not in `choices`.
    pub(crate) fn other(&self) -> usize {
        self.choices.len()
    }

    /// Whether the cursor is on the row that opens the box.
    pub(crate) fn on_other(&self) -> bool {
        self.cursor >= self.other()
    }
}

/// What the person did with a question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The answer typed into the box.
    Typed(String),
    /// The choices taken, in the order they were taken.
    Taken(Vec<usize>),
    /// The question was left unanswered.
    Left,
}

/// Everything the footer draws, read under one lock.
///
/// One lock for the lot: taking each field on its own lets a writer land
/// between two of them, and the row would then show a summary belonging to a
/// different paint than the tab name beside it.
#[cfg(test)]
struct Snapshot {
    session: String,
    todos: Vec<TodoItem>,
    tab: SessionTab,
    tool_summary: String,
    mcp_summary: String,
}

/// The pinned text above the input line.
///
/// The renderer owns the session totals; the input editor owns the terminal.
/// Both share this handle: the renderer replaces the text, the editor redraws
/// it when it changes.
pub(crate) struct Pinned {
    inner: Mutex<PinnedInner>,
    dirty: AtomicBool,
    resized: AtomicBool,
    /// Whether a panel is on screen, and whether Escape has closed it.
    ///
    /// The keyboard is read on its own thread and the panel is drawn on the
    /// other, so the two speak through here: the reader marks it closed and the
    /// frame stops drawing it. Flags and not a channel, because the state is
    /// one bit and the reader never waits for an answer.
    modal_closed: AtomicBool,
    /// Whether a panel is on screen right now.
    modal_open: AtomicBool,
    /// How far the panel is scrolled back from the end, in lines.
    modal_scroll: AtomicUsize,
}

impl Default for PinnedInner {
    fn default() -> Self {
        Self {
            session: String::new(),
            todos: Vec::new(),
            asked: None,
            tab: SessionTab::Stats,
            pending_steers: Vec::new(),
            tool_summary: String::new(),
            mcp_summary: String::new(),
            draft: String::new(),
            draft_cursor: 0,
        }
    }
}

struct PinnedInner {
    session: String,
    todos: Vec<TodoItem>,
    /// The question being answered, when the model asked one.
    asked: Option<Asked>,
    /// Which tab is showing. What the session has spent is what a client opens
    /// on: it is the line the footer carried before the other tabs existed.
    tab: SessionTab,
    /// The one-row summary the tools tab shows.
    ///
    /// It is built where the rules live and handed over as text: a client that
    /// counted the rules itself would be a second place that knows what `deny`
    /// winning over `allow` means, and the two would drift.
    tool_summary: String,
    /// The one-row summary the MCP tab shows.
    mcp_summary: String,
    /// The draft as the key reader last had it.
    ///
    /// It is a copy and not the buffer itself: the reader owns the buffer and
    /// the frame reads this, so a frame never waits for a keystroke and a
    /// keystroke never waits for a frame.
    draft: String,
    /// Where the cursor is in the draft, in characters.
    draft_cursor: usize,

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
impl Default for Pinned {
    fn default() -> Self {
        Self {
            inner: Mutex::new(PinnedInner::default()),
            dirty: AtomicBool::new(false),
            resized: AtomicBool::new(false),
            modal_closed: AtomicBool::new(false),
            modal_open: AtomicBool::new(false),
            modal_scroll: AtomicUsize::new(0),
        }
    }
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

    /// Puts a question on screen, with the cursor on the first choice.
    ///
    /// It marks the footer dirty, which is what makes the editor repaint it: the
    /// editor draws the question, and this is the only writer it has.
    pub(crate) fn ask(&self, prompt: String, choices: Vec<String>, multiple: bool) {
        let mut inner = self.lock();
        inner.asked = Some(Asked {
            prompt,
            choices,
            cursor: 0,
            multiple,
            taken: Vec::new(),
            typing: false,
        });
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Moves the cursor of the question on screen.
    fn move_cursor(&self, delta: isize) {
        let mut inner = self.lock();
        let Some(asked) = inner.asked.as_mut() else {
            return;
        };
        if asked.typing {
            return;
        }
        let last = asked.other();
        asked.cursor = if delta < 0 {
            asked.cursor.saturating_sub(delta.unsigned_abs())
        } else {
            asked.cursor.saturating_add(delta.unsigned_abs()).min(last)
        };
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Marks or unmarks the choice under the cursor, when several may be taken.
    ///
    /// A single-choice question moves the cursor instead, which is what makes
    /// space mean "select this one" in both without a separate keymap.
    fn toggle(&self) {
        let mut inner = self.lock();
        let Some(asked) = inner.asked.as_mut() else {
            return;
        };
        if asked.typing || asked.on_other() {
            return;
        }
        if !asked.multiple {
            asked.cursor = asked.cursor.min(asked.other());
        } else if let Some(at) = asked.taken.iter().position(|index| *index == asked.cursor) {
            asked.taken.remove(at);
        } else {
            asked.taken.push(asked.cursor);
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Opens the input box, which is the answer once "something else" is chosen.
    fn type_answer(&self) {
        let mut inner = self.lock();
        let Some(asked) = inner.asked.as_mut() else {
            return;
        };
        asked.typing = true;
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Takes the choice the cursor is on as the answer, when that is what it is.
    ///
    /// `None` means the list does not answer the question here: either there is
    /// no question, or the cursor is on the row that opens the box — answering
    /// "something else" by its own label would send the label, not the answer.
    ///
    /// A question that takes several choices answers with what was marked, and
    /// with the row under the cursor when nothing was: a multi-select submitted
    /// with nothing marked would answer with an empty string, which is not an
    /// answer anybody gave.
    fn pick(&self) -> Option<Answer> {
        let mut inner = self.lock();
        let asked = inner.asked.as_ref()?;
        if asked.typing || asked.on_other() {
            return None;
        }
        let answer = if asked.multiple {
            let mut taken = asked.taken.clone();
            if taken.is_empty() {
                taken.push(asked.cursor);
            }
            Answer::Taken(taken)
        } else {
            Answer::Taken(vec![asked.cursor])
        };
        let _ = inner.asked.take();
        self.dirty.store(true, Ordering::Relaxed);
        Some(answer)
    }

    /// Takes the question off screen, whatever was answered.
    ///
    /// The answer travels as an [`EditorEvent::Question`], because the editor is
    /// what reads the key and so the only thing that knows what the key meant.
    /// This clears the block once that answer is on its way, and doing nothing
    /// when there is no question keeps it safe to call on every path out of the
    /// wait: a question left on screen is one the person can see and nobody can
    /// answer any more.
    pub(crate) fn retire(&self) {
        let mut inner = self.lock();
        if inner.asked.take().is_some() {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Whether the input box belongs to the question rather than to the draft.
    fn asking(&self) -> bool {
        self.lock().asked.as_ref().is_some_and(|asked| asked.typing)
    }

    /// Whether a question is on screen at all.
    pub(crate) fn is_asking(&self) -> bool {
        self.lock().asked.is_some()
    }

    /// Replaces the todo widget. An empty list clears it.
    pub(crate) fn set_todos(&self, todos: Vec<TodoItem>) {
        let mut inner = self.lock();
        if inner.todos != todos {
            inner.todos = todos;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }
    /// Replaces the tools summary the footer shows for the tools tab.
    ///
    /// The text is the whole line, already styled: the tab is one row and what
    /// goes in it is decided where the rules are, so the editor draws what it
    /// is given and never counts a rule itself.
    pub(crate) fn set_tool_summary(&self, text: String) {
        let mut inner = self.lock();
        if inner.tool_summary != text {
            inner.tool_summary = text;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Replaces the MCP summary the footer shows for the MCP tab.
    pub(crate) fn set_mcp_summary(&self, text: String) {
        let mut inner = self.lock();
        if inner.mcp_summary != text {
            inner.mcp_summary = text;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// A snapshot of the todo list, in the model's order.
    #[cfg(test)]
    pub(crate) fn todos(&self) -> Vec<TodoItem> {
        self.lock().todos.clone()
    }

    /// Asks for the panel to be closed, and says whether one was open.
    ///
    /// The answer is what the reader needs: Escape means "close the panel" when
    /// there is one and "cancel the turn" when there is not, and only the
    /// screen knows which.
    pub(crate) fn close_modal(&self) -> bool {
        if self.modal_open.load(Ordering::Relaxed) {
            self.modal_closed.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Puts the panel back where it was opened, so a scroll somebody left
    /// behind is not the one they meet on the next report.
    pub(crate) fn reset_modal_scroll(&self) {
        self.modal_scroll.store(0, Ordering::Relaxed);
    }

    /// Moves the panel, and says whether there was one to move.
    ///
    /// The offset is in lines and is added to whatever is there. The reader
    /// never waits for an answer: the frame is what clamps, because it is what
    /// knows how many rows fit.
    pub(crate) fn scroll_modal(&self, delta: isize) -> bool {
        if !self.modal_open.load(Ordering::Relaxed) {
            return false;
        }
        // The offset is a line count a report never approaches; saturating
        // keeps a wild one from wrapping into a negative scroll.
        let at = self.modal_scroll.load(Ordering::Relaxed);
        let moved = if delta < 0 {
            at.saturating_sub(delta.unsigned_abs())
        } else {
            at.saturating_add(delta.unsigned_abs())
        };
        self.modal_scroll.store(moved, Ordering::Relaxed);
        true
    }

    /// How far the panel should be scrolled.
    pub(crate) fn modal_scroll(&self) -> usize {
        self.modal_scroll.load(Ordering::Relaxed)
    }

    /// Records whether a panel is on screen, called by whoever draws it.
    pub(crate) fn set_modal_open(&self, open: bool) {
        self.modal_open.store(open, Ordering::Relaxed);
        if !open {
            self.modal_closed.store(false, Ordering::Relaxed);
        }
    }

    /// Takes the request to close the panel, if one was made.
    pub(crate) fn take_modal_closed(&self) -> bool {
        self.modal_closed.swap(false, Ordering::Relaxed)
    }

    /// Records the draft, so the frame draws what is being typed.
    pub(crate) fn set_draft(&self, text: String, cursor: usize) {
        let mut inner = self.lock();
        if inner.draft != text || inner.draft_cursor != cursor {
            inner.draft = text;
            inner.draft_cursor = cursor;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// The draft and where the cursor is in it.
    pub(crate) fn draft(&self) -> (String, usize) {
        let inner = self.lock();
        (inner.draft.clone(), inner.draft_cursor)
    }

    /// The row the active tab shows.
    ///
    /// The tabs module decides what each one says from the state it holds, so
    /// the row and the pinned line cannot disagree about the same fact.
    pub(crate) fn tab_body(&self) -> String {
        let inner = self.lock();
        let todos: Vec<crate::input::TodoItem> = inner.todos.clone();
        match inner.tab {
            // `stats` has no body of its own: what it shows *is* the status
            // line, and drawing it here as well put the same sentence on two
            // rows — which is what a doubled footer looks like.
            SessionTab::Stats => String::new(),
            SessionTab::Todo => {
                crate::input::unix::todo_line(crate::style::Style::plain(), &todos, 80)
            }
            SessionTab::Tools => inner.tool_summary.clone(),
            SessionTab::Mcp => inner.mcp_summary.clone(),
        }
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

    /// The question and its cursor, for the editor that draws and drives it.
    #[cfg(test)]
    fn question(&self) -> Option<Asked> {
        self.lock().asked.clone()
    }

    /// Everything the footer draws, read under one lock.
    #[cfg(test)]
    fn snapshot(&self) -> Snapshot {
        let inner = self.lock();
        Snapshot {
            session: inner.session.clone(),
            todos: inner.todos.clone(),
            tab: inner.tab,
            tool_summary: inner.tool_summary.clone(),
            mcp_summary: inner.mcp_summary.clone(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PinnedInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Consumes the resize notice, if one is pending.
    pub(crate) fn take_resized(&self) -> bool {
        self.resized.swap(false, Ordering::Acquire)
    }
}

#[cfg(unix)]
mod unix {
    use super::Attachment;

    use std::fmt::Write as _;
    use std::io::{self, IsTerminal};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use tokio::sync::mpsc;

    use super::{Pinned, TodoItem, TodoStatus};
    use crate::interaction::command_suggestion;
    use crate::style::Style;
    use crate::tabs::SessionTab;

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
        /// The question on screen was answered.
        Question(crate::input::Answer),
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
        /// The thread reading keys, stopped when this is dropped. Absent for a
        /// test that drives the channel directly, which has no thread.
        ///
        /// It is never read, and that is the point: holding it is the whole of
        /// its job, because its `Drop` is what ends the thread.
        #[allow(dead_code)]
        reader: Option<Reader>,
    }

    impl InteractiveInput {
        /// Starts the key reader.
        ///
        /// It reads keys and reports what they meant; it does not draw. The
        /// terminal belongs to whoever is drawing the frame, and there is one
        /// such owner, so the reader is only the input half of the editor.
        ///
        /// The raw mode this needs is set by the terminal guard before this is
        /// called: putting the terminal in raw mode twice, from two places,
        /// would leave the restore answering a question nobody asked.
        pub(crate) fn start(pinned: Arc<Pinned>) -> Result<Self, String> {
            if !io::stdin().is_terminal() {
                return Err("interactive input needs a terminal on stdin".to_owned());
            }
            let (sender, events) = mpsc::unbounded_channel();
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = Arc::clone(&stop);
            let handle = thread::Builder::new()
                .name("norte-input".to_owned())
                .spawn(move || read_keys(&pinned, &sender, &thread_stop))
                .map_err(|error| format!("could not start the input reader: {error}"))?;

            Ok(Self {
                events,
                reader: Some(Reader {
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
                reader: None,
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
                    // A question answered while another prompt wanted a line:
                    // the answer is not that line, and dropping it would leave
                    // the question on screen with nothing driving it.
                    Some(EditorEvent::Question(_) | EditorEvent::Action(_)) => {}
                    Some(EditorEvent::End) | None => return Some(None),
                }
            }
        }
    }

    /// The thread reading keys, stopped when the input is dropped.
    ///
    /// It holds no terminal: what it owns is the thread, and stopping it is all
    /// this has to do. The terminal is put back by the guard that took it, and
    /// two guards restoring the same terminal would be one too many.
    struct Reader {
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
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
            self.shift_chips(1);
            self.chars.insert(self.cursor, character);
            self.cursor += 1;
        }

        /// Moves every chip at or after the cursor forward by `delta`.
        ///
        /// A chip's range is recorded in the same indices as `chars`, so text
        /// inserted in front of a chip moves that chip's characters and its
        /// range has to move with them. A range that is left behind points at a
        /// different run than the label it stands for, and `text()` then walks
        /// backwards through the two — which is the panic, not a cosmetic
        /// drift. Only ranges at or after the insertion point move: the chips
        /// before it are unaffected, and the cursor was already stepped out of
        /// any chip it was inside.
        fn shift_chips(&mut self, delta: usize) {
            for chip in &mut self.chips {
                if chip.range.start >= self.cursor {
                    chip.range.start += delta;
                    chip.range.end += delta;
                }
            }
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
            // The label goes in front of whatever follows, so every chip after
            // the cursor shifts by its length before the characters land.
            self.shift_chips(label.chars().count());
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
            let start = self.chars[..self.cursor]
                .iter()
                .rposition(|character| *character == '\n')
                .map_or(0, |index| index + 1);
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
            let above_start = self.chars[..above_end]
                .iter()
                .rposition(|character| *character == '\n')
                .map_or(0, |index| index + 1);
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

    /// Reads keys and reports what they meant, until the terminal closes.
    ///
    /// It is the input half of the editor and nothing else: the frame is drawn
    /// by whoever owns the terminal, from the state this updates. That split is
    /// what lets a keypress and a server event be handled in the same loop,
    /// which is what a frame needs and what two independent writers made
    /// impossible.
    #[allow(clippy::too_many_lines)]
    fn read_keys(pinned: &Pinned, sender: &mpsc::UnboundedSender<EditorEvent>, stop: &AtomicBool) {
        let mut buffer = LineBuffer::default();
        let mut history: Vec<String> = Vec::new();
        let mut history_pos: Option<usize> = None;
        let mut draft = String::new();
        // Where the keyboard is. `None` is the input line, which is where a
        // person starts and where every key belongs; `Some(tab)` is the tab row
        // with that tab selected but not yet applied.
        let mut focus: Option<SessionTab> = None;
        // The draft the answer box stepped over, kept to be handed back.
        //
        // The box draws on the same buffer the draft does, and an answer is not
        // a draft: what was half-written before the question arrived is not part
        // of the reply, and losing it would punish the person for answering.
        let mut stashed: Option<LineBuffer> = None;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            // A question retired from outside this loop — the reader clears the
            // block on its way out of the wait — gives the draft back, whoever
            // ended the question.
            if stashed.is_some() && !pinned.is_asking() {
                buffer = stashed.take().unwrap_or_default();
            }
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => {
                    let mut redraw = false;
                    match event::read() {
                        Ok(Event::Key(key))
                            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                        {
                            // A panel owns the keyboard while it is up: it is
                            // over the interface, so Escape closes it and the
                            // arrows walk it. A report longer than the screen is
                            // read a page at a time, and the row under it says
                            // how much there is.
                            let scrolling = match key.code {
                                KeyCode::Esc => Some(0),
                                KeyCode::Up => Some(-1),
                                KeyCode::Down => Some(1),
                                KeyCode::PageUp => Some(-10),
                                KeyCode::PageDown => Some(10),
                                _ => None,
                            };
                            if let Some(delta) = scrolling {
                                if key.code == KeyCode::Esc {
                                    if pinned.close_modal() {
                                        continue;
                                    }
                                } else if pinned.scroll_modal(delta) {
                                    continue;
                                }
                            }
                            // A question owns the keyboard before anything
                            // else does: it is what the model is waiting on, and
                            // the draft is not what the person is working on
                            // while it is on screen. The box opens only once
                            // "something else" is chosen, so until then every
                            // key belongs to the list and a stray letter is not
                            // an answer.
                            let asking = pinned.is_asking();
                            let typing = pinned.asking();
                            let mut to_input = true;
                            if asking {
                                to_input = false;
                                match asked_key(key, typing) {
                                    AskedKey::Up => pinned.move_cursor(-1),
                                    AskedKey::Down => pinned.move_cursor(1),
                                    AskedKey::Toggle => pinned.toggle(),
                                    AskedKey::Accept => {
                                        if typing {
                                            // The box is the answer: Enter takes what
                                            // it holds and closes the question. An empty
                                            // box is nobody's answer, so it stays open
                                            // until there is one.
                                            let text = buffer.text();
                                            if !text.trim().is_empty() {
                                                buffer.clear();
                                                pinned.retire();
                                                let _ = sender.send(EditorEvent::Question(
                                                    super::Answer::Typed(text),
                                                ));
                                            }
                                        } else if let Some(answer) = pinned.pick() {
                                            let _ = sender.send(EditorEvent::Question(answer));
                                        } else {
                                            // The cursor is on "something else", which
                                            // is the row that opens the box rather than
                                            // an answer of its own. The draft steps aside
                                            // so what was being written does not read as
                                            // the beginning of the answer.
                                            stashed = Some(std::mem::take(&mut buffer));
                                            pinned.type_answer();
                                        }
                                    }
                                    // Leaving is a decision, so the editor
                                    // reports it rather than making it.
                                    AskedKey::Leave => {
                                        pinned.retire();
                                        let _ =
                                            sender.send(EditorEvent::Question(super::Answer::Left));
                                    }
                                    // A key the question does not use goes to the
                                    // box while it is open and nowhere while it
                                    // is not.
                                    AskedKey::Input => to_input = typing,
                                }
                                redraw = true;
                            }
                            if let Some(selected) = focus.filter(|_| !asking) {
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
                        // A resize needs no handling here: the frame reads the
                        // terminal's size when it draws, so the next frame is
                        // already the right shape. Everything else the terminal
                        // can send is not a key.
                        _ => {}
                    }
                    if redraw {
                        // The frame draws the draft, and the draft lives here:
                        // this is the one place the two halves of the editor
                        // meet, and it is a copy rather than the buffer so
                        // neither waits for the other.
                        pinned.set_draft(buffer.text(), buffer.cursor);
                    }
                }
                Ok(false) => {}
                Err(_) => {
                    let _ = sender.send(EditorEvent::End);
                    break;
                }
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

    /// What a key means while a question is on screen.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum AskedKey {
        /// Move the cursor up the choices.
        Up,
        /// Move the cursor down the choices.
        Down,
        /// Mark or unmark the choice under the cursor.
        Toggle,
        /// Take the answer: opens the box on "something else", submits it
        /// anywhere else.
        Accept,
        /// Leave the question unanswered.
        Leave,
        /// Something the question does not use: the box, when it is open.
        Input,
    }

    /// Reads one key while a question is on screen.
    ///
    /// `typing` is whether the box is already open, and it changes what Enter
    /// means: while it is closed Enter is what opens it on the "something else"
    /// row and what submits an answer anywhere else, and while it is open Enter
    /// is a line like any other.
    fn asked_key(key: KeyEvent, typing: bool) -> AskedKey {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up if !typing => AskedKey::Up,
            KeyCode::Down if !typing => AskedKey::Down,
            // Space selects without committing, which is the only way to take
            // more than one choice, and it is what every list does.
            KeyCode::Char(' ') if !typing => AskedKey::Toggle,
            KeyCode::Enter => AskedKey::Accept,
            KeyCode::Esc => AskedKey::Leave,
            KeyCode::Char('c' | 'g') if control => AskedKey::Leave,
            _ => AskedKey::Input,
        }
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

    /// Builds the one-line todo widget: progress, the current item first, then
    /// pending items that fit. Completed items are represented by the count,
    /// and a `+N` says how many actionable items did not fit.
    pub(crate) fn todo_line(style: Style, todos: &[TodoItem], max: usize) -> String {
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

    #[cfg(test)]
    mod tests {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::tabs::SessionTab;

        use super::{
            EditorEvent, KeyOutcome, LineBuffer, TabRowKey, TodoItem, TodoStatus, handle_key,
            tab_row_key, todo_line,
        };

        #[test]
        fn the_tab_row_reads_its_own_keys() {
            // Left and right walk, Enter applies, up leaves. The row is what a
            // person reaches by pressing down one time too many, so it has to
            // be walkable and it has to be escapable.
            let key = |code, modifiers| KeyEvent::new(code, modifiers);
            assert_eq!(
                tab_row_key(key(KeyCode::Right, KeyModifiers::NONE), SessionTab::Stats),
                TabRowKey::Select(SessionTab::Todo)
            );
            assert_eq!(
                tab_row_key(key(KeyCode::Left, KeyModifiers::NONE), SessionTab::Stats),
                TabRowKey::Select(SessionTab::Mcp),
                "left from the first wraps to the last"
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
                tab_row_key(key(KeyCode::Tab, KeyModifiers::NONE), SessionTab::Stats),
                TabRowKey::Select(SessionTab::Todo)
            );
            assert_eq!(
                tab_row_key(key(KeyCode::BackTab, KeyModifiers::NONE), SessionTab::Stats),
                TabRowKey::Select(SessionTab::Mcp)
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

        /// A second label typed before the first must not fold the two chips
        /// together.
        ///
        /// Inserting into `chars` moves every character after the cursor, but a
        /// chip's range is recorded in those same indices. A range that is not
        /// moved with them ends up pointing at a different run than the label it
        /// stands for, and the next `text()` walks backwards through the two.
        #[test]
        fn inserting_before_a_chip_keeps_its_range_in_place() {
            let mut buffer = LineBuffer::default();
            buffer.insert_chip("[one]", "one".to_owned());
            assert_eq!(buffer.text(), "one");
            // Type in front of the chip: every character of the chip shifts by
            // one, so its range has to shift too.
            buffer.home();
            buffer.insert_text("x");
            assert_eq!(buffer.text(), "xone", "the label still stands for `one`");
            assert_eq!(buffer.text_without_chips(), "x[one]");
            // And the same the other way round: a chip after a chip.
            buffer.end();
            buffer.insert_chip("[two]", "two".to_owned());
            assert_eq!(buffer.text(), "xonetwo");
            assert_eq!(buffer.text_without_chips(), "x[one][two]");
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
    use super::{Answer, Pinned, TodoItem, TodoStatus};

    use crate::tabs::SessionTab;

    #[test]
    fn the_pinned_footer_keeps_the_session_and_todos() {
        let pinned = Pinned::new();
        pinned.set_session("session".to_owned());
        pinned.set_todos(vec![TodoItem {
            content: "task".to_owned(),
            status: TodoStatus::Pending,
        }]);
        let snapshot = pinned.snapshot();
        assert_eq!(snapshot.session, "session");
        assert_eq!(snapshot.todos.len(), 1);
        assert_eq!(snapshot.todos[0].content, "task");
        assert_eq!(
            snapshot.tab,
            SessionTab::Stats,
            "a session opens on the stats"
        );
        // The rows the tabs that count things show, which start empty because
        // nothing has been counted yet.
        assert_eq!(
            snapshot.tool_summary, "",
            "nothing has been counted for the tools row yet"
        );
        assert_eq!(
            snapshot.mcp_summary, "",
            "nothing has been counted for the MCP row yet"
        );
    }

    #[test]
    fn the_active_tab_is_shared_and_marks_the_footer_dirty() {
        // The editor changes it and the footer draws it, and they are two
        // threads: the shared handle is what keeps them agreeing.
        let pinned = Pinned::new();
        assert_eq!(pinned.tab(), SessionTab::Stats);
        pinned.set_tab(SessionTab::Todo);
        assert_eq!(pinned.tab(), SessionTab::Todo);
        assert_eq!(pinned.snapshot().tab, SessionTab::Todo);
    }

    #[test]
    fn the_waiting_steers_are_what_the_footer_draws() {
        // The list the editor draws comes from the same handle the repl writes,
        // so a message stays on screen until it is applied and leaves then.
        let pinned = Pinned::new();
        assert!(
            pinned.pending_steers().is_empty(),
            "nothing is waiting before one is set"
        );
        pinned.set_pending_steers(vec!["first".to_owned(), "second".to_owned()]);
        assert_eq!(pinned.pending_steers(), ["first", "second"]);
        pinned.set_pending_steers(vec!["second".to_owned()]);
        assert_eq!(pinned.pending_steers(), ["second"]);
        pinned.set_pending_steers(Vec::new());
        assert!(
            pinned.pending_steers().is_empty(),
            "clearing the list leaves nothing waiting"
        );
    }

    #[test]
    fn a_question_is_walked_with_the_arrows_and_the_box_waits_for_something_else() {
        // The point of the whole thing: the choices are walked, and the input
        // box is not the answer until the person says the answer is not on the
        // list. A box that is open from the start is one nobody knows what to
        // type into.
        let pinned = Pinned::new();
        pinned.ask(
            "which file?".to_owned(),
            vec!["src/main.rs".to_owned(), "src/lib.rs".to_owned()],
            false,
        );
        assert!(pinned.is_asking(), "the question is on screen");
        assert!(
            !pinned.asking(),
            "and the box is closed: nothing is typed yet"
        );
        let asked = pinned.question().expect("a question");
        assert_eq!(asked.cursor, 0, "the cursor starts on the first choice");
        assert_eq!(asked.other(), 2, "the extra row is after the choices");
        assert!(!asked.on_other());

        pinned.move_cursor(1);
        assert_eq!(pinned.question().expect("a question").cursor, 1);
        // The cursor reaches the "something else" row and holds there.
        pinned.move_cursor(1);
        assert!(pinned.question().expect("a question").on_other());
        pinned.move_cursor(1);
        assert_eq!(
            pinned.question().expect("a question").cursor,
            2,
            "the last row is the end of the list"
        );
        pinned.move_cursor(-1);
        pinned.move_cursor(-1);
        pinned.move_cursor(-1);
        assert_eq!(pinned.question().expect("a question").cursor, 0);

        // Enter on a choice answers with it; the box never opened.
        let answer = pinned.pick();
        assert_eq!(answer, Some(Answer::Taken(vec![0])));
        assert!(!pinned.is_asking(), "the question left the screen");
    }

    #[test]
    fn opening_the_box_is_what_makes_the_input_the_answer() {
        let pinned = Pinned::new();
        pinned.ask(
            "what is it?".to_owned(),
            vec!["a".to_owned(), "b".to_owned()],
            false,
        );
        // Move to the "something else" row and open the box there.
        pinned.move_cursor(2);
        assert!(pinned.question().expect("a question").on_other());
        pinned.type_answer();
        assert!(pinned.asking(), "the box is open, so keys are its own");
        // The row that opens the box does not answer the question: picking it
        // would send the label "something else" as the answer, and while the
        // box is open the line is the answer and the list is not.
        assert_eq!(pinned.pick(), None);
        assert!(
            pinned.is_asking(),
            "and the question is still there to be answered"
        );
    }

    #[test]
    fn several_choices_may_be_taken_and_they_keep_their_order() {
        let pinned = Pinned::new();
        pinned.ask(
            "which ones?".to_owned(),
            vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
            true,
        );
        pinned.toggle();
        pinned.move_cursor(2);
        pinned.toggle();
        let question = pinned.question().expect("a question");
        assert_eq!(question.taken, vec![0, 2], "in the order they were marked");
        // Marking the same one again unmarks it.
        pinned.toggle();
        assert_eq!(pinned.question().expect("a question").taken, vec![0]);
        assert_eq!(pinned.pick(), Some(Answer::Taken(vec![0])));
    }

    #[test]
    fn a_multi_select_cursor_answers_when_nothing_was_marked() {
        // Enter with nothing marked used to answer with an empty string: the
        // person walked to a choice and submitted, and the model was told
        // nothing. The row the cursor is on is the choice they were looking at.
        let pinned = Pinned::new();
        pinned.ask(
            "which ones?".to_owned(),
            vec!["one".to_owned(), "two".to_owned()],
            true,
        );
        pinned.move_cursor(1);
        assert_eq!(pinned.pick(), Some(Answer::Taken(vec![1])));
        assert!(!pinned.is_asking());
    }

    #[test]
    fn leaving_a_question_takes_it_off_the_screen() {
        // Leaving is a decision of its own: the reader says so rather than
        // inventing a choice nobody made, and the block is cleared so the
        // question does not outlive the answer.
        let pinned = Pinned::new();
        pinned.ask("which?".to_owned(), vec!["a".to_owned()], false);
        // The row that opens the box is not an answer by itself.
        pinned.move_cursor(1);
        assert_eq!(pinned.pick(), None, "the box row is not an answer");
        assert!(
            pinned.is_asking(),
            "and a question nobody answered stays on screen"
        );
        pinned.retire();
        assert!(!pinned.is_asking());
        // Retiring one that is not there is not an error: the wait says so on
        // every way out, including the ones with nothing to retire.
        pinned.retire();
        assert!(!pinned.is_asking());
    }

    #[test]
    fn a_question_with_no_choices_still_has_something_else() {
        // A model may ask a question and offer nothing: the list is then just
        // the row that opens the box, which is the only thing to do.
        let pinned = Pinned::new();
        pinned.ask("what do you think?".to_owned(), Vec::new(), false);
        let asked = pinned.question().expect("a question");
        assert_eq!(asked.other(), 0);
        assert!(asked.on_other(), "the only row is the box");
        pinned.type_answer();
        assert!(pinned.asking());
    }
}
