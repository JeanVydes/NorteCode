//! The overview section of the host view.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{
    components::palette,
    text_builder::TextBuilder,
    ui_model::{ThemeMode, UiModel},
};

/// The width the value column starts at.
const LABEL_WIDTH: usize = 16;

/// The overview: health, connection, then what the host can do.
#[allow(clippy::too_many_lines)]
pub(crate) fn overview(model: &UiModel, theme: ThemeMode) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(section("health", theme));
    lines.push(Line::default());
    lines.push(badges(model, theme));
    lines.push(Line::default());

    lines.push(section("connection", theme));
    lines.push(Line::default());
    lines.push(row(
        "endpoint",
        model.endpoint.clone(),
        palette::text(theme),
        theme,
    ));
    lines.push(row(
        "authentication",
        if model.authentication_configured {
            "configured".to_owned()
        } else {
            "not configured".to_owned()
        },
        if model.authentication_configured {
            palette::success()
        } else {
            palette::warning()
        },
        theme,
    ));
    if let Some(server) = model.server.as_ref() {
        lines.push(row(
            "server",
            format!("v{}", server.server_version),
            palette::text(theme),
            theme,
        ));
        lines.push(row(
            "default agent",
            server.default_agent.clone(),
            palette::text(theme),
            theme,
        ));
        if let Some(protocol) = server.protocol.as_ref() {
            lines.push(row(
                "protocol",
                format!("{} {}", protocol.name, protocol.version),
                palette::text_muted(theme),
                theme,
            ));
        }
    } else {
        lines.push(row(
            "server",
            "not loaded".to_owned(),
            palette::text_muted(theme),
            theme,
        ));
    }
    lines.push(Line::default());

    lines.push(section("agents", theme));
    lines.push(Line::default());
    if model.agents.is_empty() {
        lines.push(row(
            "agents",
            "none reported".to_owned(),
            palette::text_muted(theme),
            theme,
        ));
    } else {
        for agent in &model.agents {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    agent.id.clone(),
                    Style::default()
                        .fg(palette::accent(theme))
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }
    lines.push(Line::default());

    lines.push(section("capabilities", theme));
    lines.push(Line::default());
    match model.server.as_ref() {
        Some(server) if !server.capabilities.is_empty() => {
            lines.push(chips(&server.capabilities, palette::info(), theme));
        }
        _ => lines.push(row(
            "capabilities",
            "none reported".to_owned(),
            palette::text_muted(theme),
            theme,
        )),
    }
    if let Some(server) = model.server.as_ref()
        && !server.extensions.is_empty()
    {
        lines.push(chips(&server.extensions, palette::success(), theme));
    }
    lines.push(Line::default());

    lines.push(section("limits", theme));
    lines.push(Line::default());
    if let Some(server) = model.server.as_ref() {
        lines.push(row(
            "frame",
            TextBuilder::optional_limit(server.limits.frame_bytes),
            palette::text(theme),
            theme,
        ));
        lines.push(row(
            "jobs",
            TextBuilder::optional_limit(server.limits.concurrent_jobs),
            palette::text(theme),
            theme,
        ));
        lines.push(row(
            "artifact",
            TextBuilder::optional_limit(server.limits.artifact_bytes),
            palette::text(theme),
            theme,
        ));
        lines.push(row(
            "prompt",
            TextBuilder::optional_limit(server.limits.prompt_bytes),
            palette::text(theme),
            theme,
        ));
    } else {
        lines.push(row(
            "limits",
            "not loaded".to_owned(),
            palette::text_muted(theme),
            theme,
        ));
    }

    if !model.host_errors.is_empty() {
        lines.push(Line::default());
        lines.push(section("probe failures", theme));
        lines.push(Line::default());
        for (probe, error) in &model.host_errors {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!("{probe}  "),
                    Style::default()
                        .fg(palette::danger())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(error.clone(), palette::muted_style(theme)),
            ]));
        }
    }
    lines
}

/// The health and quota badges, on one line.
fn badges(model: &UiModel, theme: ThemeMode) -> Line<'static> {
    let mut spans = Vec::new();
    let health = |label: &str, value: Option<norte_code_client::HealthStatus>| {
        let (text, color) = match value {
            Some(_) => (label.to_owned(), palette::success()),
            None => ("unknown".to_owned(), palette::text_muted(theme)),
        };
        (text, color)
    };
    let liveness = health("live", model.liveness.as_ref().map(|health| health.status));
    let readiness = health(
        "ready",
        model.readiness.as_ref().map(|health| health.status),
    );
    for (text, color) in [liveness, readiness] {
        spans.push(Span::styled(
            format!(" {text} "),
            Style::default()
                .fg(theme_background(theme))
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    }
    if let Some(quota) = model.quota.as_ref() {
        for dimension in &quota.dimensions {
            // A gauge band only needs the fraction, and a quota above 2^53
            // is not a limit any provider declares.
            #[allow(clippy::cast_precision_loss)]
            let ratio = if dimension.limit == 0 {
                0.0
            } else {
                dimension.used as f64 / dimension.limit as f64
            };
            let color = palette::GaugeBand::of(ratio).color();
            spans.push(Span::styled(
                format!(
                    " {} {}/{} ",
                    dimension.name.label(),
                    dimension.used,
                    dimension.limit
                ),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        }
    }
    Line::from(spans)
}

/// A bold caption for one section.
fn section(name: &str, theme: ThemeMode) -> Line<'static> {
    Line::from(Span::styled(
        name.to_uppercase(),
        Style::default()
            .fg(palette::accent(theme))
            .add_modifier(Modifier::BOLD),
    ))
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
            format!("  {label:<LABEL_WIDTH$}"),
            palette::muted_style(theme),
        ),
        Span::styled(value, Style::default().fg(color)),
    ])
}

/// A run of names as small boxes.
fn chips(names: &[String], color: ratatui::style::Color, theme: ThemeMode) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")];
    for name in names {
        spans.push(Span::styled(
            format!(" {name} "),
            Style::default().fg(color).bg(palette::surface_alt(theme)),
        ));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// The color text takes over the accent badges.
const fn theme_background(theme: ThemeMode) -> ratatui::style::Color {
    palette::background(theme)
}
