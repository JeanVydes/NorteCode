use ratatui::{
    style::Style,
    text::{Line, Span},
};

use norte_code_client::{CostReport, PluginSummary, ServerInfo, ToolRule};
use norte_code_wire::{
    ChildSessionSummary, McpFailure, McpInfo, ModelInfo, PlanSummary, SessionSummary, SkillInfo,
    TelemetryRecord, TelemetrySummary,
};

use super::TextBuilder;
use crate::{
    components::palette,
    ui_model::{TelemetryScope, UiModel},
};

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
            Style::default().fg(palette::brand()),
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
            Style::default().fg(palette::muted()),
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
                Style::default().fg(palette::brand()),
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
                Style::default().fg(palette::muted()),
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
            Style::default().fg(palette::muted())
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

    #[must_use]
    pub fn plan_detail(plan: &PlanSummary, show_details: bool) -> String {
        if show_details {
            format!(
                "{}\n\nPageUp/PageDown scroll · {} toggles summary",
                plan.content,
                Self::submit_key_label()
            )
        } else {
            format!(
                "{}\n\nPress {} to toggle full plan text",
                plan.title.as_deref().unwrap_or(&plan.id),
                Self::submit_key_label()
            )
        }
    }

    #[must_use]
    pub fn telemetry_row(record: &TelemetryRecord) -> Line<'static> {
        Line::from(format!(
            "{}  {} / {}  {} ms",
            record.started_at, record.provider, record.model, record.duration_ms
        ))
    }

    #[must_use]
    pub fn telemetry_summary(summary: &TelemetrySummary) -> String {
        format!(
            "requests     {}\nerrors       {}\ncancelled    {}\nduration     {} ms\ninput        {}\noutput       {}\ncache read   {}\ncache write  {}",
            summary.requests,
            summary.errors,
            summary.cancelled,
            summary.duration_ms,
            summary.input_tokens,
            summary.output_tokens,
            summary.cache_read_tokens,
            summary.cache_write_tokens
        )
    }

    #[must_use]
    pub fn plugin_row(plugin: &PluginSummary) -> Line<'static> {
        let enabled = if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let loaded = if plugin.loaded {
            "loaded"
        } else {
            "not loaded"
        };
        Line::from(format!("{enabled}  {loaded}  {}", plugin.name))
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

    #[must_use]
    pub fn host_overview(model: &UiModel) -> String {
        let server = model
            .server
            .as_ref()
            .map_or_else(|| Self::host_unavailable().to_owned(), Self::host_details);
        let liveness = model
            .liveness
            .as_ref()
            .map_or("unknown", |health| match health.status {
                norte_code_client::HealthStatus::Live => "live",
                norte_code_client::HealthStatus::Ready => "ready",
            });
        let readiness = model
            .readiness
            .as_ref()
            .map_or("unknown", |health| match health.status {
                norte_code_client::HealthStatus::Live => "live",
                norte_code_client::HealthStatus::Ready => "ready",
            });
        let agents = model
            .agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let quota = model.quota.as_ref().map_or_else(
            || "not loaded".to_owned(),
            |quota| {
                quota
                    .dimensions
                    .iter()
                    .map(|dimension| {
                        format!(
                            "{} {}/{}",
                            dimension.name.label(),
                            dimension.used,
                            dimension.limit
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        );
        let capabilities = model
            .server
            .as_ref()
            .map_or_else(String::new, |server| server.capabilities.join(", "));
        let extensions = model
            .server
            .as_ref()
            .map_or_else(String::new, |server| server.extensions.join(", "));
        let limits = model.server.as_ref().map_or_else(
            || "not loaded".to_owned(),
            |server| {
                format!(
                    "frame {} · jobs {} · artifact {} · prompt {} bytes",
                    Self::optional_limit(server.limits.frame_bytes),
                    Self::optional_limit(server.limits.concurrent_jobs),
                    Self::optional_limit(server.limits.artifact_bytes),
                    Self::optional_limit(server.limits.prompt_bytes),
                )
            },
        );
        let host_errors = model
            .host_errors
            .iter()
            .map(|(probe, error)| format!("{probe}: {error}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "endpoint        {}\nauthentication  {}\n{server}\nliveness        {liveness}\nreadiness       {readiness}\nagents          {agents}\nquota           {quota}\ncapabilities    {capabilities}\nextensions      {extensions}\nlimits          {limits}\n\n{}",
            model.endpoint,
            if model.authentication_configured {
                "configured"
            } else {
                "not configured"
            },
            if host_errors.is_empty() {
                "Tab changes section | r refreshes".to_owned()
            } else {
                format!("Probe failures:\n{host_errors}\n\nTab changes section | r refreshes")
            }
        )
    }

    #[must_use]
    pub fn cost_reports(
        global: Option<&CostReport>,
        session: Option<&CostReport>,
        session_id: Option<&norte_code_wire::SessionKey>,
    ) -> String {
        let global = Self::cost_report_line("all sessions", global);
        let session_title = session_id.map_or("current session", |id| id.as_str());
        let session = Self::cost_report_line(session_title, session);
        format!("{global}\n{session}\n\nPress r to refresh")
    }

    fn cost_report_line(label: &str, report: Option<&CostReport>) -> String {
        let total = report.map_or("not loaded".to_owned(), |report| {
            report.total_usd.map_or_else(
                || "not reported".to_owned(),
                |amount| format!("${amount:.6}"),
            )
        });
        let requests = report.map_or_else(|| "-".to_owned(), |report| report.requests.to_string());
        format!("{label}\n  cost: {total}\n  requests: {requests}")
    }

    #[must_use]
    pub fn telemetry_scope(scope: TelemetryScope) -> &'static str {
        match scope {
            TelemetryScope::Host => "all sessions",
            TelemetryScope::Session => "current session",
        }
    }

    #[must_use]
    pub fn telemetry_record_detail(record: &TelemetryRecord) -> String {
        let optional = |value: Option<u64>| {
            value.map_or_else(|| "not reported".to_owned(), |value| value.to_string())
        };
        let cost = record
            .cost_usd
            .map_or_else(|| "not reported".to_owned(), |value| format!("${value:.6}"));
        let endpoint = record.endpoint.as_deref().unwrap_or("not reported");
        let error = record.error.as_deref().unwrap_or("none");
        let tool_calls = if record.tool_calls.is_empty() {
            "none".to_owned()
        } else {
            record.tool_calls.join(", ")
        };
        format!(
            "request       {}\nsession       {}\nturn          {}\nprovider      {}\nmodel         {}\nstarted       {}\nendpoint      {}\nstatus        {:?}\nduration      {} ms\nfirst token   {} ms\nlast token    {} ms\ninput         {}\noutput        {}\nreasoning     {}\ncache read    {}\ncache write   {}\ncost          {}\ncache hit     {}\ntool calls    {}\nerror         {}",
            record.id,
            record.session,
            record.turn,
            record.provider,
            record.model,
            record.started_at,
            endpoint,
            record.status,
            record.duration_ms,
            optional(record.first_token_ms),
            optional(record.last_token_ms),
            optional(record.input_tokens),
            optional(record.output_tokens),
            optional(record.reasoning_tokens),
            optional(record.cache_read_tokens),
            optional(record.cache_write_tokens),
            cost,
            record.cache_hit,
            tool_calls,
            error,
        )
    }

    fn optional_limit(limit: Option<u64>) -> String {
        limit.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
    }

    #[must_use]
    pub fn model_provider_row(name: &str, model_count: usize, expanded: bool) -> Line<'static> {
        let marker = if expanded { "▾" } else { "▸" };
        Line::from(format!("{marker} {name}  ·  {model_count} models"))
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

    #[must_use]
    pub fn host_details(server: &ServerInfo) -> String {
        let protocol_name = server
            .protocol
            .as_ref()
            .map_or("", |protocol| &protocol.name);
        let protocol_version = server
            .protocol
            .as_ref()
            .map_or("", |protocol| &protocol.version);
        format!(
            "server version  {}\ndefault agent   {}\nprotocol        {protocol_name} {protocol_version}",
            server.server_version, server.default_agent
        )
    }
}
