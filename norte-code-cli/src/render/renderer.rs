//! The event renderer: streamed text, tool activity, usage and the footer.

use std::sync::Arc;

use crate::input::{Pinned, TodoItem};

use super::argument::todo_items;
use super::dial::{context_dial, token_ratio};
use super::edit::{
    EditBox, EditPreview, Side, clip_command, clip_plain, edit_box_width, edit_preview, line_range,
    preview_path, render_panel,
};
use super::helpers::{cache_percent_label, human_tokens};
use super::labels::{
    budget_action_name, budget_label, code_name, format_measure, scope_name, short, unit_name,
};
use super::loading::LoadingBar;
use super::{
    BTreeMap, BudgetAction, BudgetScope, BudgetUnit, Delta, Phase, ServerEvent, ServerEventKind,
    Style, Write, clip_text, compact_json, duration_label,
};

/// How events are written.
#[derive(Clone, Copy)]
pub(crate) enum Format {
    /// Streamed text on stdout, diagnostics on stderr.
    Text,
    /// One JSON event per line on stdout.
    Jsonl,
}

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
    pub(crate) debug: bool,
    pub(crate) debug_session: bool,
    /// The session key, for the footer. The server sends it on every event.
    pub(crate) session: Option<String>,
    pub(crate) show_cost: bool,
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
    /// The path a `write` or `read` is working on, by call, until it finishes.
    pending_paths: BTreeMap<String, String>,
    pub(crate) model_name: Option<String>,
    /// Animated loading bar shown while working.
    pub(crate) loading_bar: Option<LoadingBar>,
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
}

