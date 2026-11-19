use ratatui::{
    style::Style,
    text::{Line, Span},
};

use norte_code_client::{PluginSummary, ToolRule};
use norte_code_wire::{
    ChildSessionSummary, McpFailure, McpInfo, ModelInfo, PlanSummary, SessionSummary, SkillInfo,
    TelemetryRecord,
};

use super::TextBuilder;
use crate::{components::palette, ui_model::UiModel};

/// How far indented the meta line sits under a numbered title.
const META_INDENT: &str = "      ";

/// Days after the epoch, as a civil date `YYYY-MM-DD`.
///
/// The calendar math is Howard Hinnant's civil-from-days; no date library is
/// pulled in for one line of text.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let doe = shifted.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The civil date a moment belongs to, `YYYY-MM-DD`.
fn civil_date(milliseconds: i64) -> String {
    const DAY_MS: i64 = 86_400_000;
    let (year, month, day) = civil_from_days(milliseconds.div_euclid(DAY_MS));
    format!("{year:04}-{month:02}-{day:02}")
}

/// How long ago a moment is, as `just now`, `5m ago`, `3h ago`, `2d ago` or,
/// past a month, its civil date.
fn relative_time(milliseconds: i64, now_milliseconds: i64) -> String {
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let elapsed = (now_milliseconds - milliseconds).max(0);
    match elapsed {
        delta if delta < MINUTE => "just now".to_owned(),
        delta if delta < HOUR => format!("{}m ago", delta / MINUTE),
        delta if delta < DAY => format!("{}h ago", delta / HOUR),
        delta if delta < 30 * DAY => format!("{}d ago", delta / DAY),
        _ => civil_date(milliseconds),
    }
}

/// The clock the renderer reads, in milliseconds since the epoch.
fn now_milliseconds() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis()),
    )
    .unwrap_or(i64::MAX)
}

