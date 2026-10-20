//! Output formatting: event renderer, debug views and small label helpers.

use norte_code_agent::DEFAULT_SYSTEM_PROMPT;

use crate::input::Pinned;

use super::{
    ApprovalMode, BTreeMap, Budget, BudgetAction, BudgetScope, BudgetUnit, CacheMode, Config,
    Delta, ErrorCode, Feature, Modality, ModelSpec, Path, PathBuf, Phase, Reasoning,
    ReasoningEffort, ReasoningInfo, ServerEvent, ServerEventKind, Service, Source,
    StreamingGranularity, StructuredOutputMode, Style, ToolKind, Write, clip_text, compact_json,
    duration_label,
};
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

/// Renders one server event to the terminal.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Renderer {
    pub(crate) format: Format,
    pub(crate) quiet: bool,
    pub(crate) verbose: bool,
    pub(crate) debug: bool,
    pub(crate) debug_session: bool,
    pub(crate) show_cost: bool,
    pub(crate) style: Style,
    /// The pinned footer text shared with the input editor.
    pub(crate) pinned: Arc<Pinned>,
    pub(crate) block: Block,
    pub(crate) reasoning_line_start: bool,
    /// Whether `--quiet` has already printed its one redacted reasoning marker
    /// for this turn.
    pub(crate) reasoning_placeholder: bool,
    pub(crate) last_char: Option<char>,
    pub(crate) failed: bool,
    pub(crate) cancelled: bool,
    pub(crate) budget_stopped: bool,
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
}

impl Renderer {
    pub(crate) fn event(&mut self, event: &ServerEvent) -> Result<(), String> {
        if self.debug && !self.debug_session {
            if let Some(session) = &event.session {
                eprintln!("{}", self.style.dim(format!("debug: session {session}")));
                self.debug_session = true;
            }
        }
        if matches!(self.format, Format::Jsonl) {
            let line = serde_json::to_string(event).map_err(|error| error.to_string())?;
            println!("{line}");
            return Ok(());
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
            kind => self.status_event(kind),
        }
        Ok(())
    }