impl Renderer {
    /// A renderer with empty counters for a new run.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    pub(crate) fn new(
        format: Format,
        verbosity: Verbosity,
        debug: bool,
        show_cost: bool,
        style: Style,
        pinned: Arc<Pinned>,
    ) -> Self {
        Self {
            format,
            verbosity,
            debug,
            debug_session: false,
            session: None,
            show_cost,
            cost_silent: false,
            style,
            pinned,
            block: Block::None,
            reasoning_line_start: false,
            reasoning_placeholder: false,
            last_char: None,
            answer_buffer: String::new(),
            pending_paths: BTreeMap::new(),
            model_name: None,
            loading_bar: None,
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
        }
    }

    pub(crate) fn event(&mut self, event: &ServerEvent) -> Result<(), String> {
        self.session = event.session.as_ref().map(ToString::to_string);
        if self.debug && !self.debug_session {
            if let Some(session) = &event.session {
                // The loading bar owns a line on stderr; a diagnostic that lands
                // on it would interleave with the animation, so the bar stops
                // first, like every other writer on that stream.
                if let Some(mut bar) = self.loading_bar.take() {
                    bar.stop();
                }
                eprintln!("{}", self.style.dim(format!("debug: session {session}")));
                self.debug_session = true;
            }
        }
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
            ServerEventKind::Delta { delta, .. } => self.delta(delta),
            ServerEventKind::ToolStarted { .. } | ServerEventKind::ToolFinished { .. } => {
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
            kind => self.status_event(kind),
        }
        Ok(())
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
        self.loading_bar = Some(LoadingBar::start(self.style));
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
    pub(crate) fn end_reasoning(&mut self) {
        if self.block != Block::Reasoning {
            return;
        }
        if !self.reasoning_line_start {
            eprintln!();
        }
        eprintln!("  {}", self.style.faint("╰"));
        self.block = Block::None;
        self.last_char = Some('\n');
    }

    /// Finishes any streamed block and leaves one blank line.
    pub(crate) fn end_blocks(&mut self) {
        self.reasoning_placeholder = false;
        if let Some(mut bar) = self.loading_bar.take() {
            bar.stop();
        }
        self.end_reasoning();
        if self.block == Block::Answer {
            if !self.answer_buffer.is_empty() && self.verbosity.at_least(Verbosity::Medium) {
                let rendered = crate::markdown::render(&self.answer_buffer);
                print!("{rendered}");
                if !rendered.ends_with('\n') {
                    println!();
                }
            }
            self.answer_buffer.clear();
            if self.last_char != Some('\n') {
                println!();
            }
        }
        if self.block != Block::None {
            self.block = Block::None;
            self.last_char = Some('\n');
        }
        if self.verbosity.at_least(Verbosity::Medium) && self.last_char.is_some() {
            eprintln!();
        }
    }

    /// Prints a single blank separator line.
    pub(crate) fn space(&mut self) {
        if !self.verbosity.at_least(Verbosity::Medium) {
            return;
        }
        if self.last_char != Some('\n') {
            println!();
        }
        eprintln!();
        self.last_char = Some('\n');
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
        if let Some(mut bar) = self.loading_bar.take() {
            bar.stop();
        }
        self.space();
        match role {
            "user" => {
                let mut lines = text.lines();
                let first = lines.next().unwrap_or_default();
                eprintln!("{} {}", self.style.cyan("›"), first);
                for line in lines {
                    eprintln!("  {}", crate::text::terminal(line));
                }
            }
            "assistant" => {
                let rendered = crate::markdown::render(text);
                eprint!("{rendered}");
                if !rendered.ends_with('\n') {
                    eprintln!();
                }
            }
            other => {
                let label = self.style.faint(format!("#{index} {other}"));
                eprintln!("{label}");
                for line in text.lines() {
                    eprintln!("  {}", self.style.dim(line));
                }
            }
        }
        self.last_char = Some('\n');
    }

    /// Writes reasoning into its own dim gutter.
    pub(crate) fn reasoning_delta(&mut self, text: &str) {
        let mut rest = text;
        while !rest.is_empty() {
            if self.reasoning_line_start {
                eprint!("  {} ", self.style.faint("│"));
                self.reasoning_line_start = false;
            }
            if let Some(index) = rest.find('\n') {
                let (line, tail) = rest.split_at(index);
                eprint!("{}", self.style.thought(line));
                eprintln!();
                self.last_char = Some('\n');
                self.reasoning_line_start = true;
                rest = &tail[1..];
            } else {
                eprint!("{}", self.style.thought(rest));
                self.last_char = rest.chars().last();
                rest = "";
            }
        }
        let _ = std::io::stderr().flush();
    }

    /// Writes answer text, closing reasoning and separating it first.
    pub(crate) fn answer_delta(&mut self, text: &str) {
        let text = crate::text::terminal(text);
        if !self.verbosity.at_least(Verbosity::Medium) {
            print!("{text}");
            if let Some(last) = text.chars().last() {
                self.last_char = Some(last);
            }
            let _ = std::io::stdout().flush();
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
        if let Some(mut bar) = self.loading_bar.take() {
            bar.stop();
        }
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
        self.answer_buffer.push_str(&text);
        if let Some(last) = text.chars().last() {
            self.last_char = Some(last);
        }
    }

    /// Renders a fenced block, so verbose tool activity stays readable and can
    /// be copied into a markdown report.
    pub(crate) fn code_block(&self, language: &str, text: &str) {
        eprintln!("  {}", self.style.faint(format!("```{language}")));
        let text = crate::text::terminal(text);
        if text.is_empty() {
            eprintln!("  {}", self.style.faint("(empty)"));
        } else {
            for line in text.lines() {
                eprintln!("  {line}");
            }
        }
        eprintln!("  {}", self.style.faint("```"));
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
                    eprintln!(
                        "{} {} {}",
                        self.style.blue("→"),
                        self.style.tool(tool),
                        self.style.faint(format!("#{}", short(call_id)))
                    );
                    if let Some(command) = &command {
                        eprintln!("  {}", self.style.dim(format!("$ {command}")));
                    }
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
                                eprintln!(
                                    "  {}",
                                    self.style.yellow("arguments shortened for display")
                                );
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
                truncated,
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
                                eprintln!("  {text}");
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
                if self.verbosity.at_least(Verbosity::Medium) {
                    if *is_error {
                        eprintln!(
                            "{} {} {}",
                            self.style.red("✗"),
                            self.style.bold(tool),
                            self.style.dim(format!("failed in {duration_ms} ms"))
                        );
                    } else {
                        eprintln!(
                            "{} {} {}",
                            self.style.green("✓"),
                            self.style.bold(tool),
                            self.style.dim(format!("in {duration_ms} ms"))
                        );
                    }
                }
                if *is_error && tool.as_str() != "edit" {
                    // A failure needs its reason whatever the verbosity.
                    if let Some(error) = error {
                        self.print_tool_result(error, *truncated);
                    }
                } else if let Some(result) =
                    Self::tool_result(tool, path.as_deref(), output.as_deref())
                {
                    self.print_tool_result(&result, *truncated);
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
            _ => {}
        }
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
        if tool == "command" {
            return output.map(str::to_owned);
        }
        None
    }

    /// Prints a tool result as a short, dimmed, indented block.
    ///
    /// Bounded on purpose: the model may have received far more than a person
    /// wants to read, so each section shows its first lines and a count replaces
    /// the rest. `stdout` and `stderr` are budgeted apart, because a long stdout
    /// would otherwise hide the line of stderr that explains a failure.
    fn print_tool_result(&self, text: &str, truncated: bool) {
        let escaped = crate::text::terminal(text);
        let sections = tool_sections(&escaped);
        let labelled = sections
            .iter()
            .filter(|section| section.name.is_some())
            .count()
            > 1;
        let mut first = true;
        for section in &sections {
            if !first {
                eprintln!();
            }
            first = false;
            if labelled {
                if let Some(name) = &section.name {
                    eprintln!("  {}", self.style.faint(name.clone()));
                }
            }
            let hidden = print_tool_lines(self.style, &section.lines);
            if hidden > 0 {
                eprintln!(
                    "  {}",
                    self.style.yellow(format!("… {hidden} more line(s)"))
                );
            }
        }
        if truncated {
            eprintln!(
                "  {}",
                self.style.yellow("(the text above is what the model saw)")
            );
        }
    }

    /// Prints each edit as its own box: the original and resulting lines side by
    /// side, with line numbers and a little surrounding context. This is
    /// deliberately independent of `--quiet` and `--verbose`: changing a file is
    /// the one tool effect a human needs to see.
    fn print_edit_diff(&self, preview: &EditPreview) {
        let width = edit_box_width();
        for (index, edit) in preview.boxes.iter().enumerate() {
            if index > 0 {
                eprintln!();
            }
            self.print_edit_box(&preview.path, edit, width);
        }
    }

    /// Renders one boxed edit. The two panels are sized independently so the
    /// right border stays aligned on odd terminal widths.
    #[allow(clippy::too_many_lines)]
    fn print_edit_box(&self, path: &str, edit: &EditBox, width: usize) {
        let panel_left = width.saturating_sub(7) / 2;
        let panel_right = width.saturating_sub(7).saturating_sub(panel_left);
        if panel_left < 10 || panel_right < 10 {
            return;
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

        let range = match (edit.old_range, edit.new_range) {
            (Some((start, end)), Some((new_start, new_end)))
                if (start, end) == (new_start, new_end) =>
            {
                line_range(start, end)
            }
            (Some((start, end)), Some((new_start, new_end))) => {
                format!(
                    "{} → {}",
                    line_range(start, end),
                    line_range(new_start, new_end)
                )
            }
            (Some((start, end)), None) => format!("{} → deleted", line_range(start, end)),
            (None, _) => "location unavailable".to_owned(),
        };
        let approximate = if edit.approximate {
            " · approximate"
        } else {
            ""
        };
        let mut title = format!(
            " edit {}/{} · {range}{approximate} · {}",
            edit.index,
            edit.total,
            preview_path(path)
        );
        title.push(' ');
        let title = clip_plain(&title, width.saturating_sub(3));
        let fill = width.saturating_sub(title.chars().count() + 3);
        eprintln!(
            "  {}{}{}{}",
            self.style.blue("╭─"),
            self.style.bold(title),
            self.style.blue("─".repeat(fill)),
            self.style.blue("╮")
        );

        let left_label = format!("{:<panel_left$}", clip_plain(" original", panel_left));
        let right_label = format!("{:<panel_right$}", clip_plain(" result", panel_right));
        eprintln!(
            "  {} {} {} {} {}",
            self.style.blue("│"),
            self.style.cyan(left_label),
            self.style.blue("│"),
            self.style.cyan(right_label),
            self.style.blue("│")
        );
        let left_segment = "─".repeat(panel_left + 2);
        let right_segment = "─".repeat(panel_right + 2);
        eprintln!(
            "  {}{}{}{}{}",
            self.style.blue("├"),
            self.style.blue(left_segment),
            self.style.blue("┼"),
            self.style.blue(right_segment),
            self.style.blue("┤")
        );

        for row in &edit.rows {
            let left = render_panel(
                row.left.as_ref(),
                Side::Left,
                panel_left,
                digits,
                self.style,
            );
            let right = render_panel(
                row.right.as_ref(),
                Side::Right,
                panel_right,
                digits,
                self.style,
            );
            eprintln!(
                "  {} {} {} {} {}",
                self.style.blue("│"),
                left,
                self.style.blue("│"),
                right,
                self.style.blue("│")
            );
        }
        if edit.omitted > 0 {
            let note = format!("… {} more line(s) …", edit.omitted);
            let inner = width.saturating_sub(4);
            let note = format!("{:<inner$}", clip_plain(&note, inner));
            eprintln!(
                "  {} {} {}",
                self.style.blue("│"),
                self.style.yellow(note),
                self.style.blue("│")
            );
        }
        eprintln!(
            "  {}{}{}",
            self.style.blue("╰"),
            self.style.blue("─".repeat(width.saturating_sub(2))),
            self.style.blue("╯")
        );
    }

    /// Returns the command a `command` tool call ran, clipped to one line.
    fn command_summary(arguments: Option<&serde_json::Value>) -> Option<String> {
        let arguments = arguments?;
        let command = arguments.get("command")?;
        let text = if let Some(text) = command.as_str() {
            text.to_owned()
        } else {
            match command.get("kind").and_then(serde_json::Value::as_str) {
                Some("shell") => command
                    .get("script")
                    .and_then(serde_json::Value::as_str)?
                    .to_owned(),
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
                    if args.is_empty() {
                        program.to_owned()
                    } else {
                        format!("{program} {args}")
                    }
                }
                _ => return None,
            }
        };
        Some(clip_command(&text, 160))
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
                context_used,
                context_window,
            } => {
                self.input = *input_tokens;
                self.output = *output_tokens;
                self.cost = *cost_usd;
                self.requests = *requests;
                self.turn_requests = 0;
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
                if self.block != Block::None {
                    self.end_blocks();
                }
                let mut parts = Vec::new();
                if let Some(name) = &self.model_name {
                    parts.push(self.style.dim(name.clone()));
                }
                if let Some(input) = usage.input_tokens {
                    parts.push(self.style.dim(format!("{} in", human_tokens(input))));
                }
                if let Some(output) = usage.output_tokens {
                    parts.push(self.style.dim(format!("{} out", human_tokens(output))));
                }
                if let Some(rate) = tokens_per_second {
                    parts.push(self.style.dim(format!("{rate:.1} tok/s")));
                }
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
                if let (Some(cache), Some(input)) = (usage.cache_read_tokens, usage.input_tokens) {
                    parts.push(
                        self.style
                            .dim(format!("cache {}", cache_percent_label(cache, input))),
                    );
                } else if *cache_hit {
                    parts.push(self.style.dim("cache hit".to_owned()));
                }
                if let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) {
                    parts.push(
                        self.style
                            .dim(format!("in:out {}", token_ratio(input, output))),
                    );
                }
                if !tools.is_empty() {
                    parts.push(self.style.dim(format!("tools {}", Self::tool_list(tools))));
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
                eprintln!(
                    "  {} {}",
                    self.style.blue("↳"),
                    parts.join(&self.style.faint(" · "))
                );
                if *truncated {
                    eprintln!(
                        "  {}",
                        self.style
                            .yellow("warning: the answer hit the output limit")
                    );
                }
            }
            ServerEventKind::CostNotReported { requests }
                if self.verbosity.at_least(Verbosity::Medium) && self.show_cost =>
            {
                eprintln!(
                    "  {}",
                    self.style.yellow(format!(
                        "warning: {requests} request(s) carried no cost; the total is incomplete"
                    ))
                );
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
                    eprintln!(
                        "{} {}",
                        self.style.dim("compacting context…"),
                        self.style.faint(format!("({reason})"))
                    );
                }
                self.set_activity(Some("compacting context…".to_owned()));
            }
            ServerEventKind::CompactionFinished {
                compacted,
                north_session,
                error,
            } => {
                self.end_blocks();
                if *compacted {
                    let detail = if self.verbosity.is_quiet() {
                        String::new()
                    } else {
                        north_session
                            .as_ref()
                            .map_or_else(String::new, |id| format!(" · new session {}", short(id)))
                    };
                    eprintln!(
                        "{}{}",
                        self.style.green("✓ context compacted"),
                        self.style.dim(detail)
                    );
                } else {
                    let detail = error
                        .as_ref()
                        .map_or_else(String::new, |reason| format!(": {reason}"));
                    eprintln!(
                        "{}{}",
                        self.style
                            .yellow("compaction did not happen; history is unchanged"),
                        self.style.dim(detail)
                    );
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
                    eprintln!(
                        "{} {} {}",
                        self.style.yellow("retrying"),
                        self.style.dim(format!("in {wait} (attempt {attempt}):")),
                        self.style.faint(reason)
                    );
                }
            }
            ServerEventKind::RetryStarted { attempt, .. } => {
                self.end_blocks();
                self.set_activity(None);
                if self.verbosity.at_least(Verbosity::Medium) {
                    eprintln!(
                        "{}",
                        self.style.dim(format!("retrying now (attempt {attempt})"))
                    );
                }
            }
            ServerEventKind::Error { code, message } => {
                self.end_blocks();
                eprintln!(
                    "{} {} {}",
                    self.style.red("error:"),
                    self.style.bold(message),
                    self.style.dim(format!("({})", code_name(*code)))
                );
                if !self.cancelled {
                    self.failed = true;
                }
            }
            ServerEventKind::ApprovalResolved { approved, .. } => {
                if self.verbosity.at_least(Verbosity::Medium) {
                    self.end_blocks();
                    if *approved {
                        eprintln!("{}", self.style.green("✓ approval granted"));
                    } else {
                        eprintln!("{}", self.style.red("✗ approval denied"));
                    }
                }
            }
            ServerEventKind::TurnFinished {
                committed, phase, ..
            } => {
                self.end_blocks();
                if !*committed && self.cancelled {
                    if self.verbosity.at_least(Verbosity::Medium) {
                        eprintln!(
                            "{}",
                            self.style
                                .yellow("turn cancelled · the session is saved and can be resumed")
                        );
                        if self.show_cost && self.turn_requests == 0 {
                            eprintln!(
                                "  {}",
                                self.style.dim(
                                    "no provider usage arrived before the cancel, so no cost is \
                                     shown; the cancelled request is still recorded in telemetry"
                                )
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
                        eprintln!("{}", self.style.yellow(message));
                    }
                } else if !*committed {
                    if let Some(reason) = self.stopped.take() {
                        // The host ended the turn on purpose; that is not a
                        // failure.
                        eprintln!(
                            "{}",
                            self.style.yellow(format!("the turn was stopped: {reason}"))
                        );
                    } else {
                        eprintln!("{}", self.style.red("the turn did not commit"));
                        self.failed = true;
                    }
                } else if *phase == Phase::AwaitingApproval
                    && self.verbosity.at_least(Verbosity::Medium)
                {
                    eprintln!("{}", self.style.yellow("paused: waiting for approval"));
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
                    eprintln!(
                        "{} {}",
                        self.style.yellow("budget warning:"),
                        self.style.dim(format!(
                            "{} {} of {} {}",
                            scope_name(*scope),
                            format_measure(*unit, *spent),
                            format_measure(*unit, *limit),
                            unit_name(*unit)
                        ))
                    );
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
                eprintln!(
                    "{} {}",
                    self.style.red("budget exceeded:"),
                    self.style.dim(format!(
                        "{} {} of {} {} ({})",
                        scope_name(*scope),
                        format_measure(*unit, *spent),
                        format_measure(*unit, *limit),
                        unit_name(*unit),
                        budget_action_name(*action)
                    ))
                );
            }
            ServerEventKind::BudgetChanged { budget } => {
                // Raising or clearing the budget makes a stopped session
                // resumable again; the next turn must not look stopped.
                self.budget_stopped = false;
                self.budget_prompt = None;
                if self.verbosity.at_least(Verbosity::Medium) {
                    match budget {
                        Some(budget) => eprintln!(
                            "{} {}",
                            self.style.dim("budget set:"),
                            self.style.dim(budget_label(budget))
                        ),
                        None => eprintln!("{}", self.style.dim("budget cleared")),
                    }
                }
            }
            ServerEventKind::ModelQueued { model }
                if self.verbosity.at_least(Verbosity::Medium) =>
            {
                eprintln!(
                    "{} {}",
                    self.style.dim("model switch queued:"),
                    self.style.dim(model)
                );
            }
            ServerEventKind::ModelChanged { model }
                if self.verbosity.at_least(Verbosity::Medium) =>
            {
                eprintln!("{} {}", self.style.dim("model set:"), self.style.dim(model));
            }
            ServerEventKind::TurnStopped { reason } => {
                self.stopped = Some(reason.clone());
                if self.verbosity.at_least(Verbosity::Medium) {
                    eprintln!(
                        "{}",
                        self.style.yellow(format!("the turn was stopped: {reason}"))
                    );
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
                if self.verbosity.is_quiet() {
                    // `low` is the answer alone: not even the fact that the
                    // model is thinking is shown.
                } else if !self.verbosity.at_least(Verbosity::High) {
                    // `medium` says that it is thinking, without the thought.
                    if !self.reasoning_placeholder {
                        self.reasoning_placeholder = true;
                        eprintln!("  {}", self.style.dim("thinking…"));
                    }
                } else {
                    // The bar and the gutter share stderr, so the first reasoning
                    // token clears the bar before the gutter writes over it.
                    if let Some(mut bar) = self.loading_bar.take() {
                        bar.stop();
                    }
                    if self.block != Block::Reasoning {
                        if self.block == Block::Answer {
                            self.end_blocks();
                        } else if self.block == Block::None {
                            self.space();
                        }
                        eprintln!("  {} {}", self.style.faint("┌"), self.style.dim("thinking"));
                        self.block = Block::Reasoning;
                        self.reasoning_line_start = true;
                    }
                    self.reasoning_delta(text);
                }
            }
            Delta::Refusal { text, .. } => {
                self.end_blocks();
                eprintln!("{}", self.style.red(format!("refusal: {text}")));
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
    /// place: requests, input and output tokens, the average cache-read share,
    /// the total input:output ratio and the reported cost.
    pub(crate) fn session_status(&self) -> String {
        // The dial shows the live context against the model's window, so a turn
        // that is filling the window is visible before compaction hits. It is red
        // while a compaction is running.
        let compacting = self
            .activity
            .as_deref()
            .is_some_and(|activity| activity.starts_with("compacting"));
        let mut parts = vec![
            self.style.cyan(format!("{} req", self.requests)),
            self.style.dim(format!("{} in", human_tokens(self.input))),
            self.style.dim(format!("{} out", human_tokens(self.output))),
            context_dial(
                self.style,
                self.context_used,
                self.max_context,
                self.compaction_threshold,
                compacting,
            ),
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
        let prefix = self.activity.as_ref().map_or_else(
            || self.style.faint("session"),
            |activity| self.style.yellow(activity.clone()),
        );
        // The key, shortened, so a session can be named again without scrolling
        // back to the startup block it was printed in.
        if self.verbosity.at_least(Verbosity::Medium) {
            if let Some(key) = &self.session {
                parts.insert(0, self.style.faint(short(key).to_owned()));
            }
        }
        format!("{} {}", prefix, parts.join(&self.style.faint(" · ")))
    }

    /// Writes the run's cost and tool summary, if the run made any requests.
    pub(crate) fn summary(&self) {
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
        eprintln!();
        eprintln!(
            "  {} {}",
            self.style.faint("──"),
            parts.join(&self.style.faint(" · "))
        );
        if !self.tool_counts.is_empty() {
            eprintln!("  {} {}", self.style.faint("tools"), self.tool_summary());
        }
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

/// Prints up to [`TOOL_RESULT_LINES`] of a section, returning how many it hid.
fn print_tool_lines(style: Style, lines: &[String]) -> usize {
    let mut hidden = lines.len().saturating_sub(TOOL_RESULT_LINES);
    for line in lines.iter().take(TOOL_RESULT_LINES) {
        let (line, clipped) = clip_text(line, TOOL_RESULT_LINE_CHARS);
        eprintln!("  {}", style.dim(line));
        if clipped {
            hidden += 1;
        }
    }
    hidden
}
