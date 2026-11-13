use ratatui::text::Line;

use norte_code_client::{CostReport, PluginSummary, ServerInfo, ToolRule};
use norte_code_wire::{
    ChildSessionSummary, McpFailure, McpInfo, ModelInfo, PlanSummary, SessionSummary, SkillInfo,
    TelemetryRecord, TelemetrySummary,
};

use super::TextBuilder;
use crate::ui_model::{TelemetryScope, UiModel};

impl TextBuilder {
    #[must_use]
    pub fn numbered_session_row(number: usize, session: &SessionSummary) -> Line<'static> {
        Line::from(format!(
            "{number:>2}.  {}  {}",
            session.title.as_deref().unwrap_or("Untitled session"),
            session.session_id
        ))
    }

    #[must_use]
    pub fn numbered_child_session_row(
        number: usize,
        session: &ChildSessionSummary,
    ) -> Line<'static> {
        Line::from(format!(
            "{number:>2}.  {}  {}  {}",
            session.title.as_deref().unwrap_or("subagent"),
            session.state.label(),
            session.task.as_deref().unwrap_or(&session.session_id)
        ))
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
            plan.content.clone()
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
    pub fn model_option_row(model: &ModelInfo) -> Line<'static> {
        let suffix = if model.deprecated { "  deprecated" } else { "" };
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