impl TextBuilder {
    /// The entry a session takes in the list: its title, then one muted
    /// line with what the meta line is for — how many turns, when it was
    /// created and when it last did some work. A collapsed row carries the
    /// expand marker; an expanded row carries the collapse marker.
    #[must_use]
    pub fn numbered_session_row(
        number: usize,
        session: &SessionSummary,
        expanded: bool,
    ) -> Vec<Line<'static>> {
        let title = session.title.as_deref().unwrap_or("Untitled session");
        let mut head = vec![Span::styled(
            format!("{number:>2}."),
            Style::default().fg(palette::accent(crate::ui_model::ThemeMode::Dark)),
        )];
        head.push(Span::raw(if session.pinned { " ★" } else { "  " }));
        head.push(Span::raw(format!(
            "{title}{}",
            if expanded { " ▾" } else { "" }
        )));
        let mut meta: Vec<Span<'static>> = vec![Span::raw(META_INDENT)];
        let mut pieces: Vec<String> = Vec::with_capacity(3);
        if let Some(turns) = session.turn_count {
            pieces.push(format!("{turns} turns"));
        }
        let now = now_milliseconds();
        if let Some(created_at) = session.created_at {
            pieces.push(format!("created {}", civil_date(created_at)));
        }
        if let Some(updated_at) = session.updated_at {
            pieces.push(format!("updated {}", relative_time(updated_at, now)));
        }
        meta.push(Span::styled(
            pieces.join(" · "),
            Style::default().fg(palette::text_muted(crate::ui_model::ThemeMode::Dark)),
        ));
        vec![Line::from(head), Line::from(meta)]
    }

    /// The entry a subagent's session takes: one line, like the sessions
    /// the host runs itself, without the meta line. The expanded state is
    /// shown in a trailing marker.
    #[must_use]
    pub fn numbered_child_session_row(
        number: usize,
        session: &ChildSessionSummary,
        expanded: bool,
    ) -> Vec<Line<'static>> {
        vec![Line::from(vec![
            Span::styled(
                format!("{number:>2}."),
                Style::default().fg(palette::accent(crate::ui_model::ThemeMode::Dark)),
            ),
            Span::raw(format!(
                " {}{}",
                session.title.as_deref().unwrap_or("subagent"),
                if expanded { " ▾" } else { "" }
            )),
            Span::styled(
                format!(
                    "  {} · {}",
                    session.state.label(),
                    session.task.as_deref().unwrap_or(&session.session_id)
                ),
                Style::default().fg(palette::text_muted(crate::ui_model::ThemeMode::Dark)),
            ),
        ])]
    }

    /// The line an action of an expanded session takes: an indented marker
    /// and the action, in accent when it is the selected one.
    #[must_use]
    pub fn session_action_row(
        label: &str,
        selected: bool,
        theme: crate::ui_model::ThemeMode,
    ) -> Vec<Line<'static>> {
        let style = if selected {
            Style::default()
                .fg(palette::accent(theme))
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            Style::default().fg(palette::text_muted(crate::ui_model::ThemeMode::Dark))
        };
        vec![Line::from(Span::styled(format!("      ▸ {label}"), style))]
    }

    #[must_use]
    pub fn plan_row(plan: &PlanSummary) -> Line<'static> {
        Line::from(format!(
            "{}  {}  v{}",
            plan.status.label(),
            plan.title.as_deref().unwrap_or(&plan.id),
            plan.revision
        ))
    }

    /// The row a session takes in the telemetry list: its title, its requests
    /// and what they cost.
    #[must_use]
    pub fn telemetry_session_row(model: &UiModel, session: &str) -> Line<'static> {
        let report = model.telemetry.as_ref();
        let requests = report.map_or(0, |report| {
            report
                .records
                .iter()
                .filter(|record| record.session == session)
                .count()
        });
        let label = model
            .sessions
            .iter()
            .find(|summary| summary.session_id == session)
            .and_then(|summary| summary.title.clone())
            .unwrap_or_else(|| session.chars().take(8).collect());
        Line::from(vec![
            Span::raw(label),
            Span::styled(
                format!("  {requests} req"),
                Style::default().fg(palette::text_muted(crate::ui_model::ThemeMode::Dark)),
            ),
        ])
    }

    #[must_use]
    pub fn telemetry_row(record: &TelemetryRecord) -> Line<'static> {
        Line::from(format!(
            "{}  {} / {}  {} ms",
            record.started_at, record.provider, record.model, record.duration_ms
        ))
    }

    #[must_use]
    pub fn plugin_row(plugin: &PluginSummary) -> Line<'static> {
        let theme = crate::ui_model::ThemeMode::Dark;
        let (marker, color, state) = match (plugin.enabled, plugin.loaded) {
            (true, true) => ("●", palette::success(), "loaded".to_owned()),
            // An enabled plugin the host did not load has a reason or has not
            // been tried in this process: those are different problems, and
            // saying "failed" for both sent the reader after a build error
            // that never happened.
            (true, false) => match plugin.load_error.as_deref() {
                Some(reason) => ("✗", palette::danger(), format!("failed: {reason}")),
                None => (
                    "◌",
                    palette::warning(),
                    "not loaded in this process".to_owned(),
                ),
            },
            (false, true) => (
                "◐",
                palette::text_muted(theme),
                "disabled, still loaded".to_owned(),
            ),
            (false, false) => ("○", palette::text_muted(theme), "disabled".to_owned()),
        };
        Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(color)),
            Span::raw(plugin.name.clone()),
            Span::styled(
                format!("  {state}"),
                Style::default().fg(palette::text_muted(theme)),
            ),
        ])
    }

    #[must_use]
    pub fn skill_row(skill: &SkillInfo) -> Line<'static> {
        Line::from(format!("{}  {}", skill.name, skill.description))
    }

    #[must_use]
    pub fn warning_row(warning: &str) -> Line<'static> {
        Line::from(format!("warning  {warning}"))
    }

    #[must_use]
    pub fn mcp_row(server: &McpInfo) -> Line<'static> {
        let state = if server.active { "active" } else { "inactive" };
        Line::from(format!(
            "{state}  {}  {} tools",
            server.name,
            server.tools.len()
        ))
    }

    #[must_use]
    pub fn mcp_failure_row(failure: &McpFailure) -> Line<'static> {
        Line::from(format!("failed  {}  {}", failure.name, failure.reason))
    }

    #[must_use]
    pub fn tool_rule_row(rule: &ToolRule) -> Line<'static> {
        let decision = if rule.allowed { "allowed" } else { "denied" };
        Line::from(format!("{decision}  {}  {}", rule.name, rule.scope))
    }

    pub(crate) fn optional_limit(limit: Option<u64>) -> String {
        limit.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
    }

    #[must_use]
    pub fn model_provider_row(
        name: &str,
        model_count: usize,
        expanded: bool,
        signed_in: bool,
    ) -> Line<'static> {
        let marker = if expanded { "▾" } else { "▸" };
        let key = if signed_in { "✓ " } else { "  " };
        Line::from(format!("{marker}{key}{name}  ·  {model_count} models"))
    }

    #[must_use]
    pub fn model_option_row(model: &ModelInfo, marker: Option<&str>) -> Line<'static> {
        let suffix = match (marker, model.deprecated) {
            (Some(marker), true) => format!("  {marker}  deprecated"),
            (Some(marker), false) => format!("  {marker}"),
            (None, true) => "  deprecated".to_owned(),
            (None, false) => String::new(),
        };
        Line::from(format!("    {}  ({}){suffix}", model.name, model.model))
    }
}
