//! The status line: what the session is doing, not what the keys do.
//!
//! It shows the two choices that change behavior (queue mode and reasoning),
//! a gauge of the context window, the point where compaction is expected and
//! the state of the turn. Keys live in the menu and in the help modal.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::components::{ComponentId, palette, palette::GaugeBand};
use crate::text_builder::TextBuilder;
use crate::ui_model::{ContextGauge, ThemeMode, TurnState, UiModel};

/// The cells the gauge is drawn with.
const GAUGE_CELLS: usize = 10;

/// The line the footer shows.
#[must_use]
pub(crate) fn status_line(model: &UiModel, active: ComponentId, width: u16) -> Line<'static> {
    let theme = model.preferences.theme;
    let mut spans = Vec::new();
    let separator = Span::styled("  ", Style::default().fg(palette::border(theme)));

    spans.push(Span::styled(
        format!("queue {}", model.send_mode.label()),
        Style::default().fg(palette::accent(theme)),
    ));
    spans.push(separator.clone());
    spans.push(Span::styled(
        format!("reasoning {}", model.reasoning.label()),
        Style::default().fg(palette::accent(theme)),
    ));

    if let Some(gauge) = ContextGauge::of(model.session_stats.as_ref()) {
        spans.push(separator.clone());
        spans.extend(gauge_spans(gauge, theme));
    }

    spans.push(separator);
    spans.push(Span::styled(
        turn_label(active, model.turn_state),
        turn_style(active, model.turn_state, theme),
    ));

    let line = Line::from(spans);
    TextBuilder::fit_line(line, width)
}

/// The gauge: the bar, the percentage and the compaction point.
fn gauge_spans(gauge: ContextGauge, theme: ThemeMode) -> Vec<Span<'static>> {
    let fraction = gauge.fraction();
    let band = GaugeBand::of(fraction);
    let filled = gauge.filled_cells(GAUGE_CELLS);
    let used = TextBuilder::human_count(gauge.used);
    let window = TextBuilder::human_count(gauge.window);
    let percent = gauge.percent();

    let mut spans = vec![
        Span::styled("█".repeat(filled), Style::default().fg(band.color())),
        Span::styled(
            "░".repeat(GAUGE_CELLS - filled),
            Style::default().fg(palette::border(theme)),
        ),
        Span::styled(
            format!(" {percent}% {used}/{window}"),
            Style::default().fg(band.color()),
        ),
    ];
    let limit = TextBuilder::human_count(gauge.compaction_limit());
    spans.push(if gauge.over_limit() {
        Span::styled(
            format!("  compacta {limit}"),
            Style::default()
                .fg(palette::danger())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("  compacta {limit}"),
            Style::default().fg(palette::text_muted(theme)),
        )
    });
    spans
}

/// What the session is doing, in one word.
fn turn_label(active: ComponentId, state: TurnState) -> String {
    if active != ComponentId::Session {
        return TextBuilder::panel_title_for(active).to_owned();
    }
    match state {
        TurnState::Idle => "ready".to_owned(),
        TurnState::Connecting => "connecting".to_owned(),
        TurnState::Thinking => "thinking".to_owned(),
        TurnState::Streaming => "working".to_owned(),
        TurnState::AwaitingAnswer => "waiting for your answer".to_owned(),
    }
}

/// The color the turn state is drawn with.
fn turn_style(active: ComponentId, state: TurnState, theme: ThemeMode) -> Style {
    if active != ComponentId::Session {
        return Style::default().fg(palette::text_muted(theme));
    }
    match state {
        TurnState::Idle => Style::default().fg(palette::success()),
        TurnState::Connecting | TurnState::Thinking => Style::default().fg(palette::warning()),
        TurnState::Streaming => Style::default()
            .fg(palette::accent(theme))
            .add_modifier(Modifier::BOLD),
        TurnState::AwaitingAnswer => Style::default().fg(palette::info()),
    }
}
