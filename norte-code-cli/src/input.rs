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

/// One MCP server, as the sidebar renders it.
///
/// It is a configured entry and not a connection: a server that is off, or that
/// failed to start, is exactly what a person wants to see in the column, so the
/// row exists whether or not anything is running behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpRow {
    /// The name the configuration gave it.
    pub(crate) name: String,
    /// How many tools it publishes, or zero when none connected.
    pub(crate) tools: usize,
    /// What the row says about it.
    pub(crate) state: McpState,
}

/// What a configured MCP server is doing, as the sidebar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum McpState {
    /// Connected, and its tools reach the model.
    On,
    /// Connected, but switched off for the turn.
    Off,
    /// Configured off, so it was never started.
    Disabled,
    /// It did not start; the string is why.
    Failed(String),
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

/// The text a selection holds, from the rows the last frame painted.
///
/// The corners come in any order — a drag may go up and to the left as easily
/// as down and to the right — so they are sorted first. The first and last rows
/// are cut at the selected columns and the rows between them are taken whole,
/// which is what a terminal does with a selection and what makes a drag over a
/// wrapped paragraph yield the paragraph.
#[must_use]
fn text_in(range: ((u16, u16), (u16, u16)), rows: &[String]) -> String {
    let ((c1, r1), (c2, r2)) = range;
    let (top, left) = if (r1, c1) <= (r2, c2) {
        (r1, c1)
    } else {
        (r2, c2)
    };
    let (bottom, right) = if (r1, c1) <= (r2, c2) {
        (r2, c2)
    } else {
        (r1, c1)
    };
    let mut out: Vec<String> = Vec::new();
    for row in top..=bottom {
        let Some(line) = rows.get(usize::from(row)) else {
            continue;
        };
        let chars: Vec<char> = line.chars().collect();
        // Only the first and the last row are cut at a column. The rows between
        // them are taken whole — a drag that spans three rows means all of the
        // middle one — and the first row starts where the drag did while the
        // last ends where it did. A one-row drag is both at once, which is why
        // the first row is cut at its start and the last at its end.
        let from = if row == top {
            usize::from(left).min(chars.len())
        } else {
            0
        };
        let to = if row == bottom {
            usize::from(right).saturating_add(1).min(chars.len())
        } else {
            chars.len()
        };
        let slice: String = chars[from.min(to)..to].iter().collect();
        out.push(slice.trim_end().to_owned());
    }
    // A drag that starts on a blank row above the text picks up the blank rows
    // it passed over, and they are not part of what the person meant to copy:
    // the answer is what they dragged *to*. Leading and trailing blanks go, and
    // the rows between stay — three selected rows of a paragraph are three
    // lines even when the middle one is empty.
    while out.first().is_some_and(|row| row.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|row| row.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
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
    /// Whether it can be acted on.
    modal_editable: AtomicBool,
    /// How far the panel is scrolled back from the end, in lines.
    modal_scroll: AtomicUsize,
    /// Whether the left button is down, so a drag is a selection.
    ///
    /// It lives beside the selection and not in the frame: the button and the
    /// motion arrive on the keyboard's thread, and the frame only reads the
    /// result.
    selecting: AtomicBool,
    /// How many lines the conversation is scrolled back from its end.
    ///
    /// Zero follows the tail. The reader moves it with the page keys and the
    /// frame draws from it, and neither waits: a page key that blocked until
    /// the next frame would stutter under a fast hand.
    transcript_scroll: AtomicUsize,
    /// Which row of the completion menu is chosen.
    ///
    /// The menu is built where the registry is, but the choice is the
    /// keyboard's: the reader moves it and the frame draws it, and neither
    /// waits for the other.
    completion_pick: AtomicUsize,
}

#[derive(Default)]
struct PinnedInner {
    session: String,
    todos: Vec<TodoItem>,
    /// The question being answered, when the model asked one.
    asked: Option<Asked>,
    /// The connected MCP servers, as the sidebar draws them.
    mcp: Vec<McpRow>,
    /// What the mouse has selected, in cells of the frame.
    ///
    /// A cell is a column and a row of the terminal, and the range is the two
    /// corners: the anchor where the drag began and the end where it is now.
    selection: Option<((u16, u16), (u16, u16))>,
    /// The rows the last frame painted, so a selection can be turned into text.
    painted: Vec<String>,
    /// The draft as the key reader last had it.
    ///
    /// It is a copy and not the buffer itself: the reader owns the buffer and
    /// the frame reads this, so a frame never waits for a keystroke and a
    /// keystroke never waits for a frame.
    draft: String,
    /// Where the cursor is in the draft, in characters.
    draft_cursor: usize,
    /// The other end of the draft's selection, when there is one.
    draft_anchor: Option<usize>,

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
            modal_editable: AtomicBool::new(false),
            modal_scroll: AtomicUsize::new(0),
            selecting: AtomicBool::new(false),
            completion_pick: AtomicUsize::new(0),
            transcript_scroll: AtomicUsize::new(0),
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

    /// Replaces the MCP server list the sidebar shows. Empty clears it.
    pub(crate) fn set_mcp(&self, servers: Vec<McpRow>) {
        let mut inner = self.lock();
        if inner.mcp != servers {
            inner.mcp = servers;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// The connected MCP servers, in configuration order.
    pub(crate) fn mcp(&self) -> Vec<McpRow> {
        self.lock().mcp.clone()
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
    ///
    /// The sidebar draws it, so it is read on every frame and not only by a
    /// test: the list is the model's plan, and it belongs beside the work.
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
            // Escape is handled on the keyboard's thread and the panel is taken
            // down by the frame. Nothing drew the frame unless something else
            // had changed, so the panel stayed on screen after the key that
            // closed it.
            self.dirty.store(true, Ordering::Relaxed);
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
        if moved != at {
            self.modal_scroll.store(moved, Ordering::Relaxed);
            self.dirty.store(true, Ordering::Relaxed);
        }
        true
    }

    /// How far the panel should be scrolled.
    pub(crate) fn modal_scroll(&self) -> usize {
        self.modal_scroll.load(Ordering::Relaxed)
    }

    /// Moves the conversation, in pages or in lines.
    ///
    /// Up goes back in time, which is the direction the keys point: what is
    /// above is older. The offset counts from the end, so a line arriving does
    /// not move what is being read — a person reading history stays where they
    /// are, and one at the tail sees the newest line because zero is the tail.
    pub(crate) fn scroll_transcript(&self, delta: isize) {
        let at = self.transcript_scroll.load(Ordering::Relaxed);
        // The offset counts from the end, so a negative delta — the page-up
        // direction, towards what is older — moves it *further* from the tail.
        // It was subtracted the other way round, which left the keys doing
        // nothing at the tail: `0 - 24` saturates back to zero, which is where
        // they started, so `PageUp` and `Ctrl-Home` were no-ops until something
        // else moved the offset first.
        let moved = at.saturating_add_signed(-delta);
        if moved != at {
            self.transcript_scroll.store(moved, Ordering::Relaxed);
            // The frame draws the offset, so moving it is a change like any
            // other. Without this the tick had nothing to draw and the
            // conversation stayed where it was: the keys moved a number nobody
            // read.
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Puts the conversation back on its newest line.
    pub(crate) fn follow_tail(&self) {
        if self.transcript_scroll.swap(0, Ordering::Relaxed) != 0 {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// How far the conversation is scrolled back from its end.
    pub(crate) fn transcript_scroll(&self) -> usize {
        self.transcript_scroll.load(Ordering::Relaxed)
    }

    /// Moves the completion menu, and says whether there was one to move.
    ///
    /// It wraps, because a list of a handful of rows is walked with one key: a
    /// menu that stopped at its last row would need the other key to come back,
    /// and the arrow that does nothing reads as broken.
    pub(crate) fn move_completion(&self, delta: isize, rows: usize) -> bool {
        if rows == 0 {
            return false;
        }
        let at = self.completion_pick.load(Ordering::Relaxed);
        let count = isize::try_from(rows).unwrap_or(isize::MAX);
        let moved = (isize::try_from(at).unwrap_or(0) + delta).rem_euclid(count);
        self.completion_pick
            .store(usize::try_from(moved).unwrap_or(0), Ordering::Relaxed);
        true
    }

    /// Which row of the completion menu is chosen.
    pub(crate) fn completion_pick(&self) -> usize {
        self.completion_pick.load(Ordering::Relaxed)
    }

    /// Puts the completion choice back on the first row.
    ///
    /// The menu is rebuilt from the line on every keystroke, so the choice goes
    /// with it: a row picked under one prefix is not a row the next line offers,
    /// and keeping it would complete to something nobody chose.
    pub(crate) fn reset_completion_pick(&self) {
        self.completion_pick.store(0, Ordering::Relaxed);
    }

    /// Records whether a panel is on screen, called by whoever draws it.
    pub(crate) fn set_modal_open(&self, open: bool) {
        self.modal_open.store(open, Ordering::Relaxed);
        if !open {
            self.modal_closed.store(false, Ordering::Relaxed);
        }
        // Opening a panel is a change to draw: the command that asked for it
        // returns to the prompt and the frame is written on the next tick,
        // which only writes when something moved.
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Takes the request to close the panel, if one was made.
    pub(crate) fn take_modal_closed(&self) -> bool {
        self.modal_closed.swap(false, Ordering::Relaxed)
    }

    /// Records the draft, so the frame draws what is being typed.
    ///
    /// `anchor` is the other end of the selection, when there is one: the frame
    /// draws the range it and the cursor describe, so an inverted run on screen
    /// is the selection the editor holds.
    pub(crate) fn set_draft(&self, text: String, cursor: usize, anchor: Option<usize>) {
        let mut inner = self.lock();
        if inner.draft != text || inner.draft_cursor != cursor || inner.draft_anchor != anchor {
            inner.draft = text;
            inner.draft_cursor = cursor;
            inner.draft_anchor = anchor;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// The draft, where the cursor is in it, and the other end of the selection.
    pub(crate) fn draft(&self) -> (String, usize, Option<usize>) {
        let inner = self.lock();
        (inner.draft.clone(), inner.draft_cursor, inner.draft_anchor)
    }

    /// The question on screen and its cursor, for the frame that draws it.
    ///
    /// The frame reads it the way it reads the draft: the editor owns the
    /// question and the frame draws what it is told, and neither waits for the
    /// other. It used to be a test-only reader, which is why the frame drew no
    /// question at all.
    pub(crate) fn question(&self) -> Option<Asked> {
        self.lock().asked.clone()
    }

    /// Everything the footer draws, read under one lock.
    #[cfg(test)]
    fn snapshot(&self) -> Snapshot {
        let inner = self.lock();
        Snapshot {
            session: inner.session.clone(),
            todos: inner.todos.clone(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PinnedInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether anything changed since the last frame was drawn.
    ///
    /// The flag was written by every setter and read by nobody: the frame was
    /// drawn on a clock instead, so a client sitting at a prompt with nothing
    /// happening wrote a frame every 120ms — thousands of cursor hide/show
    /// pairs a second, and a core burned to draw the same screen. The reader
    /// takes it; what it means is "something moved since the frame".
    pub(crate) fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    /// The range of the screen the mouse has selected, if any.
    ///
    /// It is in cells of the frame: a column and a row of the terminal, top
    /// left to bottom right. It is kept here because the two halves of the
    /// editor are on different threads — the mouse is read by the keyboard's
    /// and the text is drawn by the frame's — and because a selection is a
    /// property of what is on screen, not of the transcript behind it.
    pub(crate) fn selection(&self) -> Option<((u16, u16), (u16, u16))> {
        let inner = self.lock();
        inner.selection
    }

    /// Starts a selection at a cell.
    pub(crate) fn begin_selection(&self, column: u16, row: u16) {
        let mut inner = self.lock();
        inner.selection = Some(((column, row), (column, row)));
        drop(inner);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Moves the end of the selection, which is what a drag does.
    pub(crate) fn extend_selection(&self, column: u16, row: u16) {
        let mut inner = self.lock();
        if let Some((start, _)) = inner.selection {
            inner.selection = Some((start, (column, row)));
            drop(inner);
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Ends the selection and hands back the text that was in it.
    ///
    /// The frame is what turns cells into text: only it knows what was drawn
    /// where, and the transcript behind it may have been wrapped differently.
    /// So the frame records the rows it painted and this reads them, which is
    /// also why the copy happens on the frame's thread and not on the mouse's.
    pub(crate) fn finish_selection(&self) -> Option<String> {
        let mut inner = self.lock();
        let range = inner.selection.take()?;
        let rows = std::mem::take(&mut inner.painted);
        drop(inner);
        self.dirty.store(true, Ordering::Relaxed);
        let text = text_in(range, &rows);
        (!text.trim().is_empty()).then_some(text)
    }

    /// What the frame painted, for a test that is asking why a copy failed.
    #[cfg(test)]
    pub(crate) fn debug_painted(&self) -> Vec<String> {
        self.lock().painted.clone()
    }

    /// Records what the frame painted, so a selection can be read back.
    pub(crate) fn set_painted(&self, rows: Vec<String>) {
        self.lock().painted = rows;
    }

    /// Whether the panel on screen can be acted on.
    ///
    /// It is a copy of what the frame holds and not the panel itself: the two
    /// are on different threads, and the reader only needs to know which keys
    /// belong to the panel and which to the report's scroll.
    pub(crate) fn set_modal_editable(&self, editable: bool) {
        self.modal_editable.store(editable, Ordering::Relaxed);
    }

    /// Whether the panel on screen takes the keys that act on a row.
    pub(crate) fn modal_is_editable(&self) -> bool {
        self.modal_editable.load(Ordering::Relaxed)
    }

    /// Consumes the resize notice, if one is pending.
    pub(crate) fn take_resized(&self) -> bool {
        self.resized.swap(false, Ordering::Acquire)
    }
}

#[cfg(unix)]
mod unix {
    use super::Attachment;

    use std::io::{self, IsTerminal};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use tokio::sync::mpsc;

    use super::Pinned;

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
        /// A selection was copied, with the text that went to the clipboard.
        Copied(String),
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
        /// A key a panel owns: the panel decides what it means, and the loop
        /// that owns the service carries the decision out.
        Panel(crate::modal::Key),
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
                    Some(
                        EditorEvent::Question(_) | EditorEvent::Action(_) | EditorEvent::Copied(_),
                    ) => {}
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
        /// The other end of a selection, when there is one.
        ///
        /// The cursor is one end and this is the other: a selection is a range
        /// the two describe together, and it is `None` when the cursor is the
        /// only end — which is almost always.
        anchor: Option<usize>,
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
            // Typing over a selection replaces it, which is what every editor
            // does and what the delete key on a selection already means.
            self.delete_selection();
            self.settle_cursor();
            self.shift_chips(1);
            self.chars.insert(self.cursor, character);
            self.cursor += 1;
        }

        /// The selected range, if there is one, as `(lo, hi)` in `chars` order.
        fn selection(&self) -> Option<(usize, usize)> {
            let anchor = self.anchor?;
            let (lo, hi) = if anchor <= self.cursor {
                (anchor, self.cursor)
            } else {
                (self.cursor, anchor)
            };
            (lo != hi).then_some((lo, hi))
        }

        /// The far end of the selection, or `None` when there is none.
        ///
        /// It is the anchor when a selection exists and `None` when the anchor
        /// and the cursor coincide, so the frame is handed a range only when
        /// there is one to draw.
        fn selection_anchor(&self) -> Option<usize> {
            self.selection()?;
            self.anchor
        }

        /// Starts a selection at the cursor, if one has not been started.
        ///
        /// A shifted motion is the start of one: the first shifted arrow anchors
        /// the selection where the cursor was, and every one after it moves the
        /// other end.
        fn start_selection(&mut self) {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        }

        /// Selects the whole draft.
        fn select_all(&mut self) {
            self.anchor = Some(0);
            self.cursor = self.chars.len();
        }

        /// Removes the selection, if there is one, and says whether it did.
        fn delete_selection(&mut self) -> bool {
            let Some((lo, hi)) = self.selection() else {
                return false;
            };
            self.anchor = None;
            self.remove_range(lo, hi)
        }

        /// Collapses the selection to one of its ends, and says whether there
        /// was one. A plain Left or Right over a selection moves to its edge
        /// rather than by one character, which is what every editor does.
        fn collapse_selection(&mut self, to_end: bool) -> bool {
            let Some((lo, hi)) = self.selection() else {
                return false;
            };
            self.cursor = if to_end { hi } else { lo };
            self.anchor = None;
            true
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
            self.delete_selection();
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
            self.anchor = None;
            true
        }

        fn backspace(&mut self) -> bool {
            if self.delete_selection() {
                return true;
            }
            if self.cursor == 0 {
                return false;
            }
            self.remove_range(self.cursor - 1, self.cursor)
        }

        fn delete(&mut self) -> bool {
            if self.delete_selection() {
                return true;
            }
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

        /// The draft as the frame draws it: what is shown, chips included.
        ///
        /// It is [`text`] and not the buffer's characters for the frame: the
        /// frame draws the columns the person sees, and a chip is one of them.
        /// The cursor the frame is handed is an index into *this* string, which
        /// is why the two have to be the same string — handing the frame the
        /// expanded paste put a cursor on a character that is not on screen.
        ///
        /// [`text`]: LineBuffer::text
        fn display(&self) -> String {
            self.chars.iter().collect()
        }

        fn clear(&mut self) {
            self.chars.clear();
            self.chips.clear();
            self.anchor = None;
            self.cursor = 0;
        }

        fn replace(&mut self, text: &str) {
            self.chars = text.chars().collect();
            self.chips.clear();
            self.anchor = None;
            self.cursor = self.chars.len();
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

    /// How far one page key moves the conversation, in lines.
    ///
    /// The reader does not know the height of the terminal — the frame does,
    /// and it is the frame that clamps to what fits. A page is a screenful of an
    /// ordinary terminal, which is the number a person expects from the key
    /// whether their own screen is taller or shorter.
    const SCROLL_PAGE: isize = 24;

    /// How far one notch of the wheel moves the conversation, in lines.
    ///
    /// Three is what a wheel is on most terminals: one event per notch, and a
    /// conversation that moved a whole screen per notch would be impossible to
    /// read at the pace a hand turns it.
    const SCROLL_WHEEL: isize = 3;

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
                            // A panel that can be acted on owns the keys it
                            // needs: the arrows walk its rows and Enter acts
                            // on one. The decision is the panel's — it is the
                            // thing that knows which row is under the cursor —
                            // so what travels is the key and not a scroll
                            // offset, and the reader that owns the service is
                            // what carries the decision out.
                            //
                            // A report has no rows to act on, so its arrows
                            // scroll it, which is what a long list of text
                            // needs and all it needs.
                            if pinned.modal_is_editable() {
                                let key_for_panel = match key.code {
                                    KeyCode::Esc => Some(crate::modal::Key::Escape),
                                    KeyCode::Up => Some(crate::modal::Key::Up),
                                    KeyCode::Down => Some(crate::modal::Key::Down),
                                    KeyCode::PageUp => Some(crate::modal::Key::PageUp),
                                    KeyCode::PageDown => Some(crate::modal::Key::PageDown),
                                    KeyCode::Enter => Some(crate::modal::Key::Enter),
                                    KeyCode::Char('g')
                                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                    {
                                        Some(crate::modal::Key::ToggleScope)
                                    }
                                    // The keys a telemetry panel owns: it is the
                                    // thing that knows which tab, range and
                                    // metric are shown, so what travels is the
                                    // key and not the state.
                                    KeyCode::Tab => Some(crate::modal::Key::Tab),
                                    KeyCode::Char(']') => Some(crate::modal::Key::NextRange),
                                    KeyCode::Char('[') => Some(crate::modal::Key::PrevRange),
                                    KeyCode::Char('r') => Some(crate::modal::Key::NextMetric),
                                    KeyCode::Char('R') => Some(crate::modal::Key::PrevMetric),
                                    _ => None,
                                };
                                if let Some(press) = key_for_panel {
                                    if sender
                                        .send(EditorEvent::Action(EditorAction::Panel(press)))
                                        .is_err()
                                    {
                                        return;
                                    }
                                    continue;
                                }
                            }
                            // A report owns the keyboard while it is up: Escape
                            // closes it and the arrows walk it.
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
                            // The page keys belong to the conversation, and to
                            // nothing else: they are the only way to read what
                            // scrolled past, and the input has no use for them.
                            // A page moves by a screenful, which is what the
                            // key says; a line is what Ctrl does with it.
                            match key.code {
                                KeyCode::PageUp => {
                                    pinned.scroll_transcript(-SCROLL_PAGE);
                                    continue;
                                }
                                KeyCode::PageDown => {
                                    pinned.scroll_transcript(SCROLL_PAGE);
                                    continue;
                                }
                                KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    // Top of the conversation: as far back as
                                    // it goes, which the frame clamps.
                                    pinned.scroll_transcript(-10_000);
                                    continue;
                                }
                                KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    // And back to the tail, which is the one key
                                    // that resumes following it.
                                    pinned.follow_tail();
                                    continue;
                                }
                                _ => {}
                            }
                            // The completion menu owns the arrows while it is
                            // open. It has to be read before the arrows reach
                            // the draft: they are the keys that walk the list,
                            // and a list nobody can walk is a list nobody uses.
                            // Every other key belongs to the line, which is what
                            // rebuilds the menu and puts the choice back on the
                            // first row.
                            let rows = crate::interaction::completion_rows(
                                &buffer.display(),
                                buffer.cursor,
                            );
                            if rows > 1 {
                                match key.code {
                                    KeyCode::Up => {
                                        pinned.move_completion(-1, rows);
                                        continue;
                                    }
                                    KeyCode::Down => {
                                        pinned.move_completion(1, rows);
                                        continue;
                                    }
                                    _ => {}
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
                            if to_input {
                                // What Tab writes is the row the menu has under
                                // its cursor, resolved from the line as it is
                                // now: the key about to be read may be the one
                                // that changes it.
                                let picked = if rows > 1 {
                                    crate::interaction::completion_text(
                                        &buffer.display(),
                                        buffer.cursor,
                                        pinned.completion_pick(),
                                    )
                                } else {
                                    None
                                };
                                match handle_key(
                                    key,
                                    &mut buffer,
                                    &mut history,
                                    &mut history_pos,
                                    &mut draft,
                                    sender,
                                    picked.as_deref(),
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
                                }
                            }
                        }
                        Ok(Event::Paste(text)) => {
                            buffer.insert_paste(&text);
                            redraw = true;
                        }
                        // A resize is a change like any other, and the frame
                        // reads the terminal's size when it draws: what it needs
                        // is to be told to draw. Nothing marked this before, so
                        // the flag the frame's own resize handling reads was
                        // never set — the sidebar kept the columns it had been
                        // given at the old size and took the rest of the screen.
                        Ok(Event::Resize(..)) => {
                            pinned.resized.store(true, Ordering::Release);
                            pinned.dirty.store(true, Ordering::Relaxed);
                        }
                        // The wheel scrolls the conversation, which is the one
                        // thing under it that is taller than the screen. A
                        // panel that is up takes the wheel first: it is over
                        // the interface, so what a person is looking at is what
                        // they are scrolling.
                        Ok(Event::Mouse(mouse)) => {
                            use crossterm::event::{MouseButton, MouseEventKind};
                            match mouse.kind {
                                MouseEventKind::ScrollUp => {
                                    if !pinned.scroll_modal(-SCROLL_WHEEL) {
                                        pinned.scroll_transcript(-SCROLL_WHEEL);
                                    }
                                    redraw = true;
                                }
                                MouseEventKind::ScrollDown => {
                                    if !pinned.scroll_modal(SCROLL_WHEEL) {
                                        pinned.scroll_transcript(SCROLL_WHEEL);
                                    }
                                    redraw = true;
                                }
                                // Selecting with the mouse is the application's
                                // now: asking for the wheel turns off the
                                // terminal's own selection, and a terminal that
                                // does not select is one a person cannot copy
                                // out of. The drag is read here and the frame
                                // draws it; letting go copies it.
                                MouseEventKind::Down(MouseButton::Left) => {
                                    pinned.selecting.store(true, Ordering::Relaxed);
                                    pinned.begin_selection(mouse.column, mouse.row);
                                    redraw = true;
                                }
                                MouseEventKind::Drag(MouseButton::Left) => {
                                    if pinned.selecting.load(Ordering::Relaxed) {
                                        pinned.extend_selection(mouse.column, mouse.row);
                                        redraw = true;
                                    }
                                }
                                MouseEventKind::Up(MouseButton::Left) => {
                                    let was = pinned.selecting.swap(false, Ordering::Relaxed);
                                    if was {
                                        if let Some(text) = pinned.finish_selection() {
                                            let _ = sender.send(EditorEvent::Copied(text));
                                        }
                                        redraw = true;
                                    }
                                }
                                _ => {}
                            }
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
                        pinned.reset_completion_pick();
                        pinned.set_draft(
                            buffer.display(),
                            buffer.cursor,
                            buffer.selection_anchor(),
                        );
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

    #[allow(clippy::too_many_lines)] // one arm per key, which is the keymap.
    fn handle_key(
        key: KeyEvent,
        buffer: &mut LineBuffer,
        history: &mut Vec<String>,
        history_pos: &mut Option<usize>,
        draft: &mut String,
        sender: &mpsc::UnboundedSender<EditorEvent>,
        completion: Option<&str>,
    ) -> KeyOutcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            // A shifted motion extends a selection; the reader below is what
            // decides which keys are motions.
            KeyCode::Left if shift => {
                buffer.start_selection();
                buffer.left();
                KeyOutcome::Continue
            }
            KeyCode::Right if shift => {
                buffer.start_selection();
                buffer.right();
                KeyOutcome::Continue
            }
            KeyCode::Home if shift => {
                buffer.start_selection();
                buffer.home();
                KeyOutcome::Continue
            }
            KeyCode::End if shift => {
                buffer.start_selection();
                buffer.end();
                KeyOutcome::Continue
            }
            KeyCode::Up if shift => {
                buffer.start_selection();
                buffer.up();
                KeyOutcome::Continue
            }
            KeyCode::Down if shift => {
                buffer.start_selection();
                buffer.down();
                KeyOutcome::Continue
            }
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
            // Ctrl-A selects the whole draft, which is what it means in every
            // editor; a cursor to the start is a plain Home.
            KeyCode::Char('a') if control => {
                buffer.select_all();
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
                // Over a selection a plain arrow collapses to its edge rather
                // than moving by one, which is what makes a selection usable
                // with the arrows alone.
                if !buffer.collapse_selection(false) {
                    buffer.left();
                }
                KeyOutcome::Continue
            }
            KeyCode::Right => {
                if !buffer.collapse_selection(true) {
                    buffer.right();
                }
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
                // The draft has no line below, so the key walks the history.
                // That is what it has always done, and there is nothing under
                // the input for it to reach once the history is at its newest
                // entry: the row it used to focus is gone with the tabs.
                if history_pos.is_some() {
                    history_next(buffer, history, history_pos, draft);
                }
                KeyOutcome::Continue
            }
            KeyCode::Tab => {
                // What Tab writes is the row the menu has under its cursor, and
                // the first row when there is no menu: a completion that always
                // took the first candidate made the arrows pointless.
                match completion {
                    Some(completion) if !completion.is_empty() => {
                        buffer.insert_text(completion);
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

    /// A pasted path that names an image, as the attachment to carry.
    ///
    /// Only a path that exists and whose extension is one a model can look at
    /// makes an attachment: anything else is text, and pasting a path to a
    /// script is still pasting text.
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
        use super::*;

        /// A buffer holding `text`, with the cursor at its end unless moved.
        fn draft(text: &str) -> LineBuffer {
            let mut buffer = LineBuffer::default();
            for character in text.chars() {
                buffer.insert(character);
            }
            buffer
        }

        #[test]
        fn a_selection_replaces_what_is_typed_over_it() {
            // The point of a selection in the editor: it is a range the next
            // keystroke replaces, which is what makes typing over a word work.
            let mut buffer = draft("hello world");
            buffer.cursor = 0;
            buffer.start_selection();
            buffer.cursor = 5;
            assert_eq!(buffer.selection(), Some((0, 5)));
            buffer.insert('X');
            assert_eq!(buffer.display(), "X world");
            assert_eq!(buffer.selection(), None, "the selection left with the text");
        }

        #[test]
        fn backspace_and_delete_take_the_selection_whole() {
            // A deletion key over a selection removes the selection and not one
            // character beside it, which is what a person expects from it. The
            // range here is `two` inside `one two three` — its own spaces stay,
            // because they were not selected.
            let mut buffer = draft("one two three");
            buffer.cursor = 4;
            buffer.start_selection();
            buffer.cursor = 7;
            assert!(buffer.backspace());
            assert_eq!(buffer.display(), "one  three");

            let mut buffer = draft("one two three");
            buffer.cursor = 4;
            buffer.start_selection();
            buffer.cursor = 7;
            assert!(buffer.delete());
            assert_eq!(buffer.display(), "one  three");
        }

        #[test]
        fn select_all_takes_the_whole_draft_and_the_cursor_is_one_end() {
            let mut buffer = draft(
                "a
b
c",
            );
            buffer.select_all();
            assert_eq!(buffer.selection(), Some((0, 5)));
            assert_eq!(buffer.cursor, 5);
            buffer.delete_selection();
            assert!(
                buffer.is_empty(),
                "select-all then delete empties the draft"
            );
        }

        #[test]
        fn a_plain_arrow_collapses_a_selection_to_its_edge() {
            // An arrow over a selection moves to the edge rather than by one
            // character, so a selection can be narrowed with the arrows alone.
            let mut buffer = draft("abcdef");
            buffer.cursor = 1;
            buffer.start_selection();
            buffer.cursor = 4;
            assert!(buffer.collapse_selection(true), "right goes to the end");
            assert_eq!(buffer.cursor, 4);
            assert_eq!(buffer.selection(), None);

            buffer.cursor = 1;
            buffer.start_selection();
            buffer.cursor = 4;
            assert!(buffer.collapse_selection(false), "left goes to the start");
            assert_eq!(buffer.cursor, 1);
        }

        #[test]
        fn a_selection_over_a_chip_takes_the_chip_whole() {
            // Half a pasted paragraph is not a smaller paragraph, so a selection
            // that touches a chip removes the chip and not the part it covered.
            let mut buffer = LineBuffer::default();
            buffer.insert_paste(&"x".repeat(CHIP_MIN_CHARS + 8));
            assert_eq!(buffer.chips.len(), 1, "the paste became one chip");
            buffer.select_all();
            buffer.delete_selection();
            assert!(buffer.is_empty(), "the chip went with the selection");
            assert!(buffer.chips.is_empty(), "and its range went with it");
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
                    Some(EditorEvent::Action(_) | EditorEvent::Copied(_)) => continue,
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

    #[test]
    fn a_selection_is_read_back_as_the_text_that_was_on_screen() {
        // The frame is what knows where a row landed, so what a drag covers is
        // read from the rows it painted and not from the transcript behind
        // them: the same entry wraps differently at another width.
        let rows = vec![
            "  first line of the answer".to_owned(),
            "  second line".to_owned(),
            "  third".to_owned(),
        ];
        // A drag takes the rest of the row it started on — from the column it
        // started at — and everything before the column it ended on, in the row
        // it ended on. `  first line of the answer` is column 8 onwards: that is
        // `l` and a space, which is what a terminal copies too.
        assert_eq!(
            super::text_in(((8, 0), (8, 1)), &rows),
            "line of the answer\n  second"
        );
        // A drag that ends on a later row takes the rows between whole, which
        // is what makes a drag over a paragraph yield the paragraph.
        assert_eq!(
            super::text_in(((2, 0), (4, 2)), &rows),
            "first line of the answer\n  second line\n  thi"
        );
        // And the corners may come in any order: a drag goes up as easily as
        // down.
        assert_eq!(
            super::text_in(((8, 1), (8, 0)), &rows),
            "line of the answer\n  second"
        );
        // A click with no drag is one character, and a click past the end is
        // nothing to copy at all.
        assert_eq!(super::text_in(((2, 0), (2, 0)), &rows), "f");
        assert_eq!(super::text_in(((0, 9), (5, 9)), &rows), "");
    }

    #[test]
    fn moving_the_conversation_is_a_change_to_draw() {
        // The frame draws the offset it is handed, and the tick only draws when
        // something moved. The keys moved a number nobody marked as changed, so
        // the conversation stayed where it was: the scroll did nothing.
        let pinned = Pinned::new();
        pinned.take_dirty();
        // Page-up is a negative delta and it goes *back*: the offset counts
        // from the end, so going back is a bigger number. Subtracting instead
        // left the key doing nothing at the tail, where zero minus a page is
        // still zero.
        pinned.scroll_transcript(-10);
        assert_eq!(pinned.transcript_scroll(), 10, "up goes back in time");
        assert!(pinned.take_dirty(), "a scroll has to be drawn");
        pinned.scroll_transcript(10);
        assert_eq!(pinned.transcript_scroll(), 0, "down returns to the tail");
        // A key that cannot move anything is not a change: at the tail, moving
        // further towards it leaves the offset where it was.
        pinned.take_dirty();
        pinned.scroll_transcript(5);
        assert!(!pinned.take_dirty(), "nothing moved, nothing to draw");
        // And jumping back to the tail is a move when there was somewhere to
        // come back from.
        pinned.scroll_transcript(-500);
        pinned.take_dirty();
        pinned.follow_tail();
        assert!(pinned.take_dirty(), "following the tail is a move");
        assert_eq!(pinned.transcript_scroll(), 0);
    }

    #[test]
    fn the_pinned_footer_keeps_the_session_and_todos() {
        // The footer's state is read under one lock, and two of its fields are
        // the session line and the task list. The tabs that used to share this
        // handle are gone; what they carried is not part of it any more.
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
