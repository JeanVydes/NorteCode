//! Output formatting: event renderer, debug views and small label helpers.

use norte_code_agent::DEFAULT_SYSTEM_PROMPT;

use crate::input::{Pinned, TodoItem, TodoStatus};

use super::{
    ApprovalMode, BTreeMap, Budget, BudgetAction, BudgetScope, BudgetUnit, CacheMode, Config,
    Delta, ErrorCode, Feature, Modality, ModelSpec, Path, PathBuf, Phase, Reasoning,
    ReasoningEffort, ReasoningInfo, ServerEvent, ServerEventKind, Service, Source,
    StreamingGranularity, StructuredOutputMode, Style, ToolKind, Write, clip_text, compact_json,
    duration_label,
};
use std::io::Read;
use std::sync::Arc;

/// How events are written.
#[derive(Clone, Copy)]
pub(crate) enum Format {
    /// Streamed text on stdout, diagnostics on stderr.
    Text,
    /// One JSON event per line on stdout.
    Jsonl,
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
    pub(crate) quiet: bool,
    pub(crate) verbose: bool,
    pub(crate) debug: bool,
    pub(crate) debug_session: bool,
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
        quiet: bool,
        verbose: bool,
        debug: bool,
        show_cost: bool,
        style: Style,
        pinned: Arc<Pinned>,
    ) -> Self {
        Self {
            format,
            quiet,
            verbose,
            debug,
            debug_session: false,
            show_cost,
            cost_silent: false,
            style,
            pinned,
            block: Block::None,
            reasoning_line_start: false,
            reasoning_placeholder: false,
            last_char: None,
            answer_buffer: String::new(),
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
        if self.quiet || matches!(self.format, Format::Jsonl) {
            return;
        }
        self.loading_bar = Some(LoadingBar::start(self.style));
    }

    /// Replaces the activity label and redraws the pinned footer.
    fn set_activity(&mut self, activity: Option<String>) {
        if self.quiet || matches!(self.format, Format::Jsonl) {
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
            if !self.answer_buffer.is_empty() && !self.quiet {
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
        if !self.quiet && self.last_char.is_some() {
            eprintln!();
        }
    }

    /// Prints a single blank separator line.
    pub(crate) fn space(&mut self) {
        if self.quiet {
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
        if self.quiet || matches!(self.format, Format::Jsonl) {
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
        if self.quiet {
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
                let command = Self::command_summary(arguments.as_ref());
                let diff = if tool == "edit" {
                    edit_preview(arguments.as_ref(), source.as_deref())
                } else {
                    None
                };
                if self.quiet {
                    if tool == "command" {
                        if let Some(command) = &command {
                            self.end_blocks();
                            eprintln!(
                                "{} {} {}",
                                self.style.blue("→"),
                                self.style.tool(tool),
                                self.style.dim(format!("$ {command}"))
                            );
                        }
                    }
                    if let Some(diff) = &diff {
                        self.end_blocks();
                        eprintln!("{} {}", self.style.blue("→"), self.style.tool(tool));
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
                    if self.verbose && tool.as_str() != "ask_user" && tool.as_str() != "edit" {
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
                if *is_error && tool == "edit" {
                    if let Some(error) = error {
                        self.end_blocks();
                        if self.quiet {
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
                if !self.quiet {
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
                    if self.verbose {
                        if *is_error {
                            if tool.as_str() != "edit" {
                                if let Some(error) = error {
                                    self.code_block("text", error);
                                }
                            }
                        } else if let Some(output) = output {
                            self.code_block("text", output);
                            if *truncated {
                                eprintln!(
                                    "  {}",
                                    self.style.yellow("output shortened for display")
                                );
                            }
                        }
                    }
                }
            }
            _ => {}
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
                if !self.quiet && matches!(self.format, Format::Text) {
                    let status = self.session_status();
                    self.pinned.set_session(status);
                }
                if self.quiet {
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
                            .dim(format!("cache {}", percent_label(cache, input))),
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
            ServerEventKind::CostNotReported { requests } if !self.quiet && self.show_cost => {
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
                if !self.quiet {
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
                    let detail = if self.quiet {
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
                if !self.quiet {
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
                if !self.quiet {
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
                if !self.quiet {
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
                    if !self.quiet {
                        eprintln!(
                            "{}",
                            self.style
                                .yellow("turn cancelled · the session is saved and can be resumed")
                        );
                        if self.show_cost && self.turn_requests == 0 {
                            eprintln!(
                                "  {}",
                                self.style.dim(
                                    "no provider usage arrived before the cancel, so no cost is shown; the cancelled request is still recorded in telemetry"
                                )
                            );
                        }
                    }
                } else if !*committed && self.budget_stopped {
                    if !self.quiet {
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
                } else if *phase == Phase::AwaitingApproval && !self.quiet {
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
                if !self.quiet {
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
                if !self.quiet {
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
            ServerEventKind::ModelQueued { model } if !self.quiet => {
                eprintln!(
                    "{} {}",
                    self.style.dim("model switch queued:"),
                    self.style.dim(model)
                );
            }
            ServerEventKind::ModelChanged { model } if !self.quiet => {
                eprintln!("{} {}", self.style.dim("model set:"), self.style.dim(model));
            }
            ServerEventKind::TurnStopped { reason } => {
                self.stopped = Some(reason.clone());
                if !self.quiet {
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
                if self.quiet {
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
                percent_label(self.cache_read, self.input)
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
        format!("{} {}", prefix, parts.join(&self.style.faint(" · ")))
    }

    /// Writes the run's cost and tool summary, if the run made any requests.
    pub(crate) fn summary(&self) {
        if matches!(self.format, Format::Jsonl) || self.quiet || self.requests == 0 {
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
                percent_label(self.cache_read, self.input)
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

/// A search/replace edit rendered as a side-by-side, boxed preview.
#[derive(Debug)]
struct EditPreview {
    path: String,
    boxes: Vec<EditBox>,
}

/// One edit's box: the lines it touched plus surrounding context.
#[derive(Debug)]
struct EditBox {
    index: usize,
    total: usize,
    old_range: Option<(usize, usize)>,
    new_range: Option<(usize, usize)>,
    approximate: bool,
    rows: Vec<DiffRow>,
    omitted: usize,
}

/// One row of the side-by-side view. A missing side is a deletion or insertion.
#[derive(Debug)]
struct DiffRow {
    left: Option<Cell>,
    right: Option<Cell>,
}

/// One numbered line in a panel.
#[derive(Debug)]
struct Cell {
    number: Option<usize>,
    text: String,
    changed: bool,
}

/// Which panel a cell belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// The file before the edit.
    Left,
    /// The file after the edit.
    Right,
}

/// Lines of context kept before and after an edit.
const EDIT_CONTEXT_LINES: usize = 2;
/// Rows kept per box, so a huge search block cannot flood the terminal.
const EDIT_MAX_ROWS: usize = 40;
/// The largest file read for a preview. The edit tool itself is bounded too.
const EDIT_MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Builds the preview shown for an `edit` call from its search/replace blocks.
///
/// The tool's structured result records where an edit landed, not the text it
/// replaced, so the request and the file before the edit are the only sources.
/// The host captures the file in the tool metadata callback, immediately before
/// the executor runs; reading it here would race with a fast edit.
fn edit_preview(
    arguments: Option<&serde_json::Value>,
    source: Option<&str>,
) -> Option<EditPreview> {
    let arguments = arguments?;
    let path = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("(unknown path)")
        .to_owned();
    let edits = arguments.get("edits")?.as_array()?;
    if edits.is_empty() {
        return None;
    }
    let source = source
        .map(str::to_owned)
        .or_else(|| read_edit_source(&path));
    Some(build_edit_preview(path, edits, source))
}

fn build_edit_preview(
    path: String,
    edits: &[serde_json::Value],
    source: Option<String>,
) -> EditPreview {
    let mut working = source;
    let total = edits.len();
    let mut boxes = Vec::with_capacity(total);
    for (index, edit) in edits.iter().enumerate() {
        let search = edit
            .get("search")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let replace = edit
            .get("replace")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let changed = search != replace;
        let built = working.as_mut().and_then(|source| {
            let found = find_edit(source, search)?;
            let after = if found.approximate {
                // The whitespace fallback matched whole lines, including the
                // last newline. Keep the block boundary intact when the
                // replacement does not carry its own trailing newline.
                let matched = &source[found.start..found.end];
                let mut replacement = replace.to_owned();
                if matched.ends_with('\n') && !replacement.ends_with('\n') {
                    replacement.push('\n');
                } else if !matched.ends_with('\n') && replacement.ends_with('\n') {
                    replacement.pop();
                }
                format!(
                    "{}{}{}",
                    &source[..found.start],
                    replacement,
                    &source[found.end..]
                )
            } else {
                format!(
                    "{}{}{}",
                    &source[..found.start],
                    replace,
                    &source[found.end..]
                )
            };
            let built = build_edit_box(
                source,
                &after,
                found.start,
                found.end,
                replace,
                found.approximate,
                changed,
            );
            if !found.approximate {
                *source = after;
            }
            Some(built)
        });
        let (rows, old_range, new_range, approximate, omitted) =
            built.unwrap_or_else(|| fallback_edit_box(search, replace, changed));
        boxes.push(EditBox {
            index: index + 1,
            total,
            rows,
            old_range,
            new_range,
            approximate,
            omitted,
        });
    }
    EditPreview { path, boxes }
}

#[derive(Clone, Copy, Debug)]
struct Found {
    start: usize,
    end: usize,
    approximate: bool,
}

/// Finds the block an edit applies to. Exact matches win; a whitespace-only
/// mismatch is still located for the context preview, but not simulated.
fn find_edit(source: &str, search: &str) -> Option<Found> {
    if search.is_empty() {
        return None;
    }
    if let Some(start) = source.find(search) {
        return Some(Found {
            start,
            end: start + search.len(),
            approximate: false,
        });
    }
    let (start, end) = find_line_window(source, search)?;
    Some(Found {
        start,
        end,
        approximate: true,
    })
}

/// Finds a window of whole source lines that matches the search block after
/// trimming each line. This mirrors the edit tool's whitespace-only fallback
/// closely enough for a preview.
fn find_line_window(source: &str, search: &str) -> Option<(usize, usize)> {
    if search.trim().is_empty() {
        return None;
    }
    let needle: Vec<&str> = search.lines().map(str::trim).collect();
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    if needle.is_empty() || needle.len() > lines.len() {
        return None;
    }
    let mut offsets = Vec::with_capacity(lines.len() + 1);
    let mut offset = 0usize;
    for line in &lines {
        offsets.push(offset);
        offset += line.len();
    }
    offsets.push(offset);
    for start in 0..=lines.len() - needle.len() {
        if needle
            .iter()
            .enumerate()
            .all(|(index, needle)| lines[start + index].trim() == *needle)
        {
            return Some((offsets[start], offsets[start + needle.len()]));
        }
    }
    None
}

#[allow(clippy::type_complexity)]
fn build_edit_box(
    source: &str,
    after: &str,
    start: usize,
    end: usize,
    replace: &str,
    approximate: bool,
    changed: bool,
) -> (
    Vec<DiffRow>,
    Option<(usize, usize)>,
    Option<(usize, usize)>,
    bool,
    usize,
) {
    let old_start = line_index(source, start);
    let old_end = line_index(source, end.saturating_sub(1));
    let new_end = if replace.is_empty() {
        old_start.saturating_sub(1)
    } else {
        line_index(after, start + replace.len().saturating_sub(1))
    };
    let old_lines: Vec<&str> = source.split('\n').collect();
    let new_lines: Vec<&str> = after.split('\n').collect();
    let mut rows = Vec::new();

    let context_start = old_start.saturating_sub(EDIT_CONTEXT_LINES);
    for line in context_start..old_start {
        if let (Some(left), Some(right)) = (old_lines.get(line), new_lines.get(line)) {
            rows.push(DiffRow {
                left: Some(cell(line + 1, left, false)),
                right: Some(cell(line + 1, right, false)),
            });
        }
    }

    let old_count = old_end.saturating_sub(old_start) + 1;
    let new_count = if replace.is_empty() {
        0
    } else {
        new_end.saturating_sub(old_start) + 1
    };
    for offset in 0..old_count.max(new_count) {
        let left = if offset < old_count {
            old_lines
                .get(old_start + offset)
                .map(|text| cell(old_start + offset + 1, text, changed))
        } else {
            None
        };
        let right = if offset < new_count {
            new_lines
                .get(old_start + offset)
                .map(|text| cell(old_start + offset + 1, text, changed))
        } else {
            None
        };
        rows.push(DiffRow { left, right });
    }

    let context_end = (old_end + EDIT_CONTEXT_LINES).min(old_lines.len().saturating_sub(1));
    let mut new_context = if replace.is_empty() {
        old_start
    } else {
        new_end.saturating_add(1)
    };
    for line in (old_end + 1)..=context_end {
        let left = old_lines.get(line).map(|text| cell(line + 1, text, false));
        let right = new_lines
            .get(new_context)
            .map(|text| cell(new_context + 1, text, false));
        new_context = new_context.saturating_add(1);
        rows.push(DiffRow { left, right });
    }

    let omitted = rows.len().saturating_sub(EDIT_MAX_ROWS);
    if omitted > 0 {
        rows.truncate(EDIT_MAX_ROWS);
    }
    let new_range = if replace.is_empty() {
        None
    } else {
        Some((old_start + 1, new_end + 1))
    };
    (
        rows,
        Some((old_start + 1, old_end + 1)),
        new_range,
        approximate,
        omitted,
    )
}

#[allow(clippy::type_complexity)]
fn fallback_edit_box(
    search: &str,
    replace: &str,
    changed: bool,
) -> (
    Vec<DiffRow>,
    Option<(usize, usize)>,
    Option<(usize, usize)>,
    bool,
    usize,
) {
    let left: Vec<&str> = if search.is_empty() {
        Vec::new()
    } else {
        search.lines().collect()
    };
    let right: Vec<&str> = if replace.is_empty() {
        Vec::new()
    } else {
        replace.lines().collect()
    };
    let count = left.len().max(right.len()).max(1);
    let mut rows = Vec::new();
    for index in 0..count {
        let left = left.get(index).map(|text| Cell {
            number: None,
            text: display_line(text),
            changed,
        });
        let right = right.get(index).map(|text| Cell {
            number: None,
            text: display_line(text),
            changed,
        });
        rows.push(DiffRow { left, right });
    }
    let omitted = rows.len().saturating_sub(EDIT_MAX_ROWS);
    if omitted > 0 {
        rows.truncate(EDIT_MAX_ROWS);
    }
    (rows, None, None, false, omitted)
}

/// Reads the file an edit will touch, but only when it is a regular file inside
/// the current workspace and no larger than the preview limit. It may see a
/// path the model chose, so it must never read an arbitrary file.
fn read_edit_source(path: &str) -> Option<String> {
    let root = std::env::current_dir().ok()?.canonicalize().ok()?;
    let candidate = Path::new(path);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let candidate = candidate.canonicalize().ok()?;
    if !candidate.starts_with(&root) {
        return None;
    }
    // Check before opening: opening a FIFO can block, and the edit tool would
    // reject it anyway.
    let metadata = std::fs::metadata(&candidate).ok()?;
    if !metadata.is_file() || metadata.len() > EDIT_MAX_FILE_BYTES {
        return None;
    }
    let file = std::fs::File::open(candidate).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > EDIT_MAX_FILE_BYTES {
        return None;
    }
    let mut text = String::new();
    file.take(EDIT_MAX_FILE_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn line_index(text: &str, byte: usize) -> usize {
    text.as_bytes()
        .iter()
        .take(byte)
        .filter(|byte| **byte == b'\n')
        .count()
}

fn cell(number: usize, text: &str, changed: bool) -> Cell {
    Cell {
        number: Some(number),
        text: display_line(text),
        changed,
    }
}

fn display_line(text: &str) -> String {
    text.trim_end_matches('\r').replace('\t', "    ")
}

fn render_panel(
    cell: Option<&Cell>,
    side: Side,
    panel: usize,
    digits: usize,
    style: Style,
) -> String {
    let code_width = panel.saturating_sub(digits + 3).max(1);
    let (number, text, changed) = match cell {
        Some(cell) => (
            cell.number
                .map_or_else(|| " ".repeat(digits), |number| format!("{number:>digits$}")),
            clip_plain(&cell.text, code_width),
            cell.changed,
        ),
        None => (" ".repeat(digits), String::new(), false),
    };
    let sign = match (changed, side) {
        (true, Side::Left) => "-",
        (true, Side::Right) => "+",
        _ => " ",
    };
    let padded = format!("{text:<code_width$}");
    let code = match (changed, side) {
        (true, Side::Left) => style.red(padded),
        (true, Side::Right) => style.green(padded),
        _ => padded,
    };
    let number = style.faint(number);
    let sign = match (changed, side) {
        (true, Side::Left) => style.red(sign),
        (true, Side::Right) => style.green(sign),
        _ => style.faint(sign),
    };
    format!("{number} {sign} {code}")
}

fn clip_plain(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let escaped = crate::text::terminal(text);
    let escaped = escaped.replace('\t', "    ");
    if escaped.chars().count() <= max {
        return escaped;
    }
    let mut out: String = escaped.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn line_range(start: usize, end: usize) -> String {
    if start == end {
        format!("line {start}")
    } else {
        format!("lines {start}–{end}")
    }
}

/// Shows a path relative to the workspace when it is inside it, so the header
/// keeps the line range instead of spending its width on a long absolute path.
fn preview_path(path: &str) -> String {
    let root = std::env::current_dir()
        .ok()
        .and_then(|root| root.canonicalize().ok());
    let shown = Path::new(path).canonicalize().ok();
    let displayed = match (root, shown) {
        (Some(root), Some(shown)) => match shown.strip_prefix(&root) {
            Ok(relative) => relative.to_path_buf(),
            Err(_) => shown,
        },
        _ => PathBuf::from(path),
    };
    displayed.display().to_string().replace(['\n', '\r'], " ")
}

fn edit_box_width() -> usize {
    let columns = crossterm::terminal::size().map_or(100, |(columns, _)| usize::from(columns));
    let available = columns.saturating_sub(2);
    if available < 30 {
        available.max(20)
    } else {
        available.min(140)
    }
}

/// Shortens a command to one display line without splitting a character.
fn clip_command(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or_default().trim();
    let mut shortened: String = first.chars().take(max).collect();
    if first.chars().count() > max {
        shortened.push('…');
    } else if text.lines().count() > 1 {
        shortened.push_str(" …");
    }
    if shortened.is_empty() {
        "(empty)".to_owned()
    } else {
        shortened
    }
}

/// The per-project overlay config path, searched from `start` upwards.
pub(crate) fn find_project_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(".norte-code").join("config.json"))
        .find(|path| path.exists())
}

/// The environment variables a provider is likely to use.
///
/// The catalogue's `env` list is authoritative; this is only for a provider the
/// catalogue does not list.
pub(crate) fn fallback_env(provider: &str) -> Vec<String> {
    let upper: String = provider
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let mut names = vec![format!("{upper}_API_KEY")];
    if matches!(provider, "google" | "gemini") {
        for name in [
            "GOOGLE_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_GENERATIVE_AI_API_KEY",
        ] {
            if !names.iter().any(|existing| existing == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

pub(crate) fn human_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let whole = tokens / 1_000_000;
        let decimal = (tokens % 1_000_000) / 100_000;
        if decimal == 0 {
            format!("{whole}M")
        } else {
            format!("{whole}.{decimal}M")
        }
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// A share of a total, rounded so `0.4%` does not become a bare `0%`.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn percent_label(part: u64, total: u64) -> String {
    if total == 0 {
        return "0%".to_owned();
    }
    let percent = part as f64 / total as f64 * 100.0;
    if percent >= 10.0 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

/// One cell of the context dial, from empty to full.
const DIAL_CELLS: [char; 4] = ['░', '▒', '▓', '█'];

/// How close to the compaction limit the context must be before the dial warns.
const DIAL_WARN_FRACTION: f64 = 0.8;

/// How many cells the context dial draws.
const DIAL_WIDTH: usize = 8;

/// A context-usage dial for the footer.
///
/// It draws a fixed-width ring whose filled share is how much of the model's
/// context window the last request used, with the number beside it. The ring is
/// blue well below the compaction threshold, amber near it, and red at or past
/// it, where `compact` is added. `used` is the last request's input tokens.
///
/// When the model's window is unknown the ring is all-empty and only the raw
/// token count is shown: a missing maximum is not a full context.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub(crate) fn context_dial(
    style: Style,
    used: u64,
    max: Option<u64>,
    threshold: f64,
    compact: bool,
) -> String {
    let Some(max) = max.filter(|max| *max > 0) else {
        return style.dim(format!("ctx {}", human_tokens(used)));
    };
    let fraction = (used as f64 / max as f64).clamp(0.0, 1.0);
    let filled_cells = fraction * DIAL_WIDTH as f64;
    let full = filled_cells as usize;
    let mut ring = String::with_capacity(DIAL_WIDTH);
    for cell in 0..DIAL_WIDTH {
        if cell < full {
            ring.push('█');
        } else if (cell as f64) < filled_cells {
            let partial = ((filled_cells - cell as f64) * 4.0) as usize;
            ring.push(DIAL_CELLS[partial.min(3)]);
        } else {
            ring.push('░');
        }
    }
    // The window is over the limit once the used fraction reaches the threshold.
    let over = threshold > 0.0 && fraction >= threshold;
    let near = !over && threshold > 0.0 && fraction >= threshold * DIAL_WARN_FRACTION;
    let label = format!("ctx {} {}", percent_label(used, max), ring);
    if over || compact {
        format!("{} {}", style.red(label), style.red("⟳ compact"))
    } else if near {
        style.yellow(label)
    } else {
        style.blue(label)
    }
}

/// The input-to-output ratio, the number a cost conversation usually asks
/// about. A turn with no output is unbounded rather than zero.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn token_ratio(input: u64, output: u64) -> String {
    if input == 0 && output == 0 {
        return "–".to_owned();
    }
    if output == 0 {
        return "∞".to_owned();
    }
    let ratio = input as f64 / output as f64;
    if ratio >= 10.0 {
        format!("{ratio:.0}:1")
    } else {
        format!("{ratio:.1}:1")
    }
}

pub(crate) fn reasoning_label(reasoning: &ReasoningInfo) -> String {
    if !reasoning.supported {
        return "-".to_owned();
    }
    if reasoning.efforts.is_empty() {
        return if reasoning.can_disable {
            "auto/off".to_owned()
        } else {
            "auto".to_owned()
        };
    }
    reasoning.efforts.join(",")
}

pub(crate) fn source_label(source: Source) -> &'static str {
    match source {
        Source::Cache | Source::NotModified => "cache",
        Source::Network => "network",
        Source::StaleCache => "stale cache",
        Source::Bundled => "bundled snapshot",
    }
}

/// One `debug:` section title, so every block reads as a heading.
fn debug_heading(style: Style, title: &str) -> String {
    style.cyan(style.bold(title))
}

/// One `label  value` line of a debug block: the label dim and padded, the
/// value in the default colour so it stands out from the chrome around it.
fn debug_field(style: Style, label: &str, value: &str) -> String {
    format!("  {} {value}", style.dim(format!("{label:<10}")))
}

/// Prints the startup facts that make a run or resume reproducible.
#[allow(clippy::too_many_arguments)]
pub(crate) fn debug_start(
    style: Style,
    spec: &ModelSpec,
    configuration: &Config,
    source: Source,
    database: &Path,
    config_path: Option<&Path>,
    session: Option<&str>,
    catalog: &norte_code_catalog::Catalog,
) {
    let workspace =
        std::env::current_dir().map_or_else(|_| "?".to_owned(), |path| path.display().to_string());
    let config_label =
        config_path.map_or_else(|| "(default)".to_owned(), |path| path.display().to_string());
    let iterations = configuration
        .max_iterations
        .map_or_else(|| "unlimited".to_owned(), |limit| limit.to_string());
    let sandbox = if configuration.tools.sandbox.enabled {
        let name = if cfg!(target_os = "linux") {
            "bwrap"
        } else if cfg!(target_os = "macos") {
            "seatbelt"
        } else {
            "unsupported platform"
        };
        format!(
            "enabled · {name} · network {}",
            if configuration.tools.sandbox.network {
                "on"
            } else {
                "off"
            }
        )
    } else {
        "disabled".to_owned()
    };
    eprintln!("{}", debug_heading(style, "debug: startup"));
    eprintln!("{}", style.dim("  ┌──────────────┬──────────────────────────────────────────────┐"));
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ session      │ {}",
            session.unwrap_or("(new; shown on first event)")
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ model        │ {}",
            spec.label()
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ provider     │ {}{}",
            spec.provider,
            if catalog.provider(&spec.provider).is_some() {
                ""
            } else {
                "  (not in catalogue; using config/fallback)"
            }
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ wire         │ {}",
            spec.wire.family()
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ approval     │ {}",
            approval_label(configuration.approval_mode)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ iterations   │ {}",
            iterations
        ))
    );
    let compaction_pct = (configuration.compaction.threshold * 100.0) as u64;
    let compaction_tokens = spec.capabilities.max_context.map(|c| (c as f64 * configuration.compaction.threshold) as u64);
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ compaction   │ {}%{}{}",
            compaction_pct,
            compaction_tokens.map(|t| format!(" (~{} tokens)", t)).unwrap_or_default(),
            if !configuration.compaction.enabled {
                " (disabled)"
            } else if configuration.compaction.trigger == "eager" {
                " (eager)"
            } else {
                " (idle)"
            }
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ sandbox      │ {}",
            sandbox
        ))
    );
    eprintln!("{}", style.dim("  ├──────────────┼──────────────────────────────────────────────┤"));
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ workspace    │ {}",
            workspace
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ config       │ {}",
            config_label
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ database     │ {}",
            database.display()
        ))
    );
    let provider_count = catalog.providers.len();
    let model_count: usize = catalog.providers.values().map(|p| p.models.len()).sum();
    eprintln!(
        "{}",
        style.dim(format!(
            "  │ catalogue    │ {} · {} providers · {} models",
            source_label(source),
            provider_count,
            model_count
        ))
    );
    if !configuration.extensions.is_empty() {
        eprintln!(
            "{}",
            style.dim(format!(
                "  │ extensions   │ {}",
                configuration
                    .extensions
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        );
    }
    eprintln!("{}", style.dim("  └──────────────┴──────────────────────────────────────────────┘"));
}

/// Prints the static prompt size in characters. Tokens are provider-reported,
/// so no estimate is made here.
pub(crate) fn debug_prompt_size(style: Style, service: &Service, source: &str) {
    match service.prompt_size() {
        Ok(size) => {
            eprintln!("{}", debug_heading(style, "debug: prompt material"));
            eprintln!("{}", debug_field(style, "source", source));
            eprintln!(
                "{}",
                debug_field(style, "system", &format!("{} chars", size.system_chars))
            );
            if let Some(path) = &size.project_path {
                eprintln!(
                    "{}",
                    debug_field(
                        style,
                        "project",
                        &format!("{} chars ({})", size.project_chars, path.display())
                    )
                );
            }
            eprintln!(
                "{}",
                debug_field(style, "tools", &format!("{} chars", size.tool_chars))
            );
            eprintln!(
                "{}",
                debug_field(style, "total", &format!("{} chars", size.total_chars()))
            );
        }
        Err(error) => eprintln!(
            "{}",
            style.yellow(format!("debug: could not measure the prompt: {error}"))
        ),
    }
}

/// Prints the selected model's capabilities and catalogue metadata.
#[allow(clippy::too_many_lines)]
pub(crate) fn debug_model(
    style: Style,
    spec: &ModelSpec,
    model: Option<&norte_code_catalog::Model>,
) {
    let capabilities = &spec.capabilities;
    eprintln!("{}", debug_heading(style, "debug: model"));
    eprintln!("{}", debug_field(style, "reference", &spec.label()));
    if let Some(model) = model {
        if !model.name.is_empty() {
            eprintln!("{}", debug_field(style, "name", &model.name));
        }
        if let Some(family) = &model.family {
            eprintln!("{}", debug_field(style, "family", family));
        }
        if let Some(description) = &model.description {
            eprintln!("{}", debug_field(style, "about", description));
        }
    } else {
        eprintln!(
            "{}",
            style.yellow(
                "  catalogue  not found; the capabilities below are the config/fallback ones"
            )
        );
    }
    eprintln!(
        "{}",
        debug_field(
            style,
            "wire",
            &format!("{} · provider {}", spec.wire.family(), spec.provider)
        )
    );
    eprintln!(
        "{}",
        debug_field(style, "context", &token_limit_label(capabilities.max_context))
    );
    eprintln!(
        "{}",
        debug_field(style, "output", &token_limit_label(capabilities.max_output))
    );
    eprintln!(
        "{}",
        debug_field(
            style,
            "modalities",
            &format!(
                "input {} · output {}",
                modality_set(capabilities.modalities.input.keys()),
                modality_set(capabilities.modalities.output.iter())
            )
        )
    );
    eprintln!(
        "{}",
        debug_field(style, "tools", &tool_kind_set(&capabilities.tool_kinds))
    );
    eprintln!(
        "{}",
        debug_field(
            style,
            "reasoning",
            &reasoning_capabilities(&capabilities.reasoning)
        )
    );
    eprintln!(
        "{}",
        debug_field(
            style,
            "streaming",
            &capabilities
                .streaming
                .map_or_else(|| "-".to_owned(), |value| streaming_name(value).to_owned())
        )
    );
    eprintln!(
        "{}",
        debug_field(style, "cache", cache_name(capabilities.cache))
    );
    eprintln!(
        "{}",
        debug_field(style, "features", &feature_set(&capabilities.features))
    );
    eprintln!(
        "{}",
        debug_field(
            style,
            "structured",
            &structured_set(&capabilities.structured_output)
        )
    );
    if let Some(model) = model {
        let mut metadata = Vec::new();
        if let Some(status) = &model.status {
            metadata.push(format!("status {status}"));
        }
        if let Some(knowledge) = &model.knowledge {
            metadata.push(format!("knowledge {knowledge}"));
        }
        if let Some(released) = &model.release_date {
            metadata.push(format!("released {released}"));
        }
        if let Some(updated) = &model.last_updated {
            metadata.push(format!("updated {updated}"));
        }
        if let Some(open_weights) = model.open_weights {
            metadata.push(format!("open weights {}", yes_no(open_weights)));
        }
        if model.attachment {
            metadata.push("attachments".to_owned());
        }
        if let Some(temperature) = model.temperature {
            metadata.push(format!("temperature {}", yes_no(temperature)));
        }
        if !metadata.is_empty() {
            eprintln!(
                "{}",
                style.dim(format!("  catalogue  {}", metadata.join(" · ")))
            );
        }
    }
}

/// Prints the reasoning setting the session is actually using.
pub(crate) fn debug_reasoning(style: Style, reasoning: &Reasoning) {
    eprintln!("{}", debug_heading(style, "debug: reasoning"));
    eprintln!(
        "{}",
        debug_field(style, "effective", &reasoning_setting_label(reasoning))
    );
}

pub(crate) fn reasoning_setting_label(reasoning: &Reasoning) -> String {
    match reasoning {
        Reasoning::Automatic => "auto".to_owned(),
        Reasoning::Disabled => "off".to_owned(),
        Reasoning::Effort { effort } => effort.clone(),
        Reasoning::Budget { tokens } => format!("budget {tokens} tokens"),
    }
}

/// Formats a token limit without guessing an unknown value.
pub(crate) fn token_limit_label(limit: Option<u64>) -> String {
    limit.map_or_else(|| "unknown".to_owned(), |value| format!("{value} tokens"))
}

/// Formats a set of modalities.
pub(crate) fn modality_set<'a>(values: impl Iterator<Item = &'a Modality>) -> String {
    let names: Vec<_> = values.map(|value| modality_name(*value)).collect();
    if names.is_empty() {
        "-".to_owned()
    } else {
        names.join(", ")
    }
}

/// Formats the tool kinds a model accepts.
pub(crate) fn tool_kind_set(values: &std::collections::BTreeSet<ToolKind>) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values
        .iter()
        .map(|kind| tool_kind_name(*kind))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats the independent features a model declares.
pub(crate) fn feature_set(values: &std::collections::BTreeSet<Feature>) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values
        .iter()
        .map(|feature| feature_name(*feature))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats the structured-output modes a model declares.
pub(crate) fn structured_set(values: &std::collections::BTreeSet<StructuredOutputMode>) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values
        .iter()
        .map(|mode| structured_name(*mode))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats a model's reasoning controls.
pub(crate) fn reasoning_capabilities(reasoning: &norte::Reasoning) -> String {
    if !reasoning.supported {
        return "not supported".to_owned();
    }
    let mut parts = Vec::new();
    if reasoning.can_disable {
        parts.push("off allowed".to_owned());
    }
    if reasoning.efforts.is_empty() {
        parts.push("auto only".to_owned());
    } else {
        parts.push(format!(
            "efforts {}",
            reasoning
                .efforts
                .iter()
                .map(|effort| effort_label(*effort))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if reasoning.supports_budget {
        parts.push(reasoning.max_budget.map_or_else(
            || "budget yes".to_owned(),
            |max| format!("budget up to {max}"),
        ));
    }
    parts.join(" · ")
}

/// Lower-case labels for the capability enums. They serialize as snake case;
/// these keep the debug output readable without a serialization dependency.
pub(crate) fn modality_name(modality: Modality) -> &'static str {
    match modality {
        Modality::Text => "text",
        Modality::Image => "image",
        Modality::Audio => "audio",
        Modality::Video => "video",
        Modality::Document => "document",
        _ => "other",
    }
}

pub(crate) fn tool_kind_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Function => "function",
        ToolKind::ProviderExecuted => "provider-executed",
        ToolKind::ProviderSchema => "provider-schema",
        _ => "other",
    }
}

pub(crate) fn feature_name(feature: Feature) -> &'static str {
    match feature {
        Feature::Citations => "citations",
        Feature::ParallelToolCalls => "parallel-tool-calls",
        Feature::ForcedToolChoice => "forced-tool-choice",
        Feature::Batch => "batch",
        Feature::Logprobs => "logprobs",
        _ => "other",
    }
}

pub(crate) fn structured_name(mode: StructuredOutputMode) -> &'static str {
    match mode {
        StructuredOutputMode::JsonObject => "json-object",
        StructuredOutputMode::JsonSchema => "json-schema",
        StructuredOutputMode::StrictSchema => "strict-schema",
        _ => "other",
    }
}

pub(crate) fn streaming_name(mode: StreamingGranularity) -> &'static str {
    match mode {
        StreamingGranularity::Delta => "delta",
        StreamingGranularity::Part => "part",
        StreamingGranularity::Disabled => "disabled",
        _ => "other",
    }
}

pub(crate) fn cache_name(mode: CacheMode) -> &'static str {
    match mode {
        CacheMode::None => "none",
        CacheMode::Automatic => "automatic",
        CacheMode::Explicit => "explicit",
        _ => "other",
    }
}

pub(crate) fn effort_label(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::XHigh => "xhigh",
        ReasoningEffort::Max => "max",
        _ => "other",
    }
}

pub(crate) fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub(crate) fn approval_label(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "ask",
        ApprovalMode::AutoEdit => "auto-edit",
        ApprovalMode::Yolo => "yolo",
    }
}

/// Reads the `todo` tool's arguments into the footer's list.
///
/// The model sends the complete list on every call, so this is a snapshot, not
/// a patch. An argument shape the tool would reject is ignored rather than
/// guessed at.
fn todo_items(arguments: Option<&serde_json::Value>) -> Option<Vec<TodoItem>> {
    let todos = arguments?.get("todos")?.as_array()?;
    let mut items = Vec::with_capacity(todos.len());
    for todo in todos {
        let content = todo.get("content")?.as_str()?;
        let status = match todo.get("status")?.as_str()? {
            "pending" => TodoStatus::Pending,
            "in_progress" => TodoStatus::InProgress,
            "completed" => TodoStatus::Completed,
            _ => return None,
        };
        items.push(TodoItem {
            content: content.to_owned(),
            status,
        });
    }
    Some(items)
}

pub(crate) fn short(call_id: &str) -> &str {
    call_id.get(..8).unwrap_or(call_id)
}

pub(crate) fn scope_name(scope: BudgetScope) -> &'static str {
    match scope {
        BudgetScope::Message => "message",
        BudgetScope::Session => "session",
        BudgetScope::GlobalDaily => "today",
    }
}

pub(crate) fn unit_name(unit: BudgetUnit) -> &'static str {
    match unit {
        BudgetUnit::Usd => "USD",
        BudgetUnit::Tokens => "tokens",
        BudgetUnit::Requests => "requests",
    }
}

pub(crate) fn budget_action_name(action: BudgetAction) -> &'static str {
    match action {
        BudgetAction::Warn => "warn",
        BudgetAction::Ask => "ask",
        BudgetAction::Stop => "stop",
    }
}

pub(crate) fn format_measure(unit: BudgetUnit, value: f64) -> String {
    match unit {
        BudgetUnit::Usd => format!("${value:.6}"),
        BudgetUnit::Tokens | BudgetUnit::Requests => format!("{value:.0}"),
    }
}

pub(crate) fn parse_budget_unit(unit: &str) -> Result<BudgetUnit, String> {
    match unit {
        "usd" => Ok(BudgetUnit::Usd),
        "tokens" => Ok(BudgetUnit::Tokens),
        "requests" => Ok(BudgetUnit::Requests),
        other => Err(format!("unknown budget unit `{other}`")),
    }
}

pub(crate) fn parse_budget_action(action: &str) -> Result<BudgetAction, String> {
    match action {
        "warn" => Ok(BudgetAction::Warn),
        "ask" => Ok(BudgetAction::Ask),
        "stop" => Ok(BudgetAction::Stop),
        other => Err(format!("unknown budget action `{other}`")),
    }
}

/// Selects the system prompt: flag, then config, then the built-in default.
pub(crate) fn select_system_prompt(
    flag: Option<String>,
    configured: Option<String>,
) -> (&'static str, String) {
    if let Some(prompt) = flag {
        ("flag", prompt)
    } else if let Some(prompt) = configured {
        ("config", prompt)
    } else {
        ("default", DEFAULT_SYSTEM_PROMPT.to_owned())
    }
}

/// Parses `--reasoning`. The service still checks the setting against the
/// selected model, so this only catches names no model could accept.
pub(crate) fn parse_reasoning_setting(value: &str) -> Result<Reasoning, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if let Some(tokens) = normalized
        .strip_prefix("budget:")
        .or_else(|| normalized.strip_prefix("budget="))
    {
        let tokens = tokens.parse::<u32>().map_err(|_| {
            format!("`{value}` is not a reasoning budget; use budget:<positive token count>")
        })?;
        if tokens == 0 {
            return Err("the reasoning budget must be greater than zero".to_owned());
        }
        return Ok(Reasoning::Budget { tokens });
    }
    match normalized.as_str() {
        "auto" | "automatic" => Ok(Reasoning::Automatic),
        "off" | "none" | "disabled" => Ok(Reasoning::Disabled),
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
            let effort = match normalized.as_str() {
                "minimal" => "minimal",
                "low" => "low",
                "medium" => "medium",
                "high" => "high",
                "xhigh" => "xhigh",
                _ => "max",
            };
            Ok(Reasoning::Effort {
                effort: effort.to_owned(),
            })
        }
        _ => Err(format!(
            "`{value}` is not a reasoning setting; use auto, off, minimal, low, medium, high, \
             xhigh, max or budget:<tokens>"
        )),
    }
}

pub(crate) fn budget_label(budget: &Budget) -> String {
    format!(
        "{} {} ({})",
        format_measure(budget.unit, budget.limit),
        unit_name(budget.unit),
        budget_action_name(budget.action)
    )
}

pub(crate) fn code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidRequest => "invalid request",
        ErrorCode::NotFound => "not found",
        ErrorCode::Conflict => "conflict",
        ErrorCode::Capability => "capability",
        ErrorCode::Budget => "budget",
        ErrorCode::Provider => "provider",
        ErrorCode::Internal => "internal",
        ErrorCode::Unsupported => "unsupported",
    }
}

/// An animated loading bar printed below the user's message.
///
/// The bar owns the line under the user's message while a turn is in flight: it
/// writes one carriage-returned line on stderr and clears it on stop. Because
/// the reasoning gutter and the diagnostics share that stream, every writer
/// stops the bar before it emits anything, so the two never interleave.
pub(crate) struct LoadingBar {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl LoadingBar {
    /// Animates a bar on its own thread until [`LoadingBar::stop`] is called.
    ///
    /// The thread repaints a single line in place. It is written through
    /// [`Style`], so a terminal without colour shows the frames plain instead of
    /// leaking raw SGR codes.
    pub(crate) fn start(style: Style) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut tick = 0_usize;
            while !stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                let line = format!("  {} {}", style.dim("working…"), loading_frame(style, tick));
                // `\r` returns to the start of the bar's own line and `\x1b[2K`
                // clears it, so a shorter frame never leaves the tail of a long
                // one behind.
                eprint!("\r\x1b[2K{line}");
                let _ = std::io::stderr().flush();
                std::thread::sleep(std::time::Duration::from_millis(LOADING_TICK_MS));
                tick = tick.wrapping_add(1);
            }
            // Leave the cursor at the start of a cleared line for whatever runs
            // after the bar.
            eprint!("\r\x1b[2K");
            let _ = std::io::stderr().flush();
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Stops the animation and joins its thread, clearing the bar's line.
    pub(crate) fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LoadingBar {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Width of the loading bar, in cells, not counting the label.
const LOADING_BAR_WIDTH: usize = 24;

/// How often the loading bar advances, in milliseconds.
const LOADING_TICK_MS: u64 = 90;

/// One frame of the bar: a highlight that travels left to right.
///
/// The cells are painted through [`Style`], so a terminal without colour draws a
/// legible plain frame instead of leaking raw SGR codes.
fn loading_frame(style: Style, tick: usize) -> String {
    // `head` stays below the bar width, so `abs_diff` on `usize` cannot wrap.
    let head = tick % (LOADING_BAR_WIDTH + 4);
    let mut frame = String::with_capacity(LOADING_BAR_WIDTH * 4);
    for cell in 0..LOADING_BAR_WIDTH {
        let distance = cell.abs_diff(head);
        if distance < 2 {
            frame.push_str(&style.bold("█"));
        } else if distance < 4 {
            frame.push_str(&style.faint("▓"));
        } else {
            frame.push_str(&style.dim("░"));
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use norte_code_protocol::{Delta, ServerEvent, ServerEventKind};

    use super::{EditPreview, Format, Renderer, build_edit_preview, edit_preview, todo_items};
    use crate::input::{Pinned, TodoItem, TodoStatus};
    use crate::style::Style;

    fn renderer(pinned: &Arc<Pinned>) -> Renderer {
        Renderer::new(
            Format::Text,
            true,
            false,
            false,
            true,
            Style::plain(),
            Arc::clone(pinned),
        )
    }

    fn visible_renderer(pinned: &Arc<Pinned>) -> Renderer {
        Renderer::new(
            Format::Text,
            false,
            false,
            false,
            true,
            Style::plain(),
            Arc::clone(pinned),
        )
    }

    fn todo_started(call_id: &str, todos: &serde_json::Value) -> ServerEvent {
        ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::ToolStarted {
                turn: 1,
                call_id: call_id.to_owned(),
                tool: "todo".to_owned(),
                arguments: Some(serde_json::json!({ "todos": todos })),
                source: None,
            },
        }
    }

    fn tool_finished(call_id: &str, tool: &str, is_error: bool) -> ServerEvent {
        ServerEvent {
            request_id: 1,
            session: None,
            kind: ServerEventKind::ToolFinished {
                turn: 1,
                call_id: call_id.to_owned(),
                tool: tool.to_owned(),
                is_error,
                error: None,
                output: None,
                truncated: false,
                duration_ms: 0,
            },
        }
    }

    fn item(content: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            content: content.to_owned(),
            status,
        }
    }

    #[test]
    fn an_empty_text_delta_does_not_leave_the_reasoning_block() {
        let pinned = Pinned::new();
        let mut renderer = visible_renderer(&pinned);
        renderer.delta(&Delta::Reasoning {
            index: 0,
            text: "thinking".to_owned(),
        });
        assert_eq!(renderer.block, super::Block::Reasoning);
        // A keepalive or the stream's trailing empty part must not close the
        // gutter: doing so split one thought into several `┌ thinking` blocks.
        renderer.delta(&Delta::Text {
            index: 0,
            text: String::new(),
        });
        assert_eq!(renderer.block, super::Block::Reasoning);
    }

    #[test]
    fn an_empty_reasoning_delta_does_not_open_the_gutter() {
        let pinned = Pinned::new();
        let mut renderer = visible_renderer(&pinned);
        renderer.delta(&Delta::Reasoning {
            index: 0,
            text: String::new(),
        });
        assert_eq!(renderer.block, super::Block::None);
    }

    #[test]
    fn a_text_delta_after_reasoning_closes_the_gutter() {
        let pinned = Pinned::new();
        let mut renderer = visible_renderer(&pinned);
        renderer.delta(&Delta::Reasoning {
            index: 0,
            text: "thinking".to_owned(),
        });
        renderer.delta(&Delta::Text {
            index: 1,
            text: "answer".to_owned(),
        });
        assert_eq!(renderer.block, super::Block::Answer);
    }

    #[test]
    fn loading_frame_is_legible_without_colour() {
        let frame = super::loading_frame(Style::plain(), 3);
        assert_eq!(frame.chars().count(), super::LOADING_BAR_WIDTH);
        assert!(
            !frame.contains('\u{1b}'),
            "plain style must not emit escapes"
        );
    }

    #[test]
    fn loading_frame_moves_with_the_tick() {
        assert_ne!(
            super::loading_frame(Style::plain(), 0),
            super::loading_frame(Style::plain(), 2)
        );
    }

    #[test]
    fn a_cost_silent_provider_hides_the_missing_cost() {
        let pinned = Pinned::new();
        let mut renderer = renderer(&pinned);
        renderer.requests = 1;
        renderer.unpriced = 1;
        assert!(renderer.session_status().contains("cost not reported"));
        renderer.set_cost_silent(true);
        assert!(!renderer.session_status().contains("not reported"));
    }

    #[test]
    fn the_context_dial_warns_as_the_window_fills() {
        let style = Style::plain();
        // A quarter full is neither near nor over the limit.
        let low = super::context_dial(style, 25_000, Some(100_000), 0.8, false);
        assert!(low.contains("25%"), "{low}");
        assert!(!low.contains("compact"), "{low}");
        // Past the threshold the dial says so.
        let over = super::context_dial(style, 90_000, Some(100_000), 0.8, false);
        assert!(over.contains("compact"), "{over}");
    }

    #[test]
    fn the_context_dial_without_a_window_shows_only_the_count() {
        let dial = super::context_dial(Style::plain(), 12_000, None, 0.8, false);
        assert_eq!(dial, "ctx 12k");
    }

    #[test]
    fn a_successful_todo_call_updates_the_footer() {
        let pinned = Pinned::new();
        let mut renderer = renderer(&pinned);
        renderer
            .event(&todo_started(
                "call-1",
                &serde_json::json!([
                    {"content": "inspect", "status": "completed"},
                    {"content": "implement", "status": "in_progress"}
                ]),
            ))
            .expect("started");
        assert!(pinned.todos().is_empty());
        renderer
            .event(&tool_finished("call-1", "todo", false))
            .expect("finished");
        assert_eq!(
            pinned.todos(),
            vec![
                item("inspect", TodoStatus::Completed),
                item("implement", TodoStatus::InProgress)
            ]
        );
    }

    #[test]
    fn a_failed_todo_call_leaves_the_footer_unchanged() {
        let pinned = Pinned::new();
        let mut renderer = renderer(&pinned);
        renderer
            .event(&todo_started(
                "call-1",
                &serde_json::json!([{"content": "keep", "status": "pending"}]),
            ))
            .expect("started");
        renderer
            .event(&tool_finished("call-1", "todo", false))
            .expect("finished");
        renderer
            .event(&todo_started(
                "call-2",
                &serde_json::json!([{"content": "discard", "status": "pending"}]),
            ))
            .expect("started");
        renderer
            .event(&tool_finished("call-2", "todo", true))
            .expect("failed");
        assert_eq!(pinned.todos(), vec![item("keep", TodoStatus::Pending)]);
    }

    #[test]
    fn an_empty_todo_list_clears_the_footer() {
        let pinned = Pinned::new();
        let mut renderer = renderer(&pinned);
        renderer
            .event(&todo_started(
                "call-1",
                &serde_json::json!([{"content": "one", "status": "pending"}]),
            ))
            .expect("started");
        renderer
            .event(&tool_finished("call-1", "todo", false))
            .expect("finished");
        renderer
            .event(&todo_started("call-2", &serde_json::json!([])))
            .expect("started");
        renderer
            .event(&tool_finished("call-2", "todo", false))
            .expect("finished");
        assert!(pinned.todos().is_empty());
    }

    #[test]
    fn a_non_todo_call_does_not_touch_the_footer() {
        let pinned = Pinned::new();
        let mut renderer = renderer(&pinned);
        renderer
            .event(&todo_started(
                "call-1",
                &serde_json::json!([{"content": "not a todo", "status": "pending"}]),
            ))
            .expect("started");
        renderer
            .event(&tool_finished("call-1", "write", false))
            .expect("finished");
        assert!(pinned.todos().is_empty());
    }

    #[test]
    fn malformed_todo_arguments_are_ignored() {
        assert!(todo_items(None).is_none());
        assert!(todo_items(Some(&serde_json::json!({}))).is_none());
        assert!(todo_items(Some(&serde_json::json!({"todos": "nope"}))).is_none());
        assert!(
            todo_items(Some(
                &serde_json::json!({"todos": [{"content": "x", "status": "unknown"}]})
            ))
            .is_none()
        );
        assert!(todo_items(Some(&serde_json::json!({"todos": [{"status": "pending"}]}))).is_none());
        assert_eq!(
            todo_items(Some(
                &serde_json::json!({"todos": [{"content": "x", "status": "pending"}]})
            )),
            Some(vec![item("x", TodoStatus::Pending)])
        );
    }

    fn preview(source: &str, edits: &serde_json::Value) -> EditPreview {
        let arguments = serde_json::json!({ "edits": edits });
        build_edit_preview(
            "src/lib.rs".to_owned(),
            arguments["edits"].as_array().expect("edits"),
            Some(source.to_owned()),
        )
    }

    #[test]
    fn edit_preview_shows_context_and_line_numbers() {
        let source =
            "fn main() {\n    let one = 1;\n    let two = 2;\n    old();\n    let three = 3;\n}\n";
        let preview = preview(
            source,
            &serde_json::json!([{
                "search": "    old();\n",
                "replace": "    new();\n"
            }]),
        );
        let edit = &preview.boxes[0];
        assert_eq!(edit.old_range, Some((4, 4)));
        assert_eq!(edit.new_range, Some((4, 4)));
        assert_eq!(
            edit.rows
                .iter()
                .map(|row| (
                    row.left.as_ref().and_then(|cell| cell.number),
                    row.right.as_ref().and_then(|cell| cell.number)
                ))
                .collect::<Vec<_>>(),
            vec![
                (Some(2), Some(2)),
                (Some(3), Some(3)),
                (Some(4), Some(4)),
                (Some(5), Some(5)),
                (Some(6), Some(6))
            ]
        );
        let changed = &edit.rows[2];
        assert_eq!(changed.left.as_ref().expect("old").text, "    old();");
        assert_eq!(changed.right.as_ref().expect("new").text, "    new();");
        assert!(changed.left.as_ref().expect("old").changed);
        assert!(changed.right.as_ref().expect("new").changed);
        assert!(!edit.rows[0].left.as_ref().expect("context").changed);
    }

    #[test]
    fn edit_preview_numbers_later_edits_after_inserted_lines() {
        let source = "a\nb\nc\nd\n";
        let preview = preview(
            source,
            &serde_json::json!([
                {"search": "b\n", "replace": "b\nx\n"},
                {"search": "d\n", "replace": "D\n"}
            ]),
        );
        assert_eq!(preview.boxes[0].new_range, Some((2, 3)));
        let second = &preview.boxes[1];
        assert_eq!(second.old_range, Some((5, 5)));
        assert_eq!(second.new_range, Some((5, 5)));
        let changed = second
            .rows
            .iter()
            .find(|row| row.left.as_ref().is_some_and(|cell| cell.changed))
            .expect("changed row");
        assert_eq!(changed.left.as_ref().expect("old").text, "d");
        assert_eq!(changed.right.as_ref().expect("new").text, "D");
    }

    #[test]
    fn edit_preview_marks_a_deletion_without_a_result_range() {
        let source = "a\nb\nc\n";
        let preview = preview(
            source,
            &serde_json::json!([{"search": "b\n", "replace": ""}]),
        );
        let edit = &preview.boxes[0];
        assert_eq!(edit.old_range, Some((2, 2)));
        assert_eq!(edit.new_range, None);
        assert_eq!(edit.rows[1].left.as_ref().expect("old").text, "b");
        assert!(edit.rows[1].right.is_none());
        assert_eq!(edit.rows[2].left.as_ref().expect("old").text, "c");
        assert_eq!(edit.rows[2].right.as_ref().expect("new").text, "c");
    }

    #[test]
    fn edit_preview_locates_a_whitespace_only_match() {
        let source = "fn main() {\n    old();\n}\n";
        let preview = preview(
            source,
            &serde_json::json!([{
                "search": "      old();\n}",
                "replace": "      new();\n}"
            }]),
        );
        let edit = &preview.boxes[0];
        assert!(edit.approximate);
        assert_eq!(edit.old_range, Some((2, 3)));
        assert_eq!(edit.rows[1].left.as_ref().expect("old").text, "    old();");
        assert_eq!(
            edit.rows[1].right.as_ref().expect("new").text,
            "      new();"
        );
        assert_eq!(edit.rows[2].right.as_ref().expect("context").text, "}");
    }

    #[test]
    fn edit_preview_without_a_source_has_no_row_numbers() {
        let arguments = serde_json::json!({
            "path": "src/lib.rs",
            "edits": [{"search": "one\ntwo", "replace": "three"}]
        });
        let preview = build_edit_preview(
            "src/lib.rs".to_owned(),
            arguments["edits"].as_array().expect("edits"),
            None,
        );
        let edit = &preview.boxes[0];
        assert!(edit.old_range.is_none());
        assert_eq!(edit.rows[0].left.as_ref().expect("old").number, None);
        assert_eq!(edit.rows[0].left.as_ref().expect("old").text, "one");
        assert_eq!(edit.rows[1].left.as_ref().expect("old").text, "two");
        assert_eq!(edit.rows[0].right.as_ref().expect("new").text, "three");
    }

    #[test]
    fn edit_preview_uses_the_captured_source() {
        let arguments = serde_json::json!({
            "path": "/a/path/that/does/not/exist.rs",
            "edits": [{"search": "two\n", "replace": "TWO\n"}]
        });
        let preview = edit_preview(Some(&arguments), Some("one\ntwo\nthree\n"))
            .expect("preview from captured source");
        let edit = &preview.boxes[0];
        assert_eq!(edit.old_range, Some((2, 2)));
        assert_eq!(edit.rows[1].left.as_ref().expect("old").text, "two");
        assert_eq!(edit.rows[1].right.as_ref().expect("new").text, "TWO");
    }

    #[test]
    fn edit_preview_without_edits_is_not_shown() {
        let arguments = serde_json::json!({"path": "src/lib.rs", "edits": []});
        assert!(edit_preview(Some(&arguments), None).is_none());
        assert!(edit_preview(None, None).is_none());
    }
}
