//! Where the money went.
//!
//! The ledger only reports a total and a request count, so a page that showed
//! just those two numbers would say very little. The breakdown per model comes
//! from the telemetry rows the host already keeps, and the ledger's totals sit
//! above it as the receipt.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{
    components::{
        Component, ComponentAction, ComponentId, ComponentLifecycle, ViewAction, palette, panel,
    },
    text_builder::TextBuilder,
    ui_model::{ThemeMode, UiModel},
};

#[derive(Default)]
pub(in crate::components) struct CostsView {
    lifecycle: ComponentLifecycle,
    pending_action: Option<ComponentAction>,
}

impl Component for CostsView {
    fn id(&self) -> ComponentId {
        ComponentId::Costs
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn handle_key(&mut self, key: KeyEvent, _model: &UiModel) {
        if key.code == KeyCode::Char('r') {
            self.pending_action = Some(ComponentAction::View(ViewAction::RefreshCosts));
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let theme = model.preferences.theme;
        let lines = lines_for(model, theme);
        panel::render_lines(
            frame,
            area,
            TextBuilder::panel_title_for(ComponentId::Costs),
            lines,
            theme,
        );
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}

/// Everything the page shows, top to bottom.
fn lines_for(model: &UiModel, theme: ThemeMode) -> Vec<Line<'static>> {
    let mut lines = vec![
        section("ledger", theme),
        Line::default(),
        figure(
            "all sessions",
            model.costs.as_ref().and_then(|report| report.total_usd),
            model.costs.as_ref().map_or(0, |report| report.requests),
            theme,
        ),
        figure(
            "this session",
            model
                .session_costs
                .as_ref()
                .and_then(|report| report.total_usd),
            model
                .session_costs
                .as_ref()
                .map_or(0, |report| report.requests),
            theme,
        ),
    ];

    let rows = model
        .telemetry
        .as_ref()
        .map(|report| report.records.as_slice())
        .unwrap_or_default();
    if rows.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "No request has been recorded yet; the breakdown needs telemetry.",
            palette::muted_style(theme),
        )));
        return lines;
    }

    let totals = Totals::of(rows);
    lines.push(Line::default());
    lines.push(section("totals", theme));
    lines.push(Line::default());
    lines.push(metric(
        "requests",
        totals.requests.to_string(),
        palette::text(theme),
        theme,
    ));
    lines.push(metric(
        "cost",
        format!("${:.4}", totals.cost),
        palette::warning(),
        theme,
    ));
    lines.push(metric(
        "tokens",
        format!(
            "{} in · {} out",
            TextBuilder::human_count(totals.input),
            TextBuilder::human_count(totals.output)
        ),
        palette::text(theme),
        theme,
    ));

    lines.push(Line::default());
    lines.push(section("by model", theme));
    lines.push(Line::default());
    for row in totals.by_model() {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<28}", row.name), palette::text_style(theme)),
            Span::styled(
                format!("{:>9}", format!("${:.4}", row.cost)),
                Style::default().fg(palette::warning()),
            ),
            Span::styled(
                format!("  {} req", row.requests),
                palette::muted_style(theme),
            ),
        ]));
    }
    lines
}

/// The totals the telemetry rows add up to.
struct Totals {
    requests: u64,
    input: u64,
    output: u64,
    cost: f64,
    models: Vec<ModelTotal>,
}

struct ModelTotal {
    name: String,
    cost: f64,
    requests: u64,
}

impl Totals {
    fn of(rows: &[norte_code_wire::TelemetryRecord]) -> Self {
        let mut totals = Self {
            requests: 0,
            input: 0,
            output: 0,
            cost: 0.0,
            models: Vec::new(),
        };
        for row in rows {
            totals.requests = totals.requests.saturating_add(1);
            totals.input = totals.input.saturating_add(row.input_tokens.unwrap_or(0));
            totals.output = totals.output.saturating_add(row.output_tokens.unwrap_or(0));
            let cost = row.cost_usd.unwrap_or(0.0);
            totals.cost += cost;
            let name = format!("{}/{}", row.provider, row.model);
            match totals.models.iter_mut().find(|model| model.name == name) {
                Some(model) => {
                    model.cost += cost;
                    model.requests = model.requests.saturating_add(1);
                }
                None => totals.models.push(ModelTotal {
                    name,
                    cost,
                    requests: 1,
                }),
            }
        }
        totals
    }

    /// The models from the most expensive to the cheapest.
    fn by_model(mut self) -> Vec<ModelTotal> {
        self.models
            .sort_by(|left, right| right.cost.total_cmp(&left.cost));
        self.models
    }
}

/// A bold caption for one block.
fn section(name: &str, theme: ThemeMode) -> Line<'static> {
    Line::from(Span::styled(
        name.to_uppercase(),
        Style::default()
            .fg(palette::accent(theme))
            .add_modifier(Modifier::BOLD),
    ))
}

/// One label and its value.
fn metric(
    label: &str,
    value: String,
    color: ratatui::style::Color,
    theme: ThemeMode,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label:<14}"), palette::muted_style(theme)),
        Span::styled(value, Style::default().fg(color)),
    ])
}

/// A ledger line: the label, the money and the count.
fn figure(label: &str, total: Option<f64>, requests: u64, theme: ThemeMode) -> Line<'static> {
    let value = total.map_or_else(|| "not reported".to_owned(), |total| format!("${total:.4}"));
    Line::from(vec![
        Span::styled(format!("  {label:<14}"), palette::muted_style(theme)),
        Span::styled(
            format!("{value:<16}"),
            Style::default().fg(palette::warning()),
        ),
        Span::styled(format!("{requests} requests"), palette::muted_style(theme)),
    ])
}

#[cfg(test)]
#[path = "costs_tests.rs"]
mod tests;
