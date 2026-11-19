//! The right half of the telemetry page.
//!
//! Two blocks: the totals of the selected session above, the selected request
//! below. Every row is a label and a value, the value colored by what it says,
//! so a slow request or a failed one reads before the number does.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::telemetry_pages::Pages;
use crate::components::{palette, panel, spacing};
use crate::text_builder::TextBuilder;
use crate::ui_model::{ThemeMode, UiModel};

/// The width the value column starts at.
const LABEL_WIDTH: usize = 15;

/// Draws the whole right half.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &UiModel,
    session: Option<&str>,
    requests: &[norte_code_wire::TelemetryRecord],
    pages: &Pages,
) {
    let theme = model.preferences.theme;
    let blocks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(12), Constraint::Min(6)])
        .split(area);

    let summary = summary_lines(model, session, requests, pages, theme, blocks[0].width);
    panel::render_text(frame, blocks[0], "session", String::new(), theme);
    frame.render_widget(
        Paragraph::new(ratatui::text::Text::from(summary)).style(palette::panel_style(theme)),
        ratatui::layout::Rect {
            x: blocks[0].x.saturating_add(spacing::PANEL_PADDING),
            y: blocks[0].y.saturating_add(1),
            width: blocks[0].width.saturating_sub(spacing::PANEL_PADDING * 2),
            height: blocks[0].height.saturating_sub(2),
        },
    );

    let detail = match requests.get(pages.selected()) {
        Some(record) => request_lines(record, theme, blocks[1].width),
        None => vec![Line::from(Span::styled(
            "No request selected",
            palette::muted_style(theme),
        ))],
    };
    panel::render_text(frame, blocks[1], "request", String::new(), theme);
    frame.render_widget(
        Paragraph::new(ratatui::text::Text::from(detail)).style(palette::panel_style(theme)),
        ratatui::layout::Rect {
            x: blocks[1].x.saturating_add(spacing::PANEL_PADDING),
            y: blocks[1].y.saturating_add(1),
            width: blocks[1].width.saturating_sub(spacing::PANEL_PADDING * 2),
            height: blocks[1].height.saturating_sub(2),
        },
    );
}

/// The totals of the selected session: how many, how long, how much.
fn summary_lines(
    model: &UiModel,
    session: Option<&str>,
    requests: &[norte_code_wire::TelemetryRecord],
    pages: &Pages,
    theme: ThemeMode,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(session) = session else {
        return vec![Line::from(Span::styled(
            "No session selected",
            palette::muted_style(theme),
        ))];
    };
    let title = model
        .sessions
        .iter()
        .find(|summary| summary.session_id == session)
        .and_then(|summary| summary.title.clone())
        .unwrap_or_else(|| session.chars().take(12).collect());
    let mut lines = vec![Line::from(vec![
        Span::styled(
            title,
            Style::default()
                .fg(palette::accent(theme))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{}/{}", pages.page() + 1, pages.total()),
            palette::muted_style(theme),
        ),
    ])];
    lines.push(Line::default());

    let requests_count = requests.len();
    let errors = requests
        .iter()
        .filter(|record| is_failure(record.status))
        .count();
    let duration: u64 = requests.iter().map(|record| record.duration_ms).sum();
    let input: u64 = requests
        .iter()
        .filter_map(|record| record.input_tokens)
        .sum();
    let output: u64 = requests
        .iter()
        .filter_map(|record| record.output_tokens)
        .sum();
    let cached: u64 = requests
        .iter()
        .filter_map(|record| record.cache_read_tokens)
        .sum();
    let cost: f64 = requests.iter().filter_map(|record| record.cost_usd).sum();

    lines.push(row(
        "requests",
        requests_count.to_string(),
        palette::text(theme),
        theme,
    ));
    lines.push(row(
        "errors",
        errors.to_string(),
        if errors > 0 {
            palette::danger()
        } else {
            palette::success()
        },
        theme,
    ));
    lines.push(row(
        "duration",
        TextBuilder::duration(duration),
        palette::text(theme),
        theme,
    ));
    lines.push(row(
        "input",
        TextBuilder::human_count(input),
        palette::text(theme),
        theme,
    ));
    lines.push(row(
        "output",
        TextBuilder::human_count(output),
        palette::text(theme),
        theme,
    ));
    lines.push(row(
        "cache read",
        TextBuilder::human_count(cached),
        if cached > 0 {
            palette::success()
        } else {
            palette::text_muted(theme)
        },
        theme,
    ));
    lines.push(row(
        "cost",
        format!("${cost:.4}"),
        palette::warning(),
        theme,
    ));
    let _ = width;
    lines
}

