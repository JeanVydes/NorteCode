//! Picking a user turn to rewind to or to pin.
//!
//! Both actions name a turn, and the reader picks it from the list of the
//! turns they wrote, newest first.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{
    components::palette,
    ui_model::{ThemeMode, TurnPoint},
};

/// The most turns the picker offers.
const MAX_ENTRIES: usize = 9;

/// What the picker is choosing a turn for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TurnPickerMode {
    Rewind,
    Pin,
}

impl TurnPickerMode {
    pub(super) const fn title(self) -> &'static str {
        match self {
            Self::Rewind => "rewind to",
            Self::Pin => "pin or unpin",
        }
    }
}

/// The entries the picker offers for the turns.
pub(super) struct TurnPickerPanel;

impl TurnPickerPanel {
    /// The lines the panel draws, newest turn first.
    pub(super) fn lines(
        points: &[TurnPoint],
        pinned: &[String],
        theme: ThemeMode,
    ) -> Vec<Line<'static>> {
        points
            .iter()
            .rev()
            .take(MAX_ENTRIES)
            .enumerate()
            .map(|(index, point)| {
                let mark = if pinned.iter().any(|turn| turn == &point.turn_id) {
                    " (pinned)"
                } else {
                    ""
                };
                Line::from(vec![
                    Span::styled(
                        format!("{} ", index + 1),
                        Style::default().fg(palette::accent(theme)),
                    ),
                    Span::raw(point.preview.clone()),
                    Span::styled(mark.to_owned(), Style::default().fg(palette::muted())),
                ])
            })
            .collect()
    }

    /// The turns the picker offers.
    pub(super) fn offered(points: &[TurnPoint]) -> Vec<TurnPoint> {
        points.iter().rev().take(MAX_ENTRIES).cloned().collect()
    }
}
