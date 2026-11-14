use norte_code_wire::{MessageRole, OutputStream, ToolStatus};
use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::TextBuilder;
use crate::{
    components::palette,
    ui_model::{ReasoningLevel, SessionAttachmentState, SessionModel, TurnState, UsageSummary},
};
use norte_code_wire::QueueMode;

/// A token count short enough to read.
fn human_count(value: u64) -> String {
    let (divisor, suffix) = if value >= 1_000_000 {
        (1_000_000, "M")
    } else if value >= 1_000 {
        (1_000, "k")
    } else {
        return value.to_string();
    };
    let whole = value / divisor;
    let tenth = (value % divisor) * 10 / divisor;
    format!("{whole}.{tenth}{suffix}")
}

impl TextBuilder {
    #[must_use]
    pub fn transcript_title(
        session: Option<&str>,
        turn_state: TurnState,
        attachment_state: SessionAttachmentState,
        model: Option<&SessionModel>,
    ) -> Line<'static> {
        let label = session.map_or_else(
            || Self::no_active_session().to_owned(),
            Self::active_session_label,
        );
        let state = match attachment_state {
            SessionAttachmentState::Disconnected => "disconnected",
            SessionAttachmentState::Connecting => "connecting",
            SessionAttachmentState::Failed => "connection error",
            SessionAttachmentState::Attached => match turn_state {
                TurnState::Idle => Self::turn_idle_label(),
                TurnState::Connecting => Self::turn_connecting_label(),
                TurnState::Thinking => Self::turn_thinking_label(),
                TurnState::Streaming => Self::turn_running_label(),
                TurnState::AwaitingAnswer => Self::turn_awaiting_answer_label(),
            },
        };
        let mut spans = vec![Span::raw(label)];
        if let Some(model) = model {
            spans.push(Span::raw(" · "));
            spans.push(Span::raw(format!("{}/{}", model.provider, model.model)));
        }
        spans.push(Span::raw(" · "));
        spans.push(Span::raw(state));
        Line::from(spans)
    }

    #[must_use]
    pub fn speaker_label(speaker: MessageRole, text: &str) -> Line<'static> {
        let label = match speaker {
            MessageRole::User => Self::user_speaker_label(),
            MessageRole::Assistant => Self::assistant_speaker_label(),
            MessageRole::Tool => Self::tool_call_label(),
            MessageRole::System => Self::system_speaker_label(),
            MessageRole::Developer => Self::developer_speaker_label(),
            MessageRole::Unknown => Self::unknown_speaker_label(),
        };
        Line::from(vec![
            Span::raw(format!("{label}  ")),
            Span::raw(text.to_owned()),
        ])
    }

    #[must_use]
    pub fn thinking_summary() -> Line<'static> {
        Line::from(Span::styled(
            "Thought",
            Style::default()
                .fg(palette::muted())
                .add_modifier(ratatui::style::Modifier::ITALIC),
        ))
    }

    /// The footer under an answer: what the model request reported.
    #[must_use]
    pub fn usage_footer(usage: &UsageSummary) -> Option<Line<'static>> {
        let mut parts: Vec<String> = Vec::new();
        if let (Some(provider), Some(model)) = (&usage.provider, &usage.model) {
            parts.push(format!("{provider}/{model}"));
        }
        if let Some(input) = usage.input_tokens {
            parts.push(format!("{} in", human_count(input)));
        }
        if let Some(output) = usage.output_tokens {
            parts.push(format!("{} out", human_count(output)));
        }
        if let Some(cache) = usage.cache_read_tokens {
            parts.push(format!("{} cached", human_count(cache)));
        }
        if let Some(duration) = usage.duration_ms {
            parts.push(format!("{duration} ms"));
        }
        if let Some(speed) = usage.tokens_per_second {
            parts.push(format!("{speed:.1} tok/s"));
        }
        if let Some(cost) = usage.cost_usd {
            parts.push(format!("${cost:.4}"));
        }
        if parts.is_empty() {
            return None;
        }
        Some(Line::from(Span::styled(
            parts.join(" · "),
            Style::default().fg(palette::muted()),
        )))
    }

    /// The line under the input: what the session is set to.
    #[must_use]
    pub fn composer_status(queue_mode: QueueMode, reasoning: ReasoningLevel) -> String {
        format!(
            "{} · {} change · reasoning {} · {} change",
            Self::queue_mode_description(queue_mode),
            crate::keys::QUEUE_MODE_CHAR,
            reasoning.label(),
            crate::keys::REASONING_CHAR
        )
    }

    #[must_use]
    pub fn tool_call_header(name: &str, target: Option<&str>) -> Line<'static> {
        let call = match target {
            Some(target) => format!("{}  {name}  {target}", Self::tool_call_label()),
            None => format!("{}  {name}", Self::tool_call_label()),
        };
        Line::from(call)
    }

    #[must_use]
    pub fn grep_target(pattern: &str, path: Option<&str>) -> String {
        match path {
            Some(path) => format!("{pattern} in {path}"),
            None => pattern.to_owned(),
        }
    }

    #[must_use]
    pub const fn tool_output_label(stream: OutputStream) -> &'static str {
        match stream {
            OutputStream::Stdout => "out",
            OutputStream::Stderr => "err",
            OutputStream::Other => "output",
        }
    }

    #[must_use]
    pub fn tool_finished(name: &str, status: ToolStatus) -> Line<'static> {
        let state = match status {
            ToolStatus::Ok => Self::tool_done_label(),
            ToolStatus::Error => Self::tool_failed_label(),
            ToolStatus::Cancelled => Self::tool_cancelled_label(),
        };
        Line::from(format!("{name}  {state}"))
    }

    #[must_use]
    pub fn media_delta(bytes: u64, last: bool) -> Line<'static> {
        let state = if last {
            Self::media_complete_label()
        } else {
            Self::media_streaming_label()
        };
        Line::from(format!("{state}  {bytes} bytes"))
    }

    #[must_use]
    pub fn transcript_error(error: &str) -> Line<'static> {
        Line::from(format!("{}  {error}", Self::error_label()))
    }

    /// The line for a refused request: what the host said and why.
    #[must_use]
    pub fn host_error(code: norte_code_wire::ErrorCode, message: &str) -> Line<'static> {
        let label = match code {
            norte_code_wire::ErrorCode::InvalidRequest => "request",
            norte_code_wire::ErrorCode::NotFound => "not found",
            norte_code_wire::ErrorCode::Conflict => "conflict",
            norte_code_wire::ErrorCode::Capability => "capability",
            norte_code_wire::ErrorCode::Budget => "budget",
            norte_code_wire::ErrorCode::Provider => "provider",
            _ => "host",
        };
        Line::from(format!("{label}  {message}"))
    }

    #[must_use]
    pub fn retry_scheduled(attempt: u32, delay_ms: u64, reason: &str) -> Line<'static> {
        #[allow(clippy::cast_precision_loss)]
        let seconds = delay_ms as f64 / 1000.0;
        let reason = reason.trim();
        let text = if reason.is_empty() {
            format!("retry  attempt {attempt} in {seconds:.1}s")
        } else {
            let reason = Self::truncate_inline(reason, 240);
            format!("retry  attempt {attempt} in {seconds:.1}s: {reason}")
        };
        Line::from(Span::styled(text, Style::default().fg(palette::warning())))
    }

    #[must_use]
    pub fn retry_started(attempt: u32) -> Line<'static> {
        Line::from(Span::styled(
            format!("retry  attempt {attempt} started"),
            Style::default().fg(palette::warning()),
        ))
    }

    fn truncate_inline(text: &str, max_chars: usize) -> String {
        let mut chars = text.chars();
        let taken: String = chars.by_ref().take(max_chars).collect();
        if chars.next().is_some() {
            format!("{taken}...")
        } else {
            taken
        }
    }

    #[must_use]
    pub fn diff_unavailable() -> Line<'static> {
        Line::from(Self::diff_unavailable_label())
    }

    #[must_use]
    pub fn diff_too_large() -> Line<'static> {
        Line::from(Self::diff_too_large_label())
    }

    #[must_use]
    pub fn unified_diff_label() -> Line<'static> {
        Line::from(Self::unified_diff_title())
    }

    #[must_use]
    pub fn split_diff_label() -> Line<'static> {
        Line::from(format!(
            "{}  {}",
            Self::split_diff_title(),
            Self::split_diff_column_titles()
        ))
    }

    #[must_use]
    pub const fn no_active_session() -> &'static str {
        "no session"
    }

    #[must_use]
    pub fn active_session_label(session: &str) -> String {
        format!("session {session}")
    }

    #[must_use]
    pub const fn turn_running_label() -> &'static str {
        "working"
    }

    #[must_use]
    pub const fn turn_connecting_label() -> &'static str {
        "connecting"
    }

    #[must_use]
    pub const fn turn_thinking_label() -> &'static str {
        "thinking"
    }

    #[must_use]
    pub const fn turn_idle_label() -> &'static str {
        "ready"
    }

    #[must_use]
    pub const fn turn_awaiting_answer_label() -> &'static str {
        "waiting for your answer"
    }

    #[must_use]
    pub const fn tool_call_label() -> &'static str {
        "tool"
    }

    #[must_use]
    pub const fn tool_done_label() -> &'static str {
        "done"
    }

    #[must_use]
    pub const fn tool_failed_label() -> &'static str {
        "failed"
    }

    #[must_use]
    pub const fn tool_cancelled_label() -> &'static str {
        "detached"
    }

    #[must_use]
    pub const fn error_label() -> &'static str {
        "error"
    }

    #[must_use]
    pub const fn diff_unavailable_label() -> &'static str {
        "edit diff unavailable"
    }

    #[must_use]
    pub const fn diff_too_large_label() -> &'static str {
        "edit diff exceeds the display limit"
    }

    #[must_use]
    pub const fn unified_diff_title() -> &'static str {
        "unified"
    }

    #[must_use]
    pub const fn split_diff_title() -> &'static str {
        "split"
    }

    #[must_use]
    pub const fn split_diff_column_titles() -> &'static str {
        "before | after"
    }

    #[must_use]
    pub const fn user_speaker_label() -> &'static str {
        "you"
    }

    #[must_use]
    pub const fn assistant_speaker_label() -> &'static str {
        "norte"
    }

    #[must_use]
    pub const fn system_speaker_label() -> &'static str {
        "system"
    }

    #[must_use]
    pub const fn developer_speaker_label() -> &'static str {
        "developer"
    }

    #[must_use]
    pub const fn unknown_speaker_label() -> &'static str {
        "unknown"
    }

    #[must_use]
    pub const fn media_complete_label() -> &'static str {
        "media complete"
    }

    #[must_use]
    pub const fn media_streaming_label() -> &'static str {
        "media"
    }
}