    /// Notes that a cancellation was requested, so the turn's end reads as a
    /// cancellation rather than a failure.
    pub(crate) fn mark_cancelled(&mut self) {
        self.cancelled = true;
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
        self.end_reasoning();
        if self.block == Block::Answer && self.last_char != Some('\n') {
            println!();
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
        if self.quiet {
            print!("{text}");
            if let Some(last) = text.chars().last() {
                self.last_char = Some(last);
            }
            let _ = std::io::stdout().flush();
            return;
        }
        if self.block == Block::Reasoning {
            self.end_reasoning();
            self.space();
        } else if self.block != Block::Answer && !text.is_empty() {
            if self.last_char.is_some() {
                self.space();
            }
            self.block = Block::Answer;
        }
        self.block = Block::Answer;
        print!("{text}");
        if let Some(last) = text.chars().last() {
            self.last_char = Some(last);
        }
        let _ = std::io::stdout().flush();
    }

    /// Renders a fenced block, so verbose tool activity stays readable and can
    /// be copied into a markdown report.
    pub(crate) fn code_block(&self, language: &str, text: &str) {
        eprintln!("  {}", self.style.faint(format!("```{language}")));
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
    #[allow(clippy::collapsible_match)]
    pub(crate) fn tool_event(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::ToolStarted {
                tool,
                call_id,
                arguments,
                ..
            } => {
                *self.tool_counts.entry(tool.clone()).or_default() += 1;
                let command = Self::command_summary(arguments.as_ref());
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
                    if self.verbose && tool.as_str() != "ask_user" {
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
                is_error,
                error,
                output,
                truncated,
                duration_ms,
                ..
            } => {
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
                            if let Some(error) = error {
                                self.code_block("text", error);
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
                    parts.push(match cost_usd {
                        Some(cost) if *cost > 0.0 => self.style.green(format!("${cost:.6}")),
                        Some(cost) => self.style.faint(format!("${cost:.6}")),
                        None => self.style.yellow("cost not reported".to_owned()),
                    });
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
            }
            ServerEventKind::TurnStarted { .. } => {
                self.cancelled = false;
                self.reasoning_placeholder = false;
                self.turn_requests = 0;
            }
            ServerEventKind::RetryScheduled {
                attempt,
                delay_ms,
                reason,
                ..
            } => {
                self.end_blocks();
                if !self.quiet {
                    eprintln!(
                        "{} {} {}",
                        self.style.yellow("retrying"),
                        self.style
                            .dim(format!("in {delay_ms} ms (attempt {attempt}):")),
                        self.style.faint(reason)
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
                        eprintln!(
                            "{}",
                            self.style.yellow(
                                "stopped by budget; raise it and resume the session to continue"
                            )
                        );
                    }
                } else if !*committed {
                    eprintln!("{}", self.style.red("the turn did not commit"));
                    self.failed = true;
                } else if *phase == Phase::AwaitingApproval && !self.quiet {
                    eprintln!("{}", self.style.yellow("paused: waiting for approval"));
                }
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
            ServerEventKind::BudgetChanged { budget } if !self.quiet => match budget {
                Some(budget) => eprintln!(
                    "{} {}",
                    self.style.dim("budget set:"),
                    self.style.dim(budget_label(budget))
                ),
                None => eprintln!("{}", self.style.dim("budget cleared")),
            },
            _ => {}
        }
    }

    pub(crate) fn delta(&mut self, delta: &Delta) {
        match delta {
            Delta::Text { text, .. } => self.answer_delta(text),
            Delta::Reasoning { text, .. } => {
                if self.quiet {
                    if !self.reasoning_placeholder {
                        self.reasoning_placeholder = true;
                        eprintln!("  {}", self.style.dim("thinking…"));
                    }
                } else {
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
        let mut parts = vec![
            self.style.cyan(format!("{} req", self.requests)),
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
                parts.push(self.style.yellow("cost not reported"));
            } else {
                parts.push(if self.cost > 0.0 {
                    self.style.green(format!("${:.6}", self.cost))
                } else {
                    self.style.faint(format!("${:.6}", self.cost))
                });
                if self.unpriced > 0 {
                    parts.push(self.style.yellow(format!("{} not reported", self.unpriced)));
                }
            }
        }
        format!(
            "{} {}",
            self.style.faint("session"),
            parts.join(&self.style.faint(" · "))
        )
    }

    /// Replaces the queued-message preview above the input line.
    pub(crate) fn set_queued(&self, lines: Vec<String>) {
        self.pinned.set_queued(lines);
    }

    /// Clears the queued-message preview once the messages are sent.
    pub(crate) fn clear_queued(&self) {
        self.pinned.set_queued(Vec::new());
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
                parts.push(self.style.yellow("cost not reported"));
            } else {
                parts.push(if self.cost > 0.0 {
                    self.style.green(format!("${:.6}", self.cost))
                } else {
                    self.style.faint(format!("${:.6}", self.cost))
                });
                if self.unpriced > 0 {
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
) {
    let workspace =
        std::env::current_dir().map_or_else(|_| "?".to_owned(), |path| path.display().to_string());
    let config_label =
        config_path.map_or_else(|| "(default)".to_owned(), |path| path.display().to_string());
    let iterations = configuration
        .max_iterations
        .map_or_else(|| "unlimited".to_owned(), |limit| limit.to_string());
    eprintln!("{}", style.dim("debug: startup"));
    eprintln!(
        "{}",
        style.dim(format!(
            "  session    {}",
            session.unwrap_or("(new; shown on first event)")
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  model      {} · provider {} · wire {}",
            spec.label(),
            spec.provider,
            spec.wire.family()
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  approval   {}",
            approval_label(configuration.approval_mode)
        ))
    );
    eprintln!("{}", style.dim(format!("  iterations {iterations}")));
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
    eprintln!("{}", style.dim(format!("  sandbox    {sandbox}")));
    eprintln!(
        "{}",
        style.dim(format!(
            "  compaction {}{}",
            configuration.compaction.threshold,
            if !configuration.compaction.enabled {
                " (disabled)"
            } else if configuration.compaction.trigger == "eager" {
                " (eager)"
            } else {
                " (idle)"
            }
        ))
    );
    eprintln!("{}", style.dim(format!("  workspace  {workspace}")));
    eprintln!("{}", style.dim(format!("  config     {config_label}")));
    eprintln!(
        "{}",
        style.dim(format!("  database   {}", database.display()))
    );
    eprintln!(
        "{}",
        style.dim(format!("  catalogue  {}", source_label(source)))
    );
    if !configuration.extensions.is_empty() {
        eprintln!(
            "{}",
            style.dim(format!(
                "  extensions {}",
                configuration
                    .extensions
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        );
    }
}

/// Prints the static prompt size in characters. Tokens are provider-reported,
/// so no estimate is made here.
pub(crate) fn debug_prompt_size(style: Style, service: &Service, source: &str) {
    match service.prompt_size() {
        Ok(size) => {
            eprintln!("{}", style.dim("debug: prompt material"));
            eprintln!("{}", style.dim(format!("  source     {source}")));
            eprintln!(
                "{}",
                style.dim(format!("  system     {} chars", size.system_chars))
            );
            if let Some(path) = &size.project_path {
                eprintln!(
                    "{}",
                    style.dim(format!(
                        "  project    {} chars ({})",
                        size.project_chars,
                        path.display()
                    ))
                );
            }
            eprintln!(
                "{}",
                style.dim(format!("  tools      {} chars", size.tool_chars))
            );
            eprintln!(
                "{}",
                style.dim(format!("  total      {} chars", size.total_chars()))
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
    eprintln!("{}", style.dim("debug: model"));
    eprintln!("{}", style.dim(format!("  reference  {}", spec.label())));
    if let Some(model) = model {
        if !model.name.is_empty() {
            eprintln!("{}", style.dim(format!("  name       {}", model.name)));
        }
        if let Some(family) = &model.family {
            eprintln!("{}", style.dim(format!("  family     {family}")));
        }
        if let Some(description) = &model.description {
            eprintln!("{}", style.dim(format!("  about      {description}")));
        }
    }
    eprintln!(
        "{}",
        style.dim(format!(
            "  wire       {} · provider {}",
            spec.wire.family(),
            spec.provider
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  context    {}",
            token_limit_label(capabilities.max_context)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  output     {}",
            token_limit_label(capabilities.max_output)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  modalities input {} · output {}",
            modality_set(capabilities.modalities.input.keys()),
            modality_set(capabilities.modalities.output.iter())
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  tools      {}",
            tool_kind_set(&capabilities.tool_kinds)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  reasoning  {}",
            reasoning_capabilities(&capabilities.reasoning)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  streaming  {}",
            capabilities
                .streaming
                .map_or_else(|| "-".to_owned(), |value| streaming_name(value).to_owned())
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!("  cache      {}", cache_name(capabilities.cache)))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  features   {}",
            feature_set(&capabilities.features)
        ))
    );
    eprintln!(
        "{}",
        style.dim(format!(
            "  structured {}",
            structured_set(&capabilities.structured_output)
        ))
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
    eprintln!("{}", style.dim("debug: reasoning"));
    eprintln!(
        "{}",
        style.dim(format!(
            "  effective  {}",
            reasoning_setting_label(reasoning)
        ))
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
    match normalized.as_str() {
        "auto" | "automatic" => Ok(Reasoning::Automatic),
        "off" | "none" | "disabled" => Ok(Reasoning::Disabled),
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
            let effort = match normalized.as_str() {
                "minimal" => "minimal",
                "low" => "low",
                "medium" => "medium",
                _ => "high",
            };
            Ok(Reasoning::Effort {
                effort: effort.to_owned(),
            })
        }
        _ => Err(format!(
            "`{value}` is not a reasoning setting; use auto, off, minimal, low, medium or high"
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
