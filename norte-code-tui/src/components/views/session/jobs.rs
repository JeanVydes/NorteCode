//! The panel that shows the session's background jobs.
//!
//! A job is a tool call the host let run on its own, so it is not part of the
//! conversation: it gets a panel of its own, where the reader sees what is
//! running and can stop it.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph},
};

use norte_code_wire::{JobInfo, JobState};

use crate::{components::palette, ui_model::ThemeMode};

/// The most jobs the panel shows.
const MAX_JOBS: usize = 5;

/// The panel that shows the running jobs.
pub(super) struct JobsPanel;

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;

impl JobsPanel {
    /// The title of the panel: how many jobs are running.
    pub(super) fn title(jobs: &[JobInfo]) -> String {
        let running = jobs
            .iter()
            .filter(|job| job.state == JobState::Running)
            .count();
        format!("jobs {running}/{}", jobs.len())
    }

    /// The lines the panel draws: one per running job, with its last output.
    pub(super) fn lines(jobs: &[JobInfo]) -> Vec<Line<'static>> {
        jobs.iter()
            .filter(|job| job.state == JobState::Running)
            .take(MAX_JOBS)
            .map(|job| {
                let mut spans = vec![
                    Span::styled("▶ ", Style::default().fg(palette::warning())),
                    Span::raw(Self::name(job)),
                ];
                if let Some(tail) = Self::output_tail(job) {
                    spans.push(Span::styled(
                        format!(" · {tail}"),
                        Style::default().fg(palette::muted()),
                    ));
                }
                Line::from(spans)
            })
            .collect()
    }

    /// How many rows the panel needs, title included.
    pub(super) fn height(jobs: &[JobInfo]) -> u16 {
        let running = jobs
            .iter()
            .filter(|job| job.state == JobState::Running)
            .count();
        if running == 0 {
            0
        } else {
            u16::try_from(running.min(MAX_JOBS) + 1).unwrap_or(u16::MAX)
        }
    }

    /// Draws the panel, when there is something to show.
    pub(super) fn render(frame: &mut Frame<'_>, area: Rect, jobs: &[JobInfo], theme: ThemeMode) {
        let lines = Self::lines(jobs);
        if lines.is_empty() {
            return;
        }
        frame.render_widget(
            Paragraph::new(ratatui::text::Text::from(lines)).block(
                Block::default()
                    .title(Self::title(jobs))
                    .padding(Padding::horizontal(1))
                    .style(palette::panel_style(theme)),
            ),
            area,
        );
    }

    /// The lines the picker draws for the running jobs.
    pub(super) fn picker_lines(jobs: &[JobInfo], theme: ThemeMode) -> Vec<Line<'static>> {
        Self::running(jobs)
            .iter()
            .enumerate()
            .map(|(index, job)| {
                Line::from(vec![
                    Span::styled(
                        format!("{} ", index + 1),
                        Style::default().fg(palette::accent(theme)),
                    ),
                    Span::raw(Self::name(job)),
                ])
            })
            .collect()
    }

    /// The running jobs, in the order the picker offers them.
    pub(super) fn running(jobs: &[JobInfo]) -> Vec<JobInfo> {
        jobs.iter()
            .filter(|job| job.state == JobState::Running)
            .cloned()
            .collect()
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

    fn output_tail(job: &JobInfo) -> Option<String> {
        let last = job
            .output
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())?;
        let trimmed = last.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self::shorten(trimmed, 60))
    }

    fn shorten(text: &str, limit: usize) -> String {
        if text.chars().count() <= limit {
            return text.to_owned();
        }
        let mut short = text.chars().take(limit).collect::<String>();
        short.push('…');
        short
    }
}
