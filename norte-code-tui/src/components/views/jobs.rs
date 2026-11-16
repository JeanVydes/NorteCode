//! The page that lists the session's background jobs.
//!
//! A job is a tool call the host let run on its own, so it is not part of the
//! conversation; the reader finds it here, sees how it ended and can stop it.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Padding},
};

use norte_code_wire::{JobInfo, JobState};

use crate::{
    components::{Component, ComponentAction, ComponentId, ComponentLifecycle, ViewAction},
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

/// The most output bytes the second line shows.
const OUTPUT_TAIL: usize = 100;

#[derive(Default)]
pub(in crate::components) struct JobsView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    pending_action: Option<ComponentAction>,
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;

impl Component for JobsView {
    fn id(&self) -> ComponentId {
        ComponentId::Jobs
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        if key.code == KeyCode::Char('r') {
            self.pending_action = Some(ComponentAction::View(ViewAction::RefreshJobs));
            return;
        }
        if key.code == KeyCode::Char(keys::STOP_JOB_CHAR) {
            self.stop_selected(model);
            return;
        }
        if key.code == keys::NAV_DOWN || key.code == keys::NAV_DOWN_ARROW {
            let len = model.jobs.len();
            self.selected = if len == 0 {
                0
            } else {
                self.selected.saturating_add(1).min(len - 1)
            };
            return;
        }
        if key.code == keys::NAV_UP || key.code == keys::NAV_UP_ARROW {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let theme = model.preferences.theme;
        let items = model
            .jobs
            .iter()
            .map(|job| ListItem::new(Self::lines(job)))
            .collect::<Vec<_>>();
        let block = Block::default()
            .title(TextBuilder::panel_title(
                ComponentId::Jobs,
                self.lifecycle.phase(),
            ))
            .style(crate::components::palette::panel_style(theme))
            .padding(Padding::horizontal(2));
        if items.is_empty() {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(TextBuilder::empty_message(ComponentId::Jobs))
                    .style(crate::components::palette::panel_style(theme))
                    .block(block),
                area,
            );
            return;
        }
        // The selection can outlive a refresh that dropped the job it was on.
        self.selected = self.selected.min(model.jobs.len() - 1);
        let mut state = ListState::default();
        state.select(Some(self.selected));
        frame.render_stateful_widget(
            List::new(items)
                .style(crate::components::palette::panel_style(theme))
                .block(block)
                .highlight_symbol(TextBuilder::SELECTED_MARKER)
                .highlight_style(
                    crate::components::palette::selected_style(theme).add_modifier(Modifier::BOLD),
                ),
            area,
            &mut state,
        );
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}

impl JobsView {
    /// Asks the host to stop the job the reader picked, when it still runs.
    fn stop_selected(&mut self, model: &UiModel) {
        let Some(job) = model.jobs.get(self.selected) else {
            return;
        };
        if job.state != JobState::Running {
            return;
        }
        self.pending_action = Some(ComponentAction::StopJob {
            job_id: job.id.clone(),
        });
    }

    /// The two lines a job is drawn with: its row and, when it wrote
    /// something, its last output.
    fn lines(job: &JobInfo) -> Vec<Line<'static>> {
        let mut lines = vec![Self::row(job)];
        if let Some(tail) = Self::output_tail(job) {
            lines.push(Line::from(Span::styled(
                format!("  {tail}"),
                Style::default().fg(crate::components::palette::muted()),
            )));
        }
        lines
    }

    /// The head row: what it is, how it stands and how long it ran.
    fn row(job: &JobInfo) -> Line<'static> {
        let (marker, marker_color) = match job.state {
            JobState::Running => ("▶", crate::components::palette::warning()),
            JobState::Finished => match job.exit_code {
                Some(0) => ("✓", crate::components::palette::success()),
                _ => ("✗", crate::components::palette::danger()),
            },
            JobState::Cancelled => ("■", crate::components::palette::muted()),
        };
        let state = match job.state {
            JobState::Running => "running".to_owned(),
            JobState::Finished => match job.exit_code {
                Some(code) => format!("finished · exit {code}"),
                None => "finished".to_owned(),
            },
            JobState::Cancelled => "stopped".to_owned(),
        };
        Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(marker_color)),
            Span::raw(Self::name(job)),
            Span::styled(
                format!("  {state} · {}", Self::elapsed(job.elapsed_ms)),
                Style::default().fg(crate::components::palette::muted()),
            ),
        ])
    }

    /// How long the host says the job is running or took.
    fn elapsed(milliseconds: u64) -> String {
        const SECOND: u64 = 1_000;
        const MINUTE: u64 = 60 * SECOND;
        if milliseconds < SECOND {
            format!("{milliseconds}ms")
        } else if milliseconds < MINUTE {
            format!(
                "{}.{}s",
                milliseconds / SECOND,
                (milliseconds % SECOND + 50) / 100
            )
        } else {
            format!(
                "{}m{:02}s",
                milliseconds / MINUTE,
                (milliseconds % MINUTE) / SECOND
            )
        }
    }

    fn name(job: &JobInfo) -> String {
        if !job.label.is_empty() {
            return job.label.clone();
        }
        if !job.script.is_empty() {
            return job.script.clone();
        }
        job.id.clone()
    }

    /// The last non-empty line the job wrote, as one line.
    fn output_tail(job: &JobInfo) -> Option<String> {
        let last = job
            .output
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())?;
        let trimmed = last.trim();
        let mut short = trimmed.chars().take(OUTPUT_TAIL).collect::<String>();
        if trimmed.chars().count() > OUTPUT_TAIL {
            short.push('…');
        }
        Some(short)
    }
}
