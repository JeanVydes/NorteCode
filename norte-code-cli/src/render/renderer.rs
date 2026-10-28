//! The event renderer: streamed text, tool activity, usage and the footer.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::input::{Pinned, TodoItem};

/// The ink a row of the transcript is written in.
use crate::transcript::Ink;

use super::argument::{todo_item, todo_items};
use super::dial::{context_dial, reasoning_bar, token_ratio};
use super::edit::{
    EditBox, EditPreview, Side, UnifiedKind, clip_command, clip_plain, edit_box_width,
    edit_preview, edit_range, preview_path, render_panel_plain, unify,
};
use super::helpers::{cache_percent_label, human_tokens, short_path};
use super::labels::{
    budget_action_name, budget_label, code_name, format_measure, scope_name, short, unit_name,
};
use super::{
    BTreeMap, BudgetAction, BudgetScope, BudgetUnit, Delta, DiffStyle, HostFacts, Phase,
    ServerEvent, ServerEventKind, Style, clip_text, compact_json, duration_label,
};

/// How events are written.
#[derive(Clone, Copy)]
pub(crate) enum Format {
    /// Streamed text on stdout, diagnostics on stderr.
    Text,
    /// One JSON event per line on stdout.
    Jsonl,
}

/// How many characters of a line without a newline may wait before it is
/// written anyway.
///
/// A line that never ends is a progress bar or a prompt, and holding it for a
/// newline that never comes would show nothing at all while the tool runs.
const MAX_PARTIAL_LINE_CHARS: usize = 512;

/// The most rows one streamed output box keeps, whatever the screen.
///
/// A command that prints thousands of lines would push the transcript off the
/// screen; the box keeps its last rows and counts what it dropped. It is a bound
/// on one call and not on the conversation: the transcript is scrolled, so what
/// a longer command produced is not a reason to lose the work around it.
///
/// The real bound is the screen (see [`OutputLimit`]), and this is the ceiling
/// for a process with no terminal of its own to measure: a box never taller
/// than the screen it is drawn on, and never more than this.
const MAX_OUTPUT_ROWS: usize = 400;

/// How much of the conversation the renderer shows.
///
/// One axis, not two flags: `low` is what `--quiet` used to be, `high` what
/// `--verbose` used to be, and `medium` sits between them so "something is
/// happening" stays visible without the whole transcript.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Verbosity {
    /// The answer and nothing else: no reasoning, no tool activity, no usage.
    Low,
    /// Also says that a tool is running and that the model is thinking.
    #[default]
    Medium,
    /// Everything: the reasoning text, what each tool was asked and what it
    /// returned.
    High,
}

impl Verbosity {
    /// Whether the level reaches `level`.
    pub(crate) fn at_least(self, level: Self) -> bool {
        self >= level
    }

    /// Whether nothing but the answer is shown.
    pub(crate) fn is_quiet(self) -> bool {
        self == Self::Low
    }

    /// The level `--verbose` names.
    ///
    /// # Errors
    ///
    /// Returns a message naming the accepted words when `value` is none of
    /// them, so the CLI can fail before a turn starts.
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "low" => Ok(Self::Low),
            "medium" | "med" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            other => Err(format!(
                "unknown verbosity `{other}`; use low, medium or high"
            )),
        }
    }
}

/// Which streamed block the renderer is in, so reasoning never runs inline
/// with the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    /// Nothing is streaming.
    None,
    /// Model reasoning, shown in a dim gutter on stderr.
    Reasoning,
    /// The answer, printed plainly on stdout.
    Answer,
}

/// The budget that stopped a turn and may be raised interactively.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BudgetPrompt {
    pub(crate) scope: BudgetScope,
    pub(crate) unit: BudgetUnit,
    pub(crate) spent: f64,
    pub(crate) limit: f64,
}

/// Renders one server event to the terminal.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Renderer {
    pub(crate) format: Format,
    pub(crate) verbosity: Verbosity,
    /// The session key, for the footer. The server sends it on every event.
    pub(crate) session: Option<String>,
    /// Whether the footer names the directory the work is in.
    ///
    /// It is the whole line's first field, so it is shown where a person looks
    /// for it rather than only in the startup block; a footer with no room for
    /// it — a narrow terminal, a pipe — turns it off.
    pub(crate) show_workdir: bool,
    /// The session's title, when it has one.
    ///
    /// It is what the footer leads with: a key says nothing about what a
    /// session was for, and the title is what a person recognises it by. The
    /// key stays beside it, because the key is what `resume` is given.
    pub(crate) title: Option<String>,
    /// How an `edit` call's diff is drawn, from `ui.diff_style`.
    ///
    /// It lives here because the renderer draws it, and it is set once at
    /// startup: changing it mid-run would leave one transcript with two shapes
    /// in it and no way to tell which row was which.
    pub(crate) diff_style: DiffStyle,
    pub(crate) show_cost: bool,
    /// The facts only the host knows, printed by `/debug`.
    pub(crate) host_facts: Option<HostFacts>,
    /// The provider never reports cost, so a missing figure is the expected
    /// state and is not printed as if it were missing.
    pub(crate) cost_silent: bool,
    pub(crate) style: Style,
    /// The pinned footer text shared with the input editor.
    pub(crate) pinned: Arc<Pinned>,
    pub(crate) block: Block,
    pub(crate) reasoning_line_start: bool,
    /// Whether `--quiet` has already printed its one redacted reasoning marker
    /// for this turn.
    pub(crate) reasoning_placeholder: bool,
    pub(crate) last_char: Option<char>,
    /// Buffered answer text, rendered as markdown on `end_blocks`.
    pub(crate) answer_buffer: String,
    /// The terminal, when this process has one to draw on.
    ///
    /// The renderer is what knows when something changed — it is the thing
    /// events are handed to — so it is what draws the frame. A process with no
    /// terminal has none, and the plain-text path is what runs instead.
    pub(crate) screen: Option<crate::terminal::Guard>,
    /// The panel a command opened, over the interface.
    ///
    /// A report a person asked for (`/tools`, `/debug`) is a thing to read and
    /// dismiss, not a thing to add to the conversation: as lines of the
    /// transcript it would push the work away, and as prints it landed on top
    /// of the frame.
    pub(crate) modal: Option<crate::modal::Modal>,
    /// The conversation, kept as data.
    ///
    /// It is filled from the same events that are printed, so the two are the
    /// same conversation while the interface is moved onto it one part at a
    /// time. When the last part has moved, the printing goes and this stays.
    pub(crate) transcript: crate::transcript::Transcript,
    /// The path a `write` or `read` is working on, by call, until it finishes.
    pending_paths: BTreeMap<String, String>,
    pub(crate) model_name: Option<String>,
    /// The reasoning effort the session is actually using, as `/reasoning`
    /// names it. `None` until a turn has settled it, because before that the
    /// setting is whatever the next turn resolves.
    pub(crate) reasoning: Option<String>,
    /// What the client is waiting for, when a turn is in flight.
    ///
    /// It is a value and not a thread: the bar used to repaint a line of its
    /// own from a thread of its own, which is a second writer on a terminal
    /// that has one. The state says what is happening and the frame draws it.
    pub(crate) waiting: Option<super::loading::Wait>,
    /// When the current wait began, as milliseconds since the epoch.
    ///
    /// It is set when the client is told a turn started and cleared when the
    /// wait ends, which is what the row beside the spinner counts: a number
    /// that outlived the wait would keep counting after the answer arrived.
    pub(crate) waiting_since: Option<u128>,
    /// The rows the plugins asked for, read once at startup.
    ///
    /// Kept rather than asked for per row: the plugins are a value the host
    /// built, and a frame that reached for them would be a frame that could
    /// stall on somebody else's code.
    pub(crate) sidebar: Vec<(String, Vec<norte_code_plugins::SidebarRow>)>,
    pub(crate) failed: bool,
    pub(crate) cancelled: bool,
    pub(crate) budget_stopped: bool,
    /// A pending `ask` budget stop. The CLI prompts once per stop.
    pub(crate) budget_prompt: Option<BudgetPrompt>,
    /// Why the host ended the turn on purpose, when it did.
    pub(crate) stopped: Option<String>,
    /// What the turn is doing right now, shown before the session totals. It is
    /// `None` between turns, so an idle footer is never mistaken for a hung one.
    pub(crate) activity: Option<String>,
    pub(crate) requests: u64,
    /// Requests settled during the current turn, used to explain a cancel
    /// before the provider reported any usage.
    pub(crate) turn_requests: u64,
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
    pub(crate) cache_reported: bool,
    pub(crate) cost: f64,
    pub(crate) unpriced: u64,
    pub(crate) tool_counts: BTreeMap<String, u64>,
    /// The current context size, as the last request reported its input tokens.
    /// It is the number that grows toward the compaction limit, unlike the
    /// cumulative `input` total.
    pub(crate) context_used: u64,
    /// The model's context window, when the catalogue knows it.
    pub(crate) max_context: Option<u64>,
    /// The used-context fraction that triggers compaction, in (0, 1].
    pub(crate) compaction_threshold: f64,
    /// A `todo` call whose arguments arrived but whose result has not, keyed by
    /// call id. The footer changes only after the tool succeeds.
    pending_todos: BTreeMap<String, Vec<TodoItem>>,

    /// The calls whose output was already streamed to the terminal, by call id.
    ///
    /// Their finish must not print the same text again: a shell that streamed
    /// its stdout line by line would otherwise show it twice, once as it ran and
    /// once in the result block right after.
    streamed_calls: BTreeSet<String>,
    /// The calls whose finish has already been drawn. A tool that outlives its
    /// call — a detached shell — can send one more chunk after the box closed,
    /// and opening a second box for it reads as the same call having two.
    closed_calls: BTreeSet<String>,

    /// The call whose streamed output block is open, if any.
    ///
    /// A tool's output arrives in chunks, so the box is opened on the first one
    /// and stays open across them, like the reasoning gutter does for a thought
    /// that arrives in pieces. It is per call, not per stream: a command's
    /// stdout and stderr are read by two tasks at once and arrive interleaved,
    /// which is what a terminal shows.
    output_gutter: Option<String>,

    /// Whether the open output block has written a row yet.
    output_line_start: bool,

    /// How many rows the open output box has printed.
    ///
    /// Past [`MAX_OUTPUT_ROWS`] a row is not kept, so a command that prints
    /// thousands of lines cannot push the transcript off the screen. The count
    /// is what says how many were left out.
    output_rows_seen: usize,

    /// The rows the open output box holds, in the order they arrived.
    ///
    /// They are collected here and become one entry when the call closes: a
    /// streamed command used to add an entry per line, which is what bloated the
    /// transcript and what drew a box whose edges landed in the middle of the
    /// work.
    output_rows: Vec<String>,

    /// What has arrived from each stream without a newline yet, by stream.
    ///
    /// A chunk ends wherever a pipe read ended, which is usually mid-line: the
    /// reader hands over `stdout[01] p` and `arcial... completa` as two chunks
    /// of one row. A row is written only once its line is whole, so the two are
    /// joined instead of landing on rows of their own.
    output_pending: BTreeMap<String, String>,
}

/// How a finished call is announced: a mark, the tool, and how long it took.
///
/// Three endings, not two. A detached call is an error the model is right to
/// see, and it is not a failure: it was asked to stop waiting and the process is
/// still running as a job, so drawing it as `✗ failed` reads as something having
/// gone wrong at the exact moment something went right.
pub(crate) fn call_ending(
    style: Style,
    tool: &str,
    is_error: bool,
    detached: bool,
    duration_ms: u64,
) -> String {
    let (mark, word) = if detached {
        (style.yellow("◌"), format!("let go of in {duration_ms} ms"))
    } else if is_error {
        (style.red("✗"), format!("failed in {duration_ms} ms"))
    } else {
        (style.green("✓"), format!("in {duration_ms} ms"))
    };
    format!("{mark} {} {}", style.bold(tool), style.dim(word))
}