/// The selected request, row by row.
fn request_lines(
    record: &norte_code_wire::TelemetryRecord,
    theme: ThemeMode,
    width: u16,
) -> Vec<Line<'static>> {
    let status = status_label(record.status);
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("{} / {}", record.provider, record.model),
            Style::default()
                .fg(palette::accent(theme))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            status.0,
            Style::default().fg(status.1).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            TextBuilder::duration(record.duration_ms),
            palette::muted_style(theme),
        ),
    ])];
    lines.push(Line::default());

    lines.push(row(
        "tokens",
        format!(
            "{} in · {} out",
            optional(record.input_tokens),
            optional(record.output_tokens)
        ),
        palette::text(theme),
        theme,
    ));
    lines.push(row(
        "first token",
        record
            .first_token_ms
            .map_or_else(|| "-".to_owned(), TextBuilder::duration),
        palette::text(theme),
        theme,
    ));
    lines.push(row(
        "cache",
        cache_label(record),
        if record.cache_hit {
            palette::success()
        } else {
            palette::text_muted(theme)
        },
        theme,
    ));
    lines.push(row(
        "cost",
        record
            .cost_usd
            .map_or_else(|| "-".to_owned(), |cost| format!("${cost:.4}")),
        palette::warning(),
        theme,
    ));
    if let Some(finish) = record.finish_reason.as_deref() {
        lines.push(row(
            "finish",
            finish.to_owned(),
            palette::text_muted(theme),
            theme,
        ));
    }
    if !record.tool_calls.is_empty() {
        lines.push(row(
            "tools",
            record.tool_calls.join(", "),
            palette::info(),
            theme,
        ));
    }
    if let Some(error) = record.error.as_deref() {
        for line in wrap(error, usize::from(width).saturating_sub(LABEL_WIDTH)) {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<LABEL_WIDTH$}", "error"),
                    palette::muted_style(theme),
                ),
                Span::styled(line, Style::default().fg(palette::danger())),
            ]));
        }
    }
    lines
}

/// One label and its value.
fn row(
    label: &str,
    value: String,
    color: ratatui::style::Color,
    theme: ThemeMode,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<LABEL_WIDTH$}"),
            palette::muted_style(theme),
        ),
        Span::styled(value, Style::default().fg(color)),
    ])
}

/// The short word and the color a status is drawn with.
fn status_label(status: norte_code_wire::RequestStatus) -> (&'static str, ratatui::style::Color) {
    use norte_code_wire::RequestStatus;
    match status {
        RequestStatus::Ok => ("ok", palette::success()),
        RequestStatus::Error => ("error", palette::danger()),
        RequestStatus::Cancelled => ("cancelled", palette::warning()),
    }
}

fn is_failure(status: norte_code_wire::RequestStatus) -> bool {
    matches!(status, norte_code_wire::RequestStatus::Error)
}

fn optional(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn cache_label(record: &norte_code_wire::TelemetryRecord) -> String {
    match (record.cache_read_tokens, record.cache_write_tokens) {
        (Some(read), Some(write)) => format!(
            "{} read · {} write",
            TextBuilder::human_count(read),
            TextBuilder::human_count(write)
        ),
        (Some(read), None) => format!("{} read", TextBuilder::human_count(read)),
        (None, Some(write)) => format!("{} write", TextBuilder::human_count(write)),
        (None, None) => "-".to_owned(),
    }
}

/// Breaks a message at the width, keeping whole words where it can.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let projected = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };
        if projected > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