impl Renderer {
    /// A renderer with empty counters for a new run.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    pub(crate) fn new(
        format: Format,
        verbosity: Verbosity,
        show_cost: bool,
        style: Style,
        pinned: Arc<Pinned>,
    ) -> Self {
        Self {
            format,
            verbosity,
            session: None,
            show_workdir: true,
            title: None,
            diff_style: DiffStyle::default(),
            host_facts: None,
            show_cost,
            cost_silent: false,
            style,
            pinned,
            block: Block::None,
            reasoning_line_start: false,
            reasoning_placeholder: false,
            last_char: None,
            answer_buffer: String::new(),
            transcript: crate::transcript::Transcript::new(),
            modal: None,
            screen: None,
            pending_paths: BTreeMap::new(),
            model_name: None,
            reasoning: None,
            waiting: None,
            waiting_since: None,
            sidebar: Vec::new(),
            failed: false,
            cancelled: false,
            budget_stopped: false,
            budget_prompt: None,
            stopped: None,
            activity: None,
            requests: 0,
            turn_requests: 0,
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_reported: false,
            cost: 0.0,
            unpriced: 0,
            tool_counts: BTreeMap::new(),
            context_used: 0,
            max_context: None,
            compaction_threshold: 1.0,
            pending_todos: BTreeMap::new(),
            streamed_calls: BTreeSet::new(),
            closed_calls: BTreeSet::new(),
            output_gutter: None,
            output_line_start: true,
            output_pending: BTreeMap::new(),
            output_rows_seen: 0,
            output_rows: Vec::new(),
        }
    }

    /// Keeps one event as a transcript entry.
    ///
    /// It is the same conversation the printing below produces, recorded rather
    /// than drawn. Only the events that are something a person reads become
    /// entries: a phase change or a counter is state the chrome shows, not a
    /// thing that was said, and putting it in the conversation would fill the
    /// scrollback with bookkeeping.
    fn record(&mut self, kind: &ServerEventKind) {
        use crate::transcript::Kind as Entry;
        match kind {
            ServerEventKind::Delta { delta, .. } => {
                // The event already says which of the two it is, so the entry
                // does not have to ask the renderer what block it is in: the
                // wire is the authority and the block is a consequence.
                let (entry, text) = match delta {
                    // A refusal is what the model said instead of an answer,
                    // and it reads as one: the two are the same entry.
                    Delta::Text { text, .. } | Delta::Refusal { text, .. } => (Entry::Answer, text),
                    // The thought is kept only where it is asked for. It is the
                    // longest thing a model produces and the least of it is
                    // worth reading, so below `high` the spinner says the model
                    // is working and the thought itself stays out of the way.
                    Delta::Reasoning { .. } if !self.verbosity.at_least(Verbosity::High) => return,
                    Delta::Reasoning { text, .. } => (Entry::Reasoning, text),
                    _ => return,
                };
                if !text.is_empty() {
                    self.transcript.append(entry, text);
                }
            }
            ServerEventKind::ToolStarted {
                tool,
                call_id,
                arguments,
                ..
            } => {
                self.transcript.close();
                // One row per call, written here in the present tense: what the
                // call is doing, with the clock running because it has not said
                // it is done. The ending rewrites this same row, so a call is
                // one line and not two — see `tool_event`.
                // `low` is the answer alone: a tool is not part of it, and a
                // row here would be a row with no ending to rewrite it — the
                // ending is what says how it went, and `low` does not show
                // that either.
                if self.verbosity.is_quiet() {
                    return;
                }
                // The row says what the call is doing and nothing else: the id
                // it used to carry was there to pair the announcement with the
                // ending, and the ending edits this same row now, so there is
                // nothing left to pair.
                let mut entry = crate::transcript::Entry::line(
                    Entry::ToolStarted,
                    Self::doing(tool, arguments.as_ref()),
                );
                entry.started = Some(std::time::Instant::now());
                entry.call_id = Some(call_id.clone());
                self.transcript.push(entry);
            }
            ServerEventKind::ToolOutput { text, .. } => {
                self.transcript.append(Entry::ToolOutput, text);
            }
            ServerEventKind::ToolFinished {
                tool,
                call_id,
                is_error,
                ..
            } => {
                // The call is over, so its clock stops: what the row shows from
                // here is the duration the service reported, and a count that
                // kept running beside a finished call would be a lie.
                self.transcript.stop_timer();
                self.transcript.close();
                // The row the call announced itself on becomes the row that
                // says how it ended. It is one call and one line: the two used
                // to be separate entries, so a suite of ten calls put twenty
                // lines in the conversation and the reader had to pair them up
                // by the id beside them.
                //
                // There is no ending text here — the duration the service
                // reported is joined to the line in `tool_event`, which is
                // where the rest of the ending is known.
                if *is_error {
                    if let Some(entry) = self.transcript.call_mut(call_id) {
                        entry.kind = Entry::Failure;
                    } else {
                        self.transcript
                            .push(crate::transcript::Entry::line(Entry::Failure, tool.clone()));
                    }
                }
            }
            ServerEventKind::MessageReplayed { role, text, .. } => {
                self.transcript.close();
                // The replay says who spoke, and it is not always the human: a
                // resumed session replays the tool results and the model's own
                // answers too.
                let kind = if role == "user" {
                    Entry::Said
                } else {
                    Entry::Answer
                };
                let mut entry = crate::transcript::Entry::new(kind);
                entry.push_text(text);
                self.transcript.push(entry);
            }
            ServerEventKind::Error { message, .. } => {
                self.transcript.close();
                self.transcript.push(crate::transcript::Entry::line(
                    Entry::Failure,
                    message.clone(),
                ));
            }
            ServerEventKind::CompactionStarted { .. } => {
                self.transcript.close();
                self.transcript
                    .push(crate::transcript::Entry::line(Entry::Notice, "compacting"));
            }
            ServerEventKind::RetryScheduled { .. } => {
                self.transcript.close();
                self.transcript
                    .push(crate::transcript::Entry::line(Entry::Notice, "retrying"));
            }
            _ => {}
        }
    }

    /// Marks the top of a session in the conversation.
    ///
    /// The transcript is scrolled and the first line is what says where a
    /// session begins: without it a resumed conversation starts mid-thought,
    /// with no sign that anything above it was a different session.
    pub(crate) fn greet(&mut self) {
        if matches!(self.format, Format::Jsonl) {
            return;
        }
        self.transcript.push(crate::transcript::Entry::line(
            crate::transcript::Kind::Notice,
            crate::art::MARK,
        ));
        self.transcript.close();
    }

    pub(crate) fn event(&mut self, event: &ServerEvent) -> Result<(), String> {
        self.session = event.session.as_ref().map(ToString::to_string);
        if self.pinned.take_resized() {
            // The input thread moved the main cursor to a fresh line after the
            // resize. Forget the column we thought we were on so the next
            // delta starts there instead of trying to continue a line that the
            // terminal may have reflowed.
            self.last_char = Some('\n');
            self.reasoning_line_start = true;
        }
        if matches!(self.format, Format::Jsonl) {
            let line = serde_json::to_string(event).map_err(|error| error.to_string())?;
            println!("{line}");
            return Ok(());
        }
        // A delta changes the block before it is recorded. The block change is
        // where the blank row between two blocks is written, and if the entry
        // for the new text already exists that row lands inside the answer it
        // was meant to open — which is what split an answer with a gap after
        // its first word every time the model went back to thinking.
        if let ServerEventKind::Delta { delta, .. } = &event.kind {
            self.delta(delta);
        }
        self.record(&event.kind);
        // The retry delay is over as soon as the next piece of work arrives.
        // Without this the footer would keep counting a wait that already ended.
        if matches!(
            &event.kind,
            ServerEventKind::Delta { .. }
                | ServerEventKind::ToolStarted { .. }
                | ServerEventKind::ToolFinished { .. }
                | ServerEventKind::RequestReconstructed { .. }
        ) && self
            .activity
            .as_deref()
            .is_some_and(|activity| activity.starts_with("retrying"))
        {
            self.mark_working();
        }
        match &event.kind {
            // Already reached the conversation, before the entry was made: see
            // the comment above.
            ServerEventKind::Delta { .. } => {}
            ServerEventKind::ToolStarted { .. }
            | ServerEventKind::ToolFinished { .. }
            | ServerEventKind::ToolOutput { .. } => {
                self.tool_event(&event.kind);
            }
            ServerEventKind::RequestReconstructed { .. }
            | ServerEventKind::SessionStats { .. }
            | ServerEventKind::CostNotReported { .. } => {
                self.usage_event(&event.kind);
            }
            ServerEventKind::MessageReplayed { index, role, text } => {
                self.replay_message(*index, role, text);
            }
            // The list the session already had, sent when a client attaches: the
            // transcript shows messages, and the task list is a widget the turn's
            // own `todo` events draw. Without this the widget started empty and a
            // resumed session looked like it had no tasks in flight.
            ServerEventKind::SessionTodos { todos } => {
                self.pinned.set_todos(todos.iter().map(todo_item).collect());
            }
            // The session was named, by `/rename` here or by another client
            // that shares the session. The footer is where the name shows, so
            // it repaints from the event rather than from whoever asked: the
            // two paths that set a title have to agree, and this is the one
            // that holds for both.
            ServerEventKind::SessionRenamed { title } => {
                self.set_title(title.clone());
            }
            kind => self.status_event(kind),
        }
        Ok(())
    }

    /// Opens a panel over the interface.
    ///
    /// The lines are kept plain: the panel is drawn by the frame, and a report
    /// that carried escape codes into it would show them as text.
    pub(crate) fn open_modal(&mut self, title: impl Into<String>, lines: Vec<String>) {
        let lines: Vec<String> = lines
            .into_iter()
            .map(|line| crate::select::strip_style(&line))
            .collect();
        if lines.is_empty() {
            return;
        }
        self.set_modal(crate::modal::Modal::report(title, lines));
    }

    /// Opens the telemetry panel.
    ///
    /// It is its own entry point and not `open_editable`: the panel draws charts
    /// from its own state and walks its own tabs, so it is not a list of rows
    /// with a cursor over them.
    pub(crate) fn open_telemetry(&mut self, panel: crate::telemetry_panel::Panel) {
        let mut modal = crate::modal::Modal::telemetry(panel);
        modal.refresh_telemetry();
        self.set_modal(modal);
    }

    /// Opens a panel that can be acted on.
    ///
    /// A report and an editor are one widget: what makes the difference is
    /// whether its rows carry an action, and that is what the cursor walks.
    pub(crate) fn open_editable(&mut self, title: impl Into<String>, rows: Vec<crate::modal::Row>) {
        if rows.is_empty() {
            return;
        }
        self.set_modal(crate::modal::Modal::editable(title, rows));
    }

    /// Puts a panel on screen and tells the keyboard it is there.
    fn set_modal(&mut self, modal: crate::modal::Modal) {
        // The keyboard reads this on its own thread: it is the one thing that
        // decides which keys belong to the panel and which to the report's
        // scroll, and the panel itself is not shared across the two threads.
        // A telemetry panel is not `editable` — it has no actable rows — but it
        // still owns its own keys, so the reader has to route them to it.
        self.pinned
            .set_modal_editable(modal.editable || modal.telemetry.is_some());
        self.modal = Some(modal);
        // The scroll a person left on the last report is not one they want on
        // this one: a new panel opens at its top.
        self.pinned.reset_modal_scroll();
        self.pinned.set_modal_open(true);
    }

    /// How many rows the panel has room for, which bounds its scroll.
    ///
    /// It is the size the panel is drawn at and not the terminal's: the panel
    /// takes most of the screen and its border and title take a row or two, and
    /// a scroll clamped to the terminal would let the list scroll past what a
    /// person can see.
    fn screen_rows(&self) -> usize {
        let Some(screen) = self.screen.as_ref() else {
            return 20;
        };
        // The panel is four fifths of the height and two rows are its border.
        let (_, height) = screen.size();
        usize::from(height).saturating_mul(4) / 5
    }

    /// How many rows an output box may keep, bounded by the screen.
    ///
    /// A box taller than the screen shows its last rows and scrolls the rest of
    /// the conversation off the top, which is the complaint the transcript
    /// scroll already answers for the conversation: an output box should not be
    /// a second, taller thing to scroll. The last screenful is kept, so what is
    /// on screen is the end of the output, and the count of what was dropped is
    /// what says there was more. A process with no terminal falls back to the
    /// ceiling.
    fn output_limit(&self) -> usize {
        // No terminal means no screen to bound to, and the plain-text path
        // prints the whole result elsewhere: the ceiling is the only bound.
        if self.screen.is_none() {
            return MAX_OUTPUT_ROWS;
        }
        let screen = self.screen_rows().max(1);
        // Two rows are the box's own edges and the last one is the prompt, so a
        // box that filled the screen would hide the line a person types on.
        screen.saturating_sub(3).clamp(10, MAX_OUTPUT_ROWS)
    }

    /// What the panel decided about a key, and what to do about it.
    ///
    /// The panel produces the intent — allow a tool, switch a server off — and
    /// this is where it is carried out, because the service is here and the
    /// panel has no idea one exists.
    pub(crate) fn modal_key(
        &mut self,
        key: crate::modal::Key,
    ) -> Option<(crate::modal::Action, crate::modal::Scope)> {
        let modal = self.modal.as_mut()?;
        match modal.key(key) {
            crate::modal::Choice::None => None,
            crate::modal::Choice::Closed => {
                self.close_modal();
                None
            }
            crate::modal::Choice::Act { action, scope } => Some((action, scope)),
        }
    }

    /// Closes the panel, if one is open. `true` when it closed one.
    pub(crate) fn close_modal(&mut self) -> bool {
        let closed = self.modal.take().is_some();
        if closed {
            self.pinned.set_modal_editable(false);
            self.pinned.set_modal_open(false);
        }
        closed
    }

    /// Adds one line of news to the conversation.
    ///
    /// The text is plain: the ink comes from the kind of entry, so a line does
    /// not carry escape codes into the data, where they would be text on the
    /// next frame and not a colour.
    pub(crate) fn notice(&mut self, text: impl std::fmt::Display) {
        self.transcript.push(crate::transcript::Entry::line(
            crate::transcript::Kind::Notice,
            text.to_string(),
        ));
    }

    /// Adds one line about something that went wrong.
    pub(crate) fn failure(&mut self, text: impl std::fmt::Display) {
        self.transcript.push(crate::transcript::Entry::line(
            crate::transcript::Kind::Failure,
            text.to_string(),
        ));
    }

    /// Draws one frame, when there is a terminal to draw on.
    ///
    /// Called after every event and every keystroke, and never on a timer: the
    /// frame is a function of the state, and the state only changes when one of
    /// those arrives. ratatui diffs against the last frame, so a call with
    /// nothing changed writes nothing.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal is gone, which means the process is
    /// ending anyway.
    /// Whether the frame is animating, so a clock has to keep drawing it.
    ///
    /// A spinner turns and a running call counts. Everything else — the
    /// conversation, the input, the status — only changes when a key or an
    /// event does, and drawing it on a timer is a frame written to produce the
    /// same screen.
    #[must_use]
    pub(crate) fn animating(&self) -> bool {
        self.waiting.is_some()
            || self.transcript.running_for().is_some()
            || self
                .modal
                .as_ref()
                .is_some_and(crate::modal::Modal::animating)
    }

    /// Advances an open panel's animation one frame.
    ///
    /// The clock that keeps an animation moving is the loop's, and this is what
    /// the loop calls on each tick: the panel counts its own frames and the
    /// renderer marks its own state dirty, so the next draw shows the new frame
    /// and stops once the animation has run.
    pub(crate) fn tick_modal(&mut self) {
        if let Some(modal) = self.modal.as_mut() {
            modal.tick();
        }
    }

    /// Takes the "something changed" flag the keyboard's thread set.
    ///
    /// The two threads speak through the pinned handle: the reader marks it
    /// when a keystroke changes the draft, and the frame takes it, so a tick
    /// with nothing to draw writes nothing.
    pub(crate) fn take_dirty(&self) -> bool {
        self.pinned.take_dirty()
    }

    pub(crate) fn draw(&mut self) -> Result<(), String> {
        if self.screen.is_none() {
            return Ok(());
        }
        // Escape is read on the keyboard's thread and the panel is drawn on
        // this one, so the request to close it arrives here: the frame is the
        // one place that knows a panel is up and the one place that can take it
        // down.
        if self.pinned.take_modal_closed() {
            self.close_modal();
        }
        // The scroll comes from the keyboard's thread as a number of lines:
        // what it means in rows is the frame's business, because the frame is
        // what knows how many of them fit.
        if let Some(modal) = self.modal.as_mut() {
            modal.top = self.pinned.modal_scroll();
        }
        let mut state = self.ui_state();
        let pinned = std::sync::Arc::clone(&self.pinned);
        // The panel's cursor moves with the scroll the keyboard asked for, and
        // the frame is what knows how many rows fit.
        let rows = self.screen_rows();
        if let Some(modal) = self.modal.as_mut() {
            // The keyboard sends the offset it wants, and the frame moves the
            // panel's top to it. The difference is what a scroll is, in either
            // direction, and the panel clamps it to what it holds.
            let offset = self.pinned.modal_scroll();
            if offset != modal.top {
                let delta = isize::try_from(offset)
                    .unwrap_or(isize::MAX)
                    .saturating_sub(isize::try_from(modal.top).unwrap_or(isize::MAX));
                modal.scroll_by(delta, rows);
            }
        }
        // The state and the panel go into one frame: `State::draw` paints the
        // panel over the interface in the same flush, so there is no screen
        // between them and nothing to flicker.
        if let Some(screen) = self.screen.as_mut() {
            state.draw(screen, self.style, &pinned, self.modal.as_ref())?;
        }
        Ok(())
    }

    /// The state the interface draws, built from what the renderer holds.
    ///
    /// It is the one place the two meet: everything above is what the client
    /// knows, everything below is what the screen shows, and this is the
    /// translation between them.
    fn ui_state(&self) -> crate::app::State {
        let (input, cursor, anchor) = self.pinned.draft();
        let completion = self.completion_for(&input, cursor);
        // The hint speaks only when no menu is up: an open menu is already
        // saying what may be typed, and a row repeating the first candidate
        // under it would say the same thing twice.
        let suggestion = if completion.is_some() {
            String::new()
        } else {
            Self::suggestion_for(&input, cursor)
        };
        crate::app::State {
            transcript: self.transcript.clone(),
            // Asked for on every frame rather than kept: a plugin that shows a
            // number wants it to move, and one that wants it still returns the
            // same text, which the frame's diff then writes nothing for.
            sidebar: self.sidebar.clone(),
            input,
            cursor,
            select: anchor,
            completion,
            // The question on screen, when one is: the frame draws it where the
            // draft would be, and the editor owns it, so this is the one bridge
            // between the two. Plain text, like everything else the widget
            // draws, with the ink the widget's to choose.
            question: self.pinned.question().map(|asked| crate::app::Question {
                prompt: crate::select::strip_style(&asked.prompt),
                choices: asked
                    .choices
                    .iter()
                    .map(|choice| crate::select::strip_style(choice))
                    .collect(),
                cursor: asked.cursor,
                multiple: asked.multiple,
                taken: asked.taken.clone(),
                typing: asked.typing,
            }),
            // The status is built for a terminal that took escape codes, and
            // the frame draws with spans: what reaches it is the text, and the
            // ink is the widget's.
            status: crate::select::strip_style(&self.session_status()),
            mcp: self.pinned.mcp(),
            todos: self.task_rows(),
            suggestion,
            waiting: self.waiting,
            waiting_since: self.waiting_since,
            selection: self.pinned.selection(),
        }
    }

    /// The completion the line offers, as the menu the frame draws.
    ///
    /// It is built here and not in the widget because the registry is not the
    /// widget's to read: a widget that asked would need the command registry,
    /// and the frame would stop being a function of the state it was given.
    fn completion_for(&self, text: &str, cursor: usize) -> Option<crate::app::Completion> {
        let rows = crate::commands_registry::shared().candidates(text, cursor);
        // One candidate is not a menu: the hint says what Tab would write, and
        // opening a list for a single row would take the arrow keys away from
        // the draft for a choice nobody has to make.
        if rows.len() < 2 {
            return None;
        }
        Some(crate::app::Completion {
            text: rows[0].completion.clone(),
            rows: rows
                .iter()
                .map(|row| {
                    if row.note.is_empty() {
                        row.text.clone()
                    } else {
                        format!("{} — {}", row.text, row.note)
                    }
                })
                .collect(),
            cursor: self.pinned.completion_pick().min(rows.len() - 1),
        })
    }

    /// The row above the input that says what may be typed next.
    ///
    /// Plain text: the ink is the widget's, and a hint carrying escape codes
    /// would show them as characters.
    fn suggestion_for(text: &str, cursor: usize) -> String {
        crate::interaction::command_suggestion(text, cursor)
            .map_or_else(String::new, |suggestion| {
                crate::select::strip_style(&suggestion.hint)
            })
    }

    /// Records the connected MCP servers, so the sidebar can draw them.
    ///
    /// The renderer does not hold the service and cannot ask it: the CLI calls
    /// this when it opens a session and after every `/refresh`, which is exactly
    /// when the set of connected servers can change. What the sidebar draws is
    /// then a snapshot taken at those moments, which is what a column of state is.
    pub(crate) fn set_mcp(&mut self, servers: Vec<crate::input::McpRow>) {
        self.pinned.set_mcp(servers);
    }

    /// The task list, as the rows the sidebar shows.
    ///
    /// The list used to be a tab in the footer, and a footer row is one line:
    /// what the model planned and what it has finished is a list, and a list
    /// does not fit in a row that also carries the model, the cost and the
    /// effort. It is a column of the sidebar now, where it has the room and
    /// where a person can see the work in flight beside the work.
    ///
    /// The rows come from the same state the footer's row used to be built
    /// from, so the two can never disagree about what is pending.
    fn task_rows(&self) -> Vec<crate::app::TaskRow> {
        self.pinned
            .todos()
            .into_iter()
            .map(|todo| crate::app::TaskRow {
                text: todo.content,
                done: matches!(todo.status, crate::input::TodoStatus::Completed),
            })
            .collect()
    }

    /// Takes the pending `ask` budget stop, if any, so the CLI prompts once.
    pub(crate) fn take_budget_prompt(&mut self) -> Option<BudgetPrompt> {
        self.budget_prompt.take()
    }

    /// Whether this renderer writes human output and can therefore prompt.
    pub(crate) fn can_prompt(&self) -> bool {
        matches!(self.format, Format::Text)
    }

    /// Notes that a cancellation was requested, so the turn's end reads as a
    /// cancellation rather than a failure.
    pub(crate) fn mark_cancelled(&mut self) {
        self.cancelled = true;
        self.set_activity(Some("cancelling…".to_owned()));
    }

    /// Hides the missing-cost note for a provider that never reports one.
    pub(crate) fn set_cost_silent(&mut self, silent: bool) {
        self.cost_silent = silent;
    }

    /// Shows that a turn is in flight, before the first server event arrives.
    /// Prints a "working…" line below the user's message instead of in the
    /// pinned footer. Cleared on the first answer token.
    pub(crate) fn mark_working(&mut self) {
        if !self.verbosity.at_least(Verbosity::Medium) || matches!(self.format, Format::Jsonl) {
            return;
        }
        self.failed = false;
        self.cancelled = false;
        self.set_wait(super::loading::Wait::Waiting);
        self.set_activity(Some("waiting for the first token".to_owned()));
    }

    /// Moves the spinner to a phase, keeping the clock it counts on.
    ///
    /// The instant the wait began belongs to the turn, so it is recorded once
    /// and a phase change inside the same turn keeps counting from there: what
    /// the number answers is how long this turn has been out, and restarting it
    /// at the first token would hide the wait that came before.
    ///
    /// It is set here and not at the two places that raised the phase. Those
    /// set `waiting` alone, and a phase raised after `stop_bar` had cleared the
    /// clock — which is what a tool's output does, and a tool runs in the
    /// middle of most turns — left `waiting_since` at `None`. The frame read a
    /// missing instant as zero, so the row said `streaming · 0s…` and stayed
    /// there for the rest of the answer.
    fn set_wait(&mut self, phase: super::loading::Wait) {
        self.waiting = Some(phase);
        if self.waiting_since.is_none() {
            self.waiting_since = Some(now_millis());
        }
    }

    /// Stops the animated bar, if one is running.
    ///
    /// The bar repaints its own line in place, so *every* writer on stderr has
    /// to stop it first: a line printed while it runs lands at the cursor, on
    /// the bar's line, and from then on the bar repaints a line of its own below
    /// the text it was dragging. That is what left `↳ river/…` glued to
    /// `waiting for the first token`, and what made a second bar appear.
    ///
    /// It is idempotent, so a writer that may already have stopped it — the
    /// answer's first token, a tool's output — can call it without checking.
    /// Marks the wait as over.
    fn stop_bar(&mut self) {
        self.waiting = None;
        self.waiting_since = None;
    }

    /// Records the reasoning effort the footer shows, and repaints it.
    ///
    /// The footer is the only place it appears, so the repaint is part of the
    /// update rather than something the caller has to remember.
    pub(crate) fn set_reasoning(&mut self, label: String) {
        if self.reasoning.as_deref() != Some(label.as_str()) {
            self.reasoning = Some(label);
            self.refresh_footer();
        }
    }

    /// Records the session's title and repaints the footer.
    ///
    /// The title arrives from a request that runs beside the first turn, so it
    /// lands at no particular moment in the conversation; the footer is where it
    /// shows, and the repaint is part of the update rather than something the
    /// caller has to remember.
    /// Sets how an `edit` call's diff is drawn, from `ui.diff_style`.
    ///
    /// A setter rather than another argument to [`Renderer::new`], which already
    /// takes the shape of the run: this is a display preference with a working
    /// default, and a caller that does not care should not have to name it.
    pub(crate) fn set_diff_style(&mut self, style: DiffStyle) {
        self.diff_style = style;
    }

    pub(crate) fn set_title(&mut self, title: String) {
        if self.title.as_deref() != Some(title.as_str()) {
            self.title = Some(title);
            self.refresh_footer();
        }
    }

    /// Repaints the pinned footer from the state the renderer holds.
    fn refresh_footer(&mut self) {
        if !self.verbosity.at_least(Verbosity::Medium) || matches!(self.format, Format::Jsonl) {
            return;
        }
        let status = self.session_status();
        self.pinned.set_session(status);
    }

    /// Replaces the activity label and redraws the pinned footer.
    fn set_activity(&mut self, activity: Option<String>) {
        if !self.verbosity.at_least(Verbosity::Medium) || matches!(self.format, Format::Jsonl) {
            return;
        }
        self.activity = activity;
        let status = self.session_status();
        self.pinned.set_session(status);
    }

    /// Ends the current output line before a prompt or the final summary.
    pub(crate) fn ensure_newline(&mut self) {
        if matches!(self.format, Format::Text) && self.last_char.is_some_and(|last| last != '\n') {
            println!();
            self.last_char = Some('\n');
        }
    }

    /// Closes a reasoning gutter, if one is open.
    ///
    /// The thought is one bounded thing, so the view frames it and the frame is
    /// chrome: the closing rule is not a row of the conversation. It used to be
    /// pushed as an entry of its own — a bare `╰` with nothing around it — which
    /// is what left a stray bracket under every thought and made a run of
    /// reasoning read as a box that had come apart.
    pub(crate) fn end_reasoning(&mut self) {
        if self.block != Block::Reasoning {
            return;
        }
        // The entry the thought was appended to is the one that gets the frame.
        // `record` opened it without one, because whether a thought is a box is
        // not known until it ends.
        if let Some(last) = self.transcript.last_mut()
            && last.kind == crate::transcript::Kind::Reasoning
        {
            last.boxed = true;
            if last.title.is_none() {
                last.title = Some("thinking".to_owned());
            }
        }
        self.block = Block::None;
        self.last_char = Some('\n');
    }

    /// Finishes any streamed block and leaves one blank line.
    pub(crate) fn end_blocks(&mut self) {
        self.reasoning_placeholder = false;
        self.stop_bar();
        // A streamed output block is closed here too: whatever ends the turn or
        // takes the next line has to leave the block finished, or its closing
        // bracket lands after unrelated text.
        self.end_output_gutter();
        self.end_reasoning();
        if self.block == Block::Answer {
            // The answer is already in the conversation, token by token, as the
            // model wrote it. What the markdown does with it is a way of drawing
            // it and not a second answer: rendering it here as well would put
            // every answer in twice, once plain and once formatted.
            self.answer_buffer.clear();
        }
        if self.block != Block::None {
            self.block = Block::None;
            self.last_char = Some('\n');
        }
    }

    /// Prints a single blank separator line.
    pub(crate) fn space(&mut self) {
        if !self.verbosity.at_least(Verbosity::Medium) {
            return;
        }
        // A blank row in the conversation, which is what separates two things
        // that are not part of each other.
        self.transcript.push(crate::transcript::Entry::line(
            crate::transcript::Kind::Notice,
            "",
        ));
        self.last_char = Some('\n');
    }

    /// Writes a message the human typed into the transcript, where the
    /// conversation reached.
    ///
    /// It is the echo that used to happen when the line was sent: a steering
    /// message is not in the history until the engine applies it, and printing
    /// it earlier claimed something that had not happened. It keeps the live
    /// prompt's marker, so it reads as the same message it always was.
    pub(crate) fn arrived_message(&mut self, text: &str) {
        if !self.verbosity.at_least(Verbosity::Medium) || matches!(self.format, Format::Jsonl) {
            return;
        }
        self.stop_bar();
        self.end_blocks();
        // The message is the conversation: it goes there once, and the view
        // draws the `›` that marks who said it.
        let escaped = crate::text::terminal(text);
        let mut entry = crate::transcript::Entry::new(crate::transcript::Kind::Said);
        entry.push_text(&escaped);
        self.transcript.push(entry);
        self.last_char = Some('\n');
        if !self.failed && !self.cancelled {
            self.mark_working();
        }
    }

    /// Adds what the human said to the conversation.
    ///
    /// It is called when the message is sent and not when the engine applies
    /// it: the person who typed it should see it immediately, and the engine's
    /// own report of it is for the history, not for the screen.
    pub(crate) fn said(&mut self, text: &str) {
        if matches!(self.format, Format::Jsonl) {
            return;
        }
        self.end_blocks();
        let escaped = crate::text::terminal(text);
        let mut entry = crate::transcript::Entry::new(crate::transcript::Kind::Said);
        entry.push_text(&escaped);
        self.transcript.push(entry);
        self.last_char = Some('\n');
    }

    /// Shows a message that reached the running request without the human
    /// typing it, with the time it arrived.
    ///
    /// A background job reports this way: its result has no tool call left to
    /// answer, so it reaches the model as a steering message. The time is the
    /// point — a job started minutes ago answers now, and a line without one
    /// leaves the reader wondering when the work it asked for came back.
    pub(crate) fn steer_arrived(&mut self, text: &str) {
        if !self.verbosity.at_least(Verbosity::Medium) || matches!(self.format, Format::Jsonl) {
            return;
        }
        // The bar and the answer share this stream, so both are closed before a
        // line is written onto it.
        self.stop_bar();
        self.end_blocks();
        // The time is part of what was said: a job started minutes ago answers
        // now, and the line without one reads as something typed just then.
        let when = chrono::Local::now().format("%H:%M:%S").to_string();
        let escaped = crate::text::terminal(text);
        let mut entry = crate::transcript::Entry::new(crate::transcript::Kind::Said);
        entry.push_line(format!("◀ {when}"));
        entry.push_text(&escaped);
        self.transcript.push(entry);
        self.last_char = Some('\n');
        // The turn goes on: the request that just took the message is still
        // being answered.
        if !self.failed && !self.cancelled {
            self.mark_working();
        }
    }

    /// Writes one past message of a resumed session, so the client shows where
    /// the conversation was before the turn continues.
    ///
    /// A user message keeps the live prompt's `›` marker; an assistant message
    /// is rendered as markdown, like the answer it once was. Tool and system
    /// messages are shown dimmed so they read as context, not as the answer.
    pub(crate) fn replay_message(&mut self, index: u64, role: &str, text: &str) {
        if !self.verbosity.at_least(Verbosity::Medium) || matches!(self.format, Format::Jsonl) {
            return;
        }
        // The working bar shares stderr with the transcript, so the replay stops
        // it before printing, exactly as the reasoning gutter does.
        self.stop_bar();
        self.space();
        // The replay is the conversation that already happened, so it goes to
        // the transcript in one piece: a resumed session shows what was said
        // and not a second rendering of it.
        let kind = if role == "user" {
            crate::transcript::Kind::Said
        } else {
            crate::transcript::Kind::Answer
        };
        let mut entry = crate::transcript::Entry::new(kind);
        if role != "user" && role != "assistant" {
            // A role the client does not draw on its own says which it is: a
            // system or developer message reads as neither side of the
            // conversation.
            entry.push_line(format!("#{index} {role}"));
        }
        // The markdown is a way of drawing the answer, and the text is what was
        // said: what is kept is the text, and the view decides how it looks.
        entry.push_text(&crate::text::terminal(text));
        self.transcript.push(entry);
        self.last_char = Some('\n');
    }

    /// Moves the renderer into the reasoning gutter.
    ///
    /// The text reaches the conversation through `record`, which is the one
    /// place an event becomes an entry; what is left here is the block state
    /// the rest of the renderer reads, and half a line with no text is not a
    /// transition — a keepalive must not open a gutter.
    pub(crate) fn reasoning_delta(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.block = Block::Reasoning;
        self.last_char = text.chars().last();
    }

    /// Writes one chunk of a tool's streamed output, in a box of its own.
    ///
    /// The box is drawn like the reasoning gutter, because it is the same kind
    /// of thing: text produced while something runs, held apart from the answer
    /// so the two never read as one, and grey so it reads as machine output
    /// rather than as something the model said.
    ///
    /// ```text
    ///   ╭
    ///   │ running 5 tests
    ///   │ test result: ok
    ///   ╰
    /// ```
    ///
    /// No heading names the stream: the `→ shell` line above already names the
    /// call, and a command's stdout and stderr are read by two tasks at once, so
    /// a label per block would name whichever happened to arrive first.
    ///
    /// Chunk boundaries are not line boundaries — a reader hands over whatever a
    /// pipe read returned — so a partial last line is left open for the next
    /// chunk to finish. The chunk is assembled and written once: two writers
    /// share this stream, and a frame written in pieces can be broken in half.
    fn output_delta(&mut self, call_id: &str, stream: &str, text: &str) {
        // The call already finished. A tool whose process outlives it can write
        // once more after that — the last chunk before it was let go of — and it
        // belongs to the box that closed, not to a second one: the same call
        // drawn twice reads as two calls.
        if self.closed_calls.contains(call_id) {
            return;
        }
        if self.output_gutter.as_deref() != Some(call_id) {
            // Whatever box was open closes first, and its rows are already in
            // the conversation: what is left is to open this one.
            self.close_output_gutter();
            // The box is not opened here: it is one entry the view frames, and
            // it is pushed when the call closes. Opening it at the first chunk
            // put its edges in the middle of the work — the top rule arrived
            // before the output and the bottom one an unknown number of rows
            // later, which is what left a `│` running down one side of the
            // transcript.
            self.output_gutter = Some(call_id.to_owned());
            self.output_line_start = true;
        }
        // The buffer is filled and read in one borrow, and the rows are written
        // after it ends: writing needs `self`, mutably.
        let mut whole: Vec<String> = Vec::new();
        let mut leftover = String::new();
        {
            let pending = self.output_pending.entry(stream.to_owned()).or_default();
            pending.push_str(text);
            // Whole lines only: what follows the last newline waits for the rest
            // of its line, whenever that arrives. A line that grows past the cap
            // without one is taken anyway, so a progress bar still moves.
            while let Some(index) = pending.find('\n') {
                let line: String = pending.drain(..=index).collect();
                whole.push(line[..line.len() - 1].to_owned());
            }
            if pending.chars().count() >= MAX_PARTIAL_LINE_CHARS {
                leftover = std::mem::take(pending);
            }
            // An empty buffer is no buffer: leaving the entry behind makes a
            // stream look like it is mid-line when it has nothing pending.
            if pending.is_empty() {
                self.output_pending.remove(stream);
            }
        }
        for line in whole {
            self.keep_output_row(&line);
        }
        if !leftover.is_empty() {
            self.keep_partial_row(&leftover);
        }
        self.last_char = None;
    }

    /// Keeps one whole line of tool output, inside the box.
    ///
    /// The rows are what the box holds and the view draws: how many were kept,
    /// and which of them, is decided here, and nothing about it depends on
    /// where the cursor was.
    ///
    /// The row carries no bar and no marker: the box the view draws is what
    /// says where the output begins and ends, and a `│` written into every row
    /// is a `│` a person selects along with the output they meant to copy.
    fn keep_output_row(&mut self, line: &str) {
        self.output_rows_seen += 1;
        if self.output_rows_seen > self.output_limit() {
            // Past the bound the row is not kept, and the count says how many
            // were left out. The newest are the ones worth keeping, so this is
            // the oldest being dropped.
            return;
        }
        self.output_rows
            .push(crate::text::terminal(line).into_owned());
    }

    /// Keeps a line that has not ended yet, ending it for the moment.
    ///
    /// A chunk can stop mid-line and the rest may never come (a progress bar),
    /// so the row is kept whole and the next chunk starts its own.
    fn keep_partial_row(&mut self, text: &str) {
        self.output_rows
            .push(crate::text::terminal(text).into_owned());
    }

    /// Keeps what was left of a stream that ended without a newline.
    fn flush_output_pending(&mut self) {
        for text in std::mem::take(&mut self.output_pending).into_values() {
            if !text.is_empty() {
                self.keep_partial_row(&text);
            }
        }
    }

    /// Closes the open output box, if one is open.
    ///
    /// The rows go to the conversation as **one** entry the view frames, rather
    /// than as a row at a time. A streamed command used to add an entry per
    /// line with its own margin, so a call that printed a hundred lines was a
    /// hundred entries with a box drawn around the whole run — which is what
    /// bloated the transcript, and what made a box's edge land in the middle of
    /// the work it was meant to bound.
    fn close_output_gutter(&mut self) {
        let Some(call_id) = self.output_gutter.take() else {
            return;
        };
        self.flush_output_pending();
        let hidden = self.output_rows_seen.saturating_sub(self.output_limit());
        let mut entry = crate::transcript::Entry::bordered(
            crate::transcript::Kind::ToolOutput,
            format!("shell #{}", short(&call_id)),
        );
        for row in std::mem::take(&mut self.output_rows) {
            entry.push_line(row);
        }
        if hidden > 0 {
            entry.push_inked(
                crate::transcript::Ink::Yellow,
                format!("… {hidden} earlier line(s) left out"),
            );
        }
        self.transcript.push(entry);
        self.output_rows_seen = 0;
        self.output_line_start = true;
    }

    /// Closes the open streamed-output box, if there is one.
    fn end_output_gutter(&mut self) {
        self.close_output_gutter();
    }

    /// Writes answer text, closing reasoning and separating it first.
    pub(crate) fn answer_delta(&mut self, text: &str) {
        let text = crate::text::terminal(text);
        // The first answer token is the same signal as the first reasoning one.
        if !text.is_empty() {
            self.set_wait(super::loading::Wait::Streaming);
        }
        if !self.verbosity.at_least(Verbosity::Medium) {
            // `low` is the answer alone, and it is still the conversation: the
            // entry is made by `record`, which is the one place an event
            // becomes an entry. What the level changes is the chrome around it
            // and not whether the answer is kept.
            if let Some(last) = text.chars().last() {
                self.last_char = Some(last);
            }
            return;
        }
        // A delta with no text changes nothing: a keepalive or the stream's
        // trailing empty part must not close the reasoning gutter or open an
        // answer, which is what split one thought into several `┌ thinking`
        // blocks and interleaved an answer with its reasoning.
        if text.is_empty() {
            return;
        }
        // Stop the loading bar on the first answer token.
        self.stop_bar();
        if self.block == Block::Reasoning {
            self.end_reasoning();
            self.space();
        } else if self.block != Block::Answer {
            if self.last_char.is_some() {
                self.space();
            }
            self.block = Block::Answer;
        }
        self.block = Block::Answer;
        // The answer reaches the conversation through `record`, which is the
        // one place an event becomes an entry: writing it here as well put every
        // token in twice.
        self.answer_buffer.push_str(&text);
        if let Some(last) = text.chars().last() {
            self.last_char = Some(last);
        }
    }

    /// Renders a fenced block, so verbose tool activity stays readable and can
    /// be copied into a markdown report.
    /// Keeps a fenced block, so verbose activity stays readable in the
    /// conversation and can be copied out of it.
    ///
    /// The fence is part of the row: a block of JSON the model was asked with
    /// is text a person reads, and it is drawn where it happened.
    pub(crate) fn code_block(&mut self, language: &str, text: &str) {
        let text = crate::text::terminal(text);
        let mut entry = crate::transcript::Entry::new(crate::transcript::Kind::ToolOutput);
        entry.push_line(format!("```{language}"));
        if text.is_empty() {
            entry.push_line("(empty)");
        } else {
            entry.push_text(&text);
        }
        entry.push_line("```");
        self.transcript.push(entry);
    }

    /// Renders tool activity: what started, and how it ended.
    #[allow(clippy::collapsible_match, clippy::too_many_lines)]
    pub(crate) fn tool_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::ToolStarted {
                tool,
                call_id,
                arguments,
                source,
                ..
            } => {
                *self.tool_counts.entry(tool.clone()).or_default() += 1;
                if let Some(todos) = todo_items(arguments.as_ref()).filter(|_| tool == "todo") {
                    self.pending_todos.insert(call_id.clone(), todos);
                }
                // The finish event carries the tool's output but not its
                // arguments, and a write's result is mostly the path it wrote.
                if matches!(tool.as_str(), "write" | "read") {
                    if let Some(path) = arguments
                        .as_ref()
                        .and_then(|arguments| arguments.get("path"))
                        .and_then(serde_json::Value::as_str)
                    {
                        self.pending_paths.insert(call_id.clone(), path.to_owned());
                    }
                }
                let command = Self::command_summary(arguments.as_ref());
                let diff = if tool == "edit" {
                    edit_preview(arguments.as_ref(), source.as_deref())
                } else {
                    None
                };
                if self.verbosity.is_quiet() {
                    // `low` does not announce a tool run. An edit's diff is the
                    // exception, and it is printed below at every level.
                    if let Some(diff) = &diff {
                        self.end_blocks();
                        self.print_edit_diff(diff);
                    }
                } else {
                    self.end_blocks();
                    // The header goes to the conversation through `record`,
                    // which is the one place an event becomes an entry.
                    let _ = &command;
                    if let Some(diff) = &diff {
                        self.print_edit_diff(diff);
                    }
                    if self.verbosity.at_least(Verbosity::High)
                        && tool.as_str() != "ask_user"
                        && tool.as_str() != "edit"
                    {
                        if let Some(arguments) = arguments {
                            let text = serde_json::to_string_pretty(arguments)
                                .unwrap_or_else(|_| compact_json(arguments, 4_000));
                            let (text, truncated) = clip_text(&text, 4_000);
                            self.code_block("json", &text);
                            if truncated {
                                self.notice("arguments shortened for display");
                            }
                        }
                    }
                }
            }
            ServerEventKind::ToolFinished {
                tool,
                call_id,
                is_error,
                error,
                output,
                detached,
                duration_ms,
                ..
            } => {
                if let Some(todos) = self.pending_todos.remove(call_id) {
                    if tool == "todo" && !*is_error {
                        self.pinned.set_todos(todos);
                    }
                }
                let path = self.pending_paths.remove(call_id);
                if *is_error && tool == "edit" {
                    if let Some(error) = error {
                        self.end_blocks();
                        if self.verbosity.is_quiet() {
                            for line in error.lines() {
                                let escaped = crate::text::terminal(line);
                                let (text, _) = clip_text(&escaped, 400);
                                self.failure(text);
                            }
                        } else {
                            self.code_block("text", error);
                        }
                    }
                }
                // Whether the run is announced needs `medium`, but what it
                // produced is always shown: a write that landed nowhere and a
                // command that failed are the two things a person must not have
                // to ask for.
                self.stop_bar();
                // A tool that streamed its output has an open box; its closing
                // chunk usually closed it, but a call that died before sending
                // one would leave the box open and the next line inside it.
                self.end_output_gutter();
                // From here the call has no box, and a chunk that arrives after
                // this one must not open another.
                self.closed_calls.insert(call_id.clone());
                if self.verbosity.at_least(Verbosity::Medium) {
                    // How the call ended is the same row the call began on,
                    // rewritten: a row that said what was running now says how
                    // it went, on the line it already occupied. The two used to
                    // be separate rows, so a call was two lines and the reader
                    // had to pair them by the id beside them.
                    let text = call_ending(
                        crate::style::Style::plain(),
                        tool,
                        *is_error,
                        *detached,
                        *duration_ms,
                    );
                    let kind = if *is_error {
                        crate::transcript::Kind::Failure
                    } else {
                        crate::transcript::Kind::ToolOutput
                    };
                    // The outcome has its own ink: green for a call that
                    // finished, red for one that failed, and the quieter yellow
                    // for one that was let go of — its process is still running,
                    // which is neither. While it runs the row is grey, so the
                    // colour a person sees is what the call became.
                    let outcome = if *detached {
                        crate::transcript::Ink::Yellow
                    } else if *is_error {
                        crate::transcript::Ink::Red
                    } else {
                        crate::transcript::Ink::Green
                    };
                    match self.transcript.call_mut(call_id) {
                        Some(entry) => {
                            entry.kind = kind;
                            entry.started = None;
                            entry.lines = vec![crate::transcript::Line::inked(outcome, text)];
                        }
                        // A call whose beginning was never shown — `low` keeps
                        // the announcement out of the conversation — still has
                        // an ending worth reading, and it opens its own row.
                        None => {
                            self.transcript
                                .push(crate::transcript::Entry::line(kind, text));
                        }
                    }
                }
                if *is_error && !*detached && tool.as_str() != "edit" {
                    // A failure needs its reason whatever the verbosity. A
                    // detached call's text is not a reason — it is the answer the
                    // model got, naming the job the process became — and it is
                    // shown below like any other result.
                    if let Some(error) = error {
                        self.keep_tool_result(error);
                    }
                } else if *detached && !self.streamed_calls.remove(call_id) {
                    // The call answered with what it had and the name of the job
                    // the rest belongs to, so its result is what the model saw.
                    if let Some(result) = Self::tool_result_for(
                        tool,
                        self.verbosity,
                        path.as_deref(),
                        output.as_deref(),
                    ) {
                        self.keep_tool_result(&result);
                    }
                } else if !self.streamed_calls.remove(call_id) {
                    // The output was printed as it arrived, so showing the
                    // result would print the same text twice. A failure is the
                    // exception above: its reason is not part of the stream.
                    if let Some(result) = Self::tool_result_for(
                        tool,
                        self.verbosity,
                        path.as_deref(),
                        output.as_deref(),
                    ) {
                        self.keep_tool_result(&result);
                    }
                }
                // The turn is not over: the model has the result and is being
                // asked again. Without this the rest of the turn ran with no
                // signal at all, and a slow continuation looked like a hang.
                if self.waiting.is_none() && !self.failed && !self.cancelled {
                    self.mark_working();
                }
                if self.verbosity.at_least(Verbosity::High) {
                    if *is_error {
                        if tool.as_str() != "edit" {
                            if let Some(error) = error {
                                self.code_block("text", error);
                            }
                        }
                    } else if let Some(output) = output {
                        self.code_block("text", output);
                    }
                }
            }
            ServerEventKind::ToolOutput {
                call_id,
                text,
                stream,
                last,
                ..
            } => {
                // The closing chunk carries no bytes, so it is handled before
                // the emptiness check: the block it ends has to be closed, and
                // returning here would leave its bracket unprinted.
                if *last {
                    self.end_output_gutter();
                }
                if text.is_empty() {
                    return;
                }
                // The call is remembered so its finish does not print the same
                // text a second time in the result block below.
                self.streamed_calls.insert(call_id.clone());
                // The bar owns the line under the message and is written by the
                // same stream, so a streamed line stops it first, like every
                // other writer.
                self.stop_bar();
                self.end_reasoning();
                self.block = Block::None;
                self.output_delta(call_id, stream, text);
            }
            _ => {}
        }
    }

    /// Whether a call streamed its output, for a test.
    #[cfg(test)]
    pub(crate) fn streamed_calls(&self) -> Vec<String> {
        self.streamed_calls.iter().cloned().collect()
    }

    /// The call whose output box is open, for a test.
    #[cfg(test)]
    pub(crate) fn output_gutter(&self) -> Option<&str> {
        self.output_gutter.as_deref()
    }

    /// How many rows the open box has seen, for a test.
    #[cfg(test)]
    pub(crate) fn output_rows_seen(&self) -> usize {
        self.output_rows_seen
    }

    /// How many rows the open box is holding, for a test.
    #[cfg(test)]
    pub(crate) fn output_rows_len(&self) -> usize {
        self.output_rows.len()
    }

    /// The bound the box is keeping to, for a test.
    #[cfg(test)]
    pub(crate) fn output_limit_for_test(&self) -> usize {
        self.output_limit()
    }

    /// What is waiting for a newline, for a test.
    #[cfg(test)]
    pub(crate) fn output_pending(&self) -> Vec<(String, String)> {
        self.output_pending
            .iter()
            .map(|(stream, text)| (stream.clone(), text.clone()))
            .collect()
    }

    /// The short result of a finished tool, when one is worth showing.
    ///
    /// A command shows its output, because that is what it was run for; a write
    /// names the file it wrote and a few lines of what landed there; a read
    /// shows what it returned. `edit` is absent: it already has its boxed diff.
    /// No other tool is summarised, so the transcript stays about the work.
    pub(crate) fn tool_result(
        tool: &str,
        path: Option<&str>,
        output: Option<&str>,
    ) -> Option<String> {
        // A write and a read are about a file, so the path leads and the output
        // follows it; a command is about what it printed.
        if matches!(tool, "write" | "read") {
            let path = path?;
            return Some(match output {
                Some(output) if !output.trim().is_empty() => format!("{path}\n{output}"),
                _ => path.to_owned(),
            });
        }
        // A shell's result is what it printed, whether it was waited for or let
        // go of: a detached call answers with the job id instead, which is not
        // this, and its output never arrives this way.
        if tool == "shell" {
            return output.map(str::to_owned);
        }
        None
    }

    /// What a finished call's result shows at a verbosity.
    ///
    /// `read` returns a page of a file: the model needs it and a person
    /// following the work does not. At `high` the page is shown; at `medium`
    /// the read is named — the path, its kind and how much it holds — and the
    /// page is left out; at `low` neither, because `low` is the answer alone.
    /// Every other tool keeps [`Self::tool_result`], since a write that landed
    /// nowhere and a command that failed are the two things a person must not
    /// have to ask for.
    pub(crate) fn tool_result_for(
        tool: &str,
        verbosity: Verbosity,
        path: Option<&str>,
        output: Option<&str>,
    ) -> Option<String> {
        if tool == "read" {
            return match verbosity {
                Verbosity::Low => None,
                Verbosity::Medium => read_summary(path, output),
                Verbosity::High => Self::tool_result(tool, path, output),
            };
        }
        Self::tool_result(tool, path, output)
    }

    /// Prints a tool result as a short, dimmed, indented block.
    ///
    /// Bounded on purpose: the model may have received far more than a person
    /// wants to read, so each section shows its first lines and a count replaces
    /// the rest. `stdout` and `stderr` are budgeted apart, because a long stdout
    /// would otherwise hide the line of stderr that explains a failure.
    /// Keeps the result of a finished tool, as the rows the view draws.
    ///
    /// The result is what the model was told, which is the one thing a person
    /// has to be able to read after a call: it is bounded, and what the bound
    /// dropped is counted rather than lost.
    fn keep_tool_result(&mut self, text: &str) {
        let escaped = crate::text::terminal(text);
        let sections = tool_sections(&escaped);
        let labelled = sections
            .iter()
            .filter(|section| section.name.is_some())
            .count()
            > 1;
        let mut entry = crate::transcript::Entry::new(crate::transcript::Kind::ToolOutput);
        let mut first = true;
        for section in &sections {
            if !first {
                entry.push_line("");
            }
            first = false;
            if labelled {
                if let Some(name) = &section.name {
                    entry.push_inked(crate::transcript::Ink::Cyan, name.clone());
                }
            }
            let mut kept = crate::transcript::Entry::new(crate::transcript::Kind::ToolOutput);
            let hidden = tool_lines(&mut kept, &section.lines);
            for line in kept.lines {
                entry.push_line(line.text);
            }
            if hidden > 0 {
                entry.push_inked(
                    crate::transcript::Ink::Yellow,
                    format!("… {hidden} more line(s)"),
                );
            }
        }
        self.transcript.push(entry);
    }

    /// Prints each edit as its own box: the original and resulting lines side by
    /// side, with line numbers and a little surrounding context. This is
    /// deliberately independent of `--quiet` and `--verbose`: changing a file is
    /// the one tool effect a human needs to see.
    fn print_edit_diff(&mut self, preview: &EditPreview) {
        let width = edit_box_width();
        for edit in &preview.boxes {
            self.print_edit_box(&preview.path, edit, width);
        }
    }

    /// Renders one boxed edit in whichever shape `ui.diff_style` asks for.
    fn print_edit_box(&mut self, path: &str, edit: &EditBox, width: usize) {
        match self.diff_style {
            DiffStyle::Unified => self.keep_edit_box_unified(path, edit, width),
            DiffStyle::Split => self.keep_edit_box_split(path, edit, width),
        }
    }

    /// Renders one boxed edit as a patch: one column, `-` for what left and
    /// `+` for what arrived.
    ///
    /// The diff is one bounded thing, so the view draws a frame around it and a
    /// person can read where the edit starts and ends without the frame being
    /// part of what they select. The rows carry their ink, so the removal and
    /// the addition are told apart by the widget and not by escape sequences in
    /// the text.
    fn keep_edit_box_unified(&mut self, path: &str, edit: &EditBox, width: usize) {
        let mut entry = crate::transcript::Entry::bordered(
            crate::transcript::Kind::ToolOutput,
            Self::edit_title(path, edit),
        );
        for (ink, text) in Self::unified_lines(self.style, path, edit, width) {
            entry.push_inked(ink, text);
        }
        self.transcript.push(entry);
    }

    /// What the top rule of an edit's box says.
    ///
    /// The same words in both shapes, so a person who switches `ui.diff_style`
    /// does not have to learn a second set of words for the same fact: which
    /// edit of how many, where the lines it touched are, and which file.
    pub(crate) fn edit_title(path: &str, edit: &EditBox) -> String {
        let approximate = if edit.approximate {
            " · approximate"
        } else {
            ""
        };
        format!(
            "edit {}/{} · {}{approximate} · {}",
            edit.index,
            edit.total,
            edit_range(edit),
            preview_path(path)
        )
    }

    /// Renders one boxed edit side by side.
    ///
    /// The panes are laid out with a blank gutter and not with `│` between
    /// them: a bar between two columns is a bar a person selects along with the
    /// line they meant to copy, and the frame the view draws around the whole
    /// diff is what says where it begins and ends.
    fn keep_edit_box_split(&mut self, path: &str, edit: &EditBox, width: usize) {
        let mut entry = crate::transcript::Entry::bordered(
            crate::transcript::Kind::ToolOutput,
            Self::edit_title(path, edit),
        );
        for (ink, text) in Self::split_lines(edit, width) {
            entry.push_inked(ink, text);
        }
        self.transcript.push(entry);
    }

    /// The rows of a side-by-side diff, each with the ink it is written in.
    ///
    /// The two panels are sized independently so the right one ends where it
    /// should on an odd terminal width.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn split_lines(edit: &EditBox, width: usize) -> Vec<(Ink, String)> {
        let panel_left = width.saturating_sub(4) / 2;
        let panel_right = width.saturating_sub(4).saturating_sub(panel_left);
        if panel_left < 10 || panel_right < 10 {
            // A terminal too narrow for two columns gets the unified shape,
            // which is the one that fits: half a diff is worse than a patch.
            return Self::unified_lines(Style::plain(), "", edit, width);
        }
        let digits = edit
            .rows
            .iter()
            .flat_map(|row| [row.left.as_ref(), row.right.as_ref()])
            .filter_map(|cell| cell.and_then(|cell| cell.number))
            .map(|number| number.to_string().len())
            .max()
            .unwrap_or(3)
            .clamp(3, 6);
        let mut lines = vec![(
            Ink::Dim,
            format!(
                "{:<panel_left$}  {}",
                clip_plain("original", panel_left),
                clip_plain("result", panel_right)
            ),
        )];
        for row in &edit.rows {
            // The panel text is plain: the panels are laid out as text and the
            // frame draws them, because painting here would put the escape
            // sequences on screen as the characters they are.
            // kind so a removal reads red on both sides of the pair.
            let left = render_panel_plain(row.left.as_ref(), Side::Left, panel_left, digits);
            let right = render_panel_plain(row.right.as_ref(), Side::Right, panel_right, digits);
            let ink = if row.left.as_ref().is_some_and(|cell| cell.changed) {
                Ink::Red
            } else if row.right.as_ref().is_some_and(|cell| cell.changed) {
                Ink::Green
            } else {
                Ink::Faint
            };
            lines.push((ink, format!("{left}  {right}")));
        }
        if edit.omitted > 0 {
            lines.push((Ink::Yellow, format!("… {} more line(s) …", edit.omitted)));
        }
        lines
    }

    /// Returns the rows of a unified patch, each with the ink it is written in.
    ///
    /// The ink is carried as data and not wrapped around the text. The rows used
    /// to come back as strings already painted with `style.red(...)` and the
    /// rest, which is a form for a printer writing to a terminal: a frame draws
    /// text, so it put the escape sequences on screen as the characters they
    /// are.
    ///
    /// `path` is not read here: the caller puts it on the box's top rule, which
    /// is the view's chrome and not a row of the diff.
    pub(crate) fn unified_lines(
        _style: Style,
        _path: &str,
        edit: &EditBox,
        width: usize,
    ) -> Vec<(Ink, String)> {
        let inner = width.saturating_sub(4).max(1);
        let digits = edit
            .rows
            .iter()
            .flat_map(|row| [row.left.as_ref(), row.right.as_ref()])
            .filter_map(|cell| cell.and_then(|cell| cell.number))
            .map(|number| number.to_string().len())
            .max()
            .unwrap_or(3)
            .clamp(3, 6);
        let mut lines = Vec::new();
        for row in unify(&edit.rows) {
            let number = row
                .number
                .map_or_else(|| " ".repeat(digits), |number| format!("{number:>digits$}"));
            let mark = row.kind.mark();
            let room = inner.saturating_sub(digits + 3);
            let text = format!("{:<room$}", clip_plain(&row.text, room));
            let (ink, text) = match row.kind {
                UnifiedKind::Context => (Ink::Faint, format!("{number} {mark} {text}")),
                UnifiedKind::Removed => (Ink::Red, format!("{number} {mark} {text}")),
                UnifiedKind::Added => (Ink::Green, format!("{number} {mark} {text}")),
            };
            lines.push((ink, text));
        }
        if edit.omitted > 0 {
            let note = format!("… {} more line(s) …", edit.omitted);
            lines.push((Ink::Yellow, note));
        }
        lines
    }

    /// What a call says while it is running.
    ///
    /// A row is one line and it has to answer "what is happening" without the
    /// reader knowing the tool's name: `read` takes a path, `grep` takes a
    /// pattern, `shell` takes a command. The tool's own name is what the row
    /// leads with when there is nothing better to say — a row that guessed at an
    /// argument would claim the call is doing something it is not.
    #[must_use]
    fn doing(tool: &str, arguments: Option<&serde_json::Value>) -> String {
        let text = |key: &str| {
            arguments
                .and_then(|arguments| arguments.get(key))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        match tool {
            "read" | "write" | "edit" => text("path")
                .or_else(|| text("pattern"))
                .map_or_else(|| tool.to_owned(), |path| format!("{tool} {path}")),
            "grep" => {
                text("pattern").map_or_else(|| tool.to_owned(), |pattern| format!("grep {pattern}"))
            }
            "shell" => match Self::command_summary(arguments) {
                Some(command) => format!("shell $ {command}"),
                None => tool.to_owned(),
            },
            _ => tool.to_owned(),
        }
    }

    /// Returns the command a shell tool call ran, clipped to one line.
    ///
    /// The schema is flat — `script` sits at the top level, `shell` beside it —
    /// and this read the nested `{"command": {"kind": "shell", ...}}` form the
    /// tool used before it was flattened. The lookup missed, so the `$` line
    /// under `→ shell` was never printed and a reader could see a command's
    /// output without ever seeing what produced it. Both shapes are accepted:
    /// an old transcript replayed from the store still carries the nested one.
    pub(crate) fn command_summary(arguments: Option<&serde_json::Value>) -> Option<String> {
        let arguments = arguments?;
        // The flat form first, which is what the tool sends now.
        if let Some(script) = arguments.get("script").and_then(serde_json::Value::as_str) {
            return Some(clip_command(script, 160));
        }
        let command = arguments.get("command")?;
        if let Some(text) = command.as_str() {
            return Some(clip_command(text, 160));
        }
        match command.get("kind").and_then(serde_json::Value::as_str) {
            Some("shell") => {
                let script = command.get("script").and_then(serde_json::Value::as_str)?;
                Some(clip_command(script, 160))
            }
            Some("program") => {
                let program = command.get("program").and_then(serde_json::Value::as_str)?;
                let args = command
                    .get("args")
                    .and_then(serde_json::Value::as_array)
                    .map(|args| {
                        args.iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                let text = if args.is_empty() {
                    program.to_owned()
                } else {
                    format!("{program} {args}")
                };
                Some(clip_command(&text, 160))
            }
            _ => None,
        }
    }

    /// Renders usage for one reconstructed request and keeps the run totals.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn usage_event(&mut self, kind: &ServerEventKind) {
        match kind {
            // A resume starts a fresh client, so the session's accumulated
            // usage arrives once and replaces whatever this process counted.
            ServerEventKind::SessionStats {
                input_tokens,
                output_tokens,
                cost_usd,
                requests,
                cache_read_tokens,
                cache_write_tokens,
                context_used,
                context_window,
            } => {
                self.input = *input_tokens;
                self.output = *output_tokens;
                self.cost = *cost_usd;
                self.requests = *requests;
                self.turn_requests = 0;
                // The cache share is over the whole session, not the requests
                // made since the resume.
                self.cache_read = *cache_read_tokens;
                self.cache_write = *cache_write_tokens;
                self.cache_reported = *cache_read_tokens > 0 || *cache_write_tokens > 0;
                self.context_used = *context_used;
                self.max_context = *context_window;
                if self.verbosity.at_least(Verbosity::Medium) && matches!(self.format, Format::Text)
                {
                    let status = self.session_status();
                    self.pinned.set_session(status);
                }
            }
            ServerEventKind::RequestReconstructed {
                usage,
                cost_usd,
                truncated,
                duration_ms,
                first_token_ms,
                tokens_per_second,
                cache_hit,
                tools,
                ..
            } => {
                self.requests += 1;
                self.turn_requests += 1;
                self.input += usage.input_tokens.unwrap_or(0);
                self.output += usage.output_tokens.unwrap_or(0);
                if let Some(input) = usage.input_tokens {
                    self.context_used = input;
                }
                if let Some(cache) = usage.cache_read_tokens {
                    self.cache_read += cache;
                    self.cache_reported = true;
                }
                if let Some(cache) = usage.cache_write_tokens {
                    self.cache_write += cache;
                }
                if let Some(cost) = cost_usd {
                    self.cost += cost;
                } else {
                    self.unpriced += 1;
                }
                if self.verbosity.at_least(Verbosity::Medium) && matches!(self.format, Format::Text)
                {
                    let status = self.session_status();
                    self.pinned.set_session(status);
                }
                if !self.verbosity.at_least(Verbosity::Medium) {
                    return;
                }
                // The bar has to go first whatever the block is: with no answer
                // text and no reasoning, `block` is `None` and the summary below
                // was printed onto the bar's own line.
                self.stop_bar();
                if self.block != Block::None {
                    self.end_blocks();
                }
                // Four things, in the order they are worth reading: which model
                // answered, how fast it went, how much of the prompt its cache
                // served, and what it cost. At `high` the rest joins them --
                // the token counts, the wall time, the first-token wait, the
                // input:output ratio and the tools. `medium` is the line a
                // person reads while working; it is not a report, and a line
                // that grows to eleven fields is one nobody finishes.
                let high = self.verbosity.at_least(Verbosity::High);
                let mut parts = Vec::new();
                if let Some(name) = &self.model_name {
                    parts.push(self.style.dim(name.clone()));
                }
                if high {
                    if let Some(input) = usage.input_tokens {
                        parts.push(self.style.dim(format!("{} in", human_tokens(input))));
                    }
                    if let Some(output) = usage.output_tokens {
                        parts.push(self.style.dim(format!("{} out", human_tokens(output))));
                    }
                }
                if let Some(rate) = tokens_per_second {
                    parts.push(self.style.dim(format!("{rate:.1} tok/s")));
                }
                if high {
                    if *duration_ms > 0 {
                        parts.push(
                            self.style
                                .dim(format!("took {}", duration_label(*duration_ms))),
                        );
                    }
                    if let Some(first) = first_token_ms {
                        parts.push(
                            self.style
                                .dim(format!("latency {}", duration_label(*first))),
                        );
                    }
                }
                if let (Some(cache), Some(input)) = (usage.cache_read_tokens, usage.input_tokens) {
                    parts.push(
                        self.style
                            .dim(format!("cache {}", cache_percent_label(cache, input))),
                    );
                } else if *cache_hit {
                    parts.push(self.style.dim("cache hit".to_owned()));
                }
                if high {
                    if let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) {
                        parts.push(
                            self.style
                                .dim(format!("in:out {}", token_ratio(input, output))),
                        );
                    }
                    if !tools.is_empty() {
                        parts.push(self.style.dim(format!("tools {}", Self::tool_list(tools))));
                    }
                }
                if self.show_cost {
                    match cost_usd {
                        Some(cost) if *cost > 0.0 => {
                            parts.push(self.style.green(format!("${cost:.6}")));
                        }
                        Some(cost) => parts.push(self.style.faint(format!("${cost:.6}"))),
                        None if !self.cost_silent => {
                            parts.push(self.style.yellow("cost not reported".to_owned()));
                        }
                        None => {}
                    }
                }
                self.notice(format!("↳ {}", parts.join(" · ")));
                if *truncated {
                    self.notice("warning: the answer hit the output limit");
                }
            }
            ServerEventKind::CostNotReported { requests }
                if self.verbosity.at_least(Verbosity::Medium) && self.show_cost =>
            {
                self.stop_bar();
                self.notice(format!(
                    "warning: {requests} request(s) carried no cost; the total is incomplete"
                ));
            }
            _ => {}
        }
    }

    /// Renders errors, approval outcomes and the turn's final state.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn status_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::CompactionStarted { reason } => {
                if self.verbosity.at_least(Verbosity::Medium) {
                    self.end_blocks();
                    self.transcript.push(crate::transcript::Entry::line(
                        crate::transcript::Kind::Notice,
                        format!("compacting context… ({reason})"),
                    ));
                }
                self.set_activity(Some("compacting context…".to_owned()));
            }
            ServerEventKind::CompactionFinished {
                compacted,
                north_session,
                summary,
                error,
            } => {
                self.end_blocks();
                if *compacted {
                    // At `high`, what the history was replaced by: the summary
                    // is what the fresh session holds, and reading it is the
                    // only way to know whether the compaction kept what
                    // mattered. It is bounded and escaped like any other text
                    // the model wrote.
                    let detail = if self.verbosity.is_quiet() {
                        String::new()
                    } else {
                        north_session
                            .as_ref()
                            .map_or_else(String::new, |id| format!(" · new session {}", short(id)))
                    };
                    self.notice(format!("✓ context compacted{detail}"));
                    if self.verbosity.at_least(Verbosity::High) {
                        if let Some(summary) = summary {
                            let escaped = crate::text::terminal(summary);
                            let (text, truncated) = clip_text(&escaped, 8_000);
                            self.code_block("markdown", &text);
                            if truncated {
                                self.notice("summary shortened for display");
                            }
                        }
                    }
                } else {
                    let detail = error
                        .as_ref()
                        .map_or_else(String::new, |reason| format!(": {reason}"));
                    self.notice(format!(
                        "compaction did not happen; history is unchanged{detail}"
                    ));
                }
                self.set_activity(None);
            }
            ServerEventKind::TurnStarted { .. } => {
                self.cancelled = false;
                self.reasoning_placeholder = false;
                self.turn_requests = 0;
                self.pending_todos.clear();
                self.set_activity(None);
            }
            ServerEventKind::RetryScheduled {
                attempt,
                delay_ms,
                reason,
                ..
            } => {
                self.end_blocks();
                let wait = duration_label(*delay_ms);
                self.set_activity(Some(format!("retrying in {wait} (attempt {attempt})")));
                if self.verbosity.at_least(Verbosity::Medium) {
                    self.notice(format!("retrying in {wait} (attempt {attempt}): {reason}"));
                }
            }
            ServerEventKind::RetryStarted { attempt, .. } => {
                self.end_blocks();
                self.set_activity(None);
                if self.verbosity.at_least(Verbosity::Medium) {
                    self.notice(format!("retrying now (attempt {attempt})"));
                }
            }
            ServerEventKind::Error { code, message } => {
                self.end_blocks();
                self.failure(format!("error: {message} ({})", code_name(*code)));
                if !self.cancelled {
                    self.failed = true;
                }
            }
            ServerEventKind::ApprovalResolved { approved, .. } => {
                if self.verbosity.at_least(Verbosity::Medium) {
                    self.end_blocks();
                    if *approved {
                        self.notice("✓ approval granted");
                    } else {
                        self.failure("✗ approval denied");
                    }
                }
            }
            ServerEventKind::TurnFinished {
                committed, phase, ..
            } => {
                self.end_blocks();
                if !*committed && self.cancelled {
                    if self.verbosity.at_least(Verbosity::Medium) {
                        self.notice("turn cancelled · the session is saved and can be resumed");
                        if self.show_cost && self.turn_requests == 0 {
                            self.notice(
                                "no provider usage arrived before the cancel, so no cost is \
                                 shown; the cancelled request is still recorded in telemetry",
                            );
                        }
                    }
                } else if !*committed && self.budget_stopped {
                    if self.verbosity.at_least(Verbosity::Medium) {
                        let message = if self.budget_prompt.is_some() {
                            "stopped by budget"
                        } else {
                            "stopped by budget; raise it and resume the session to continue"
                        };
                        self.notice(message);
                    }
                } else if !*committed {
                    if let Some(reason) = self.stopped.take() {
                        // The host ended the turn on purpose; that is not a
                        // failure.
                        self.notice(format!("the turn was stopped: {reason}"));
                    } else {
                        self.failure("the turn did not commit");
                        self.failed = true;
                    }
                } else if *phase == Phase::AwaitingApproval
                    && self.verbosity.at_least(Verbosity::Medium)
                {
                    self.failure("paused: waiting for approval");
                }
                let activity = if *phase == Phase::AwaitingApproval {
                    Some("waiting for approval".to_owned())
                } else {
                    None
                };
                self.pending_todos.clear();
                self.set_activity(activity);
            }
            ServerEventKind::BudgetWarning {
                scope,
                unit,
                spent,
                limit,
            } => {
                if self.verbosity.at_least(Verbosity::Medium) {
                    self.stop_bar();
                    self.notice(format!(
                        "budget warning: {} {} of {} {}",
                        scope_name(*scope),
                        format_measure(*unit, *spent),
                        format_measure(*unit, *limit),
                        unit_name(*unit)
                    ));
                }
            }
            ServerEventKind::BudgetExceeded {
                scope,
                unit,
                spent,
                limit,
                action,
            } => {
                self.budget_stopped = !matches!(action, BudgetAction::Warn);
                self.budget_prompt = matches!(action, BudgetAction::Ask).then_some(BudgetPrompt {
                    scope: *scope,
                    unit: *unit,
                    spent: *spent,
                    limit: *limit,
                });
                self.stop_bar();
                self.failure(format!(
                    "budget exceeded: {} {} of {} {} ({})",
                    scope_name(*scope),
                    format_measure(*unit, *spent),
                    format_measure(*unit, *limit),
                    unit_name(*unit),
                    budget_action_name(*action)
                ));
            }
            ServerEventKind::BudgetChanged { budget } => {
                // Raising or clearing the budget makes a stopped session
                // resumable again; the next turn must not look stopped.
                self.budget_stopped = false;
                self.budget_prompt = None;
                if self.verbosity.at_least(Verbosity::Medium) {
                    self.stop_bar();
                    match budget {
                        Some(budget) => {
                            self.notice(format!("budget set: {}", budget_label(budget)));
                        }
                        None => self.notice("budget cleared"),
                    }
                }
            }
            ServerEventKind::ModelQueued { model }
                if self.verbosity.at_least(Verbosity::Medium) =>
            {
                self.stop_bar();
                self.notice(format!("model switch queued: {model}"));
            }
            ServerEventKind::ModelChanged { model }
                if self.verbosity.at_least(Verbosity::Medium) =>
            {
                self.stop_bar();
                self.notice(format!("model set: {model}"));
            }
            ServerEventKind::TurnStopped { reason } => {
                self.stopped = Some(reason.clone());
                if self.verbosity.at_least(Verbosity::Medium) {
                    self.stop_bar();
                    self.notice(format!("the turn was stopped: {reason}"));
                }
            }
            _ => {}
        }
    }

    pub(crate) fn delta(&mut self, delta: &Delta) {
        match delta {
            Delta::Text { text, .. } => self.answer_delta(text),
            Delta::Reasoning { text, .. } => {
                // An empty part never opens or reopens the gutter; it is the
                // same keepalive case as an empty text delta.
                if text.is_empty() {
                    return;
                }
                // The provider answered with a thought: the bar stops saying it
                // is waiting and says it is thinking, which is what is
                // happening. Only answer text moves it to `streaming`.
                self.set_wait(super::loading::Wait::Thinking);
                if self.verbosity.is_quiet() {
                    // `low` is the answer alone: not even the fact that the
                    // model is thinking is shown.
                } else if !self.verbosity.at_least(Verbosity::High) {
                    // `medium` says the model is thinking without drawing the
                    // thought. When the bar is running it is already saying
                    // that, on its own line, and it was told `streaming` a line
                    // above: a second `thinking…` line said it twice and, printed
                    // while the bar ran, pushed the bar onto a line of its own so
                    // the old frame stayed on screen. With no bar there is
                    // nothing else saying it, so the line is printed.
                    if !self.reasoning_placeholder && self.waiting.is_none() {
                        self.reasoning_placeholder = true;
                        // The placeholder is chrome and not conversation: it says the model is
                        // working, which is a state and not something it said.
                        self.activity = Some("thinking…".to_owned());
                    }
                } else {
                    // The bar and the gutter share stderr, so the first reasoning
                    // token clears the bar before the gutter writes over it.
                    self.stop_bar();
                    if self.block != Block::Reasoning {
                        if self.block == Block::Answer {
                            self.end_blocks();
                        } else if self.block == Block::None {
                            self.space();
                        }
                        self.block = Block::Reasoning;
                        self.reasoning_line_start = true;
                    }
                    self.reasoning_delta(text);
                }
            }
            Delta::Refusal { .. } => {
                self.end_blocks();
                self.failed = true;
            }
            Delta::ToolArguments { .. } | Delta::Media { .. } => {}
        }
    }

    /// Formats the tools of one request, with counts, in first-seen order.
    pub(crate) fn tool_list(tools: &[String]) -> String {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for tool in tools {
            if let Some((_, count)) = counts.iter_mut().find(|(name, _)| *name == tool) {
                *count += 1;
            } else {
                counts.push((tool, 1));
            }
        }
        counts
            .into_iter()
            .map(|(tool, count)| format!("{tool} ×{count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Formats how many times each tool ran, in name order.
    pub(crate) fn tool_summary(&self) -> String {
        self.tool_counts
            .iter()
            .map(|(tool, count)| format!("{} ×{count}", self.style.tool(tool.clone())))
            .collect::<Vec<_>>()
            .join(&self.style.faint(" · "))
    }

    /// The running session totals for the pinned footer.
    ///
    /// One line, always the same fields, so the footer only ever updates in
    /// place. What it says depends on the verbosity, and the difference is the
    /// point: `medium` is the line a person reads while working — what the
    /// context is doing, how much of it the cache served, what it has cost —
    /// and `high` adds the counters that are for looking at rather than
    /// reading: requests, input and output totals, and the input:output ratio.
    pub(crate) fn session_status(&self) -> String {
        // The dial shows the live context against the model's window, so a turn
        // that is filling the window is visible before compaction hits. It is red
        // while a compaction is running.
        let compacting = self
            .activity
            .as_deref()
            .is_some_and(|activity| activity.starts_with("compacting"));
        let mut parts = Vec::new();
        if self.verbosity.at_least(Verbosity::High) {
            parts.push(self.style.cyan(format!("{} req", self.requests)));
            parts.push(self.style.dim(format!("{} in", human_tokens(self.input))));
            parts.push(self.style.dim(format!("{} out", human_tokens(self.output))));
        }
        // The dial stays at both levels: it is the one number that changes
        // while a turn runs, and the one that decides when compaction hits.
        parts.push(context_dial(
            self.style,
            self.context_used,
            self.max_context,
            self.compaction_threshold,
            compacting,
        ));
        if self.cache_reported && self.input > 0 {
            parts.push(self.style.blue(format!(
                "cache {}",
                cache_percent_label(self.cache_read, self.input)
            )));
        }
        if self.verbosity.at_least(Verbosity::High) {
            parts.push(
                self.style
                    .dim(format!("in:out {}", token_ratio(self.input, self.output))),
            );
        }
        if self.show_cost {
            if self.requests > 0 && self.unpriced == self.requests {
                if !self.cost_silent {
                    parts.push(self.style.yellow("cost not reported"));
                }
            } else {
                parts.push(if self.cost > 0.0 {
                    self.style.green(format!("${:.6}", self.cost))
                } else {
                    self.style.faint(format!("${:.6}", self.cost))
                });
                if self.unpriced > 0 && !self.cost_silent {
                    parts.push(self.style.yellow(format!("{} not reported", self.unpriced)));
                }
            }
        }
        // The reasoning effort the session will ask for, which nothing else on
        // screen shows: after `/reasoning`, a downgrade to fit a new model, or a
        // resume, the transcript alone cannot say what the next turn sends. It
        // is a bar rather than the word for two reasons: the word has to be
        // mapped to an order in the reader's head, and the bar's colour says
        // which rung it is on at a glance, which is the thing that costs money.
        if let Some(effort) = &self.reasoning {
            parts.push(reasoning_bar(self.style, effort));
        }
        // The line leads with where the work is happening and what the turn is
        // doing. The directory is the one thing on it that never changes
        // between sessions, and the one a person checks when a tool reads the
        // wrong tree: it is named here rather than only in `--debug`, which is
        // a report nobody reads while working.
        let prefix = self.activity.as_ref().map_or_else(
            || self.style.faint("session"),
            |activity| self.style.yellow(activity.clone()),
        );
        let mut head = String::new();
        if self.show_workdir {
            if let Ok(dir) = std::env::current_dir() {
                head.push_str(&self.style.faint(short_path(&dir)));
                head.push_str(&self.style.faint(" · "));
            }
        }
        // What names the session, in front of the totals. The title comes first
        // because it is what a person recognises the session by; the key follows
        // in brackets, because the key is what `resume` is given and a title is
        // not unique. A session with no title yet leads with the key alone,
        // which is what it had before one was asked for.
        if self.verbosity.at_least(Verbosity::Medium) {
            if let Some(key) = &self.session {
                let short = short(key).to_owned();
                match self.title.as_deref().map(str::trim) {
                    Some(title) if !title.is_empty() => {
                        parts.insert(0, self.style.faint(format!("({short})")));
                        parts.insert(0, self.style.cyan(title.to_owned()));
                    }
                    _ => parts.insert(0, self.style.faint(short)),
                }
            }
        }
        format!("{head}{} {}", prefix, parts.join(&self.style.faint(" · ")))
    }

    /// Writes the run's cost and tool summary, if the run made any requests.
    pub(crate) fn summary(&mut self) {
        if matches!(self.format, Format::Jsonl)
            || !self.verbosity.at_least(Verbosity::Medium)
            || self.requests == 0
        {
            return;
        }
        let mut parts = vec![
            self.style.cyan(format!("{} request(s)", self.requests)),
            self.style.dim(format!("{} in", human_tokens(self.input))),
            self.style.dim(format!("{} out", human_tokens(self.output))),
        ];
        if self.cache_reported && self.input > 0 {
            parts.push(self.style.blue(format!(
                "cache {}",
                cache_percent_label(self.cache_read, self.input)
            )));
        }
        parts.push(
            self.style
                .dim(format!("in:out {}", token_ratio(self.input, self.output))),
        );
        if self.show_cost {
            if self.unpriced == self.requests {
                if !self.cost_silent {
                    parts.push(self.style.yellow("cost not reported"));
                }
            } else {
                parts.push(if self.cost > 0.0 {
                    self.style.green(format!("${:.6}", self.cost))
                } else {
                    self.style.faint(format!("${:.6}", self.cost))
                });
                if self.unpriced > 0 && !self.cost_silent {
                    parts.push(self.style.yellow(format!("{} not reported", self.unpriced)));
                }
            }
        }
        // The summary of the turn is the last thing it said, and it belongs to
        // the conversation it summarises.
        let mut entry = crate::transcript::Entry::new(crate::transcript::Kind::Notice);
        entry.push_line("");
        entry.push_line(format!("── {}", parts.join(" · ")));
        if !self.tool_counts.is_empty() {
            entry.push_line(format!("tools {}", self.tool_summary()));
        }
        self.transcript.push(entry);
    }
}

/// How many lines of one section a tool result shows.
const TOOL_RESULT_LINES: usize = 12;

/// How wide one shown line may be before it is clipped.
const TOOL_RESULT_LINE_CHARS: usize = 200;

/// One labelled run of lines inside a tool result.
#[derive(Debug)]
pub(crate) struct ToolSection {
    /// The label the tool wrote, when the text carries one.
    pub(crate) name: Option<String>,
    /// The lines the section holds.
    pub(crate) lines: Vec<String>,
}

/// One line naming a read, without the page it returned.
///
/// The read tool's result opens with `read <path> (<kind>, <n> line(s), <m>
/// bytes)` and the page follows it; the first line is the whole of what
/// `medium` shows. The output is the authority on the counts — the tool writes
/// them — so it is read from there and not recomputed, and a result that does
/// not carry the header falls back to the path alone rather than inventing one.
fn read_summary(path: Option<&str>, output: Option<&str>) -> Option<String> {
    let path = path?;
    let first = output?.lines().next()?;
    let rest = first.strip_prefix("read ")?;
    let named = rest.strip_prefix(path)?;
    let counts = named
        .strip_prefix(' ')
        .filter(|rest| rest.starts_with('('))?;
    Some(format!("{path} {counts}"))
}

/// Splits a tool result into the sections its own text declares.
///
/// A command renders `exit_status:`, `duration_ms:`, `stdout:`, `stderr:` and
/// `summary:`, and those are the labels a section can start with; nothing else
/// in the suite writes one at the start of a line. A text with no label is a
/// single unnamed section, so every tool still prints. A `summary:` is folded
/// into what precedes it, because it quotes lines already shown.
pub(crate) fn tool_sections(text: &str) -> Vec<ToolSection> {
    const LABELS: [&str; 5] = ["exit_status", "duration_ms", "stdout", "stderr", "summary"];
    let mut sections: Vec<ToolSection> = Vec::new();
    let mut current: Option<ToolSection> = None;
    // The summary quotes lines already shown above it, so it and everything
    // after it are dropped rather than opened as a section of their own.
    let mut in_summary = false;
    for line in text.lines() {
        let label = line
            .strip_suffix(':')
            .filter(|label| LABELS.contains(label));
        if label == Some("summary") {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            in_summary = true;
            continue;
        }
        if in_summary {
            continue;
        }
        match label {
            Some(label) => {
                if let Some(section) = current.take() {
                    sections.push(section);
                }
                current = Some(ToolSection {
                    name: Some(format!("{label}:")),
                    lines: Vec::new(),
                });
            }
            None => match current.as_mut() {
                Some(section) => section.lines.push(line.to_owned()),
                None => {
                    current = Some(ToolSection {
                        name: None,
                        lines: vec![line.to_owned()],
                    });
                }
            },
        }
    }
    if let Some(section) = current {
        sections.push(section);
    }
    sections
}
/// Keeps the lines of one section that fit, and says how many did not.
///
/// The cap is per section and not per result: a command that printed forty
/// lines of output should not hide the four lines of the failure that followed
/// them, which is what one budget for the whole result did.
fn tool_lines(entry: &mut crate::transcript::Entry, lines: &[String]) -> usize {
    let hidden = lines.len().saturating_sub(TOOL_RESULT_LINES);
    for line in lines.iter().take(TOOL_RESULT_LINES) {
        let (text, _) = clip_text(line, TOOL_RESULT_LINE_CHARS);
        entry.push_line(text);
    }
    hidden
}

/// The clock, as milliseconds since the epoch.
///
/// One place asks for the time, so every count in the interface counts from the
/// same clock: two numbers taken a moment apart read as two different waits.
fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
}
