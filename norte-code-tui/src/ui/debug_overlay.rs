//! A floating box with frame loop, event and memory metrics.

use std::any::Any;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;
use terminalui::components::chrome::{Chrome, Padding, draw_box};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

use crate::frame_limiter::FRAME_RATE_CAP;
use crate::metrics::{MetricsSnapshot, Phase};

const WIDTH: u16 = 58;
const HEIGHT: u16 = 18;
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// Paces overlay repaints without forcing redraws in normal use.
#[derive(Debug)]
pub(crate) struct OverlayClock {
    last: Option<Instant>,
}

impl Default for OverlayClock {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayClock {
    pub(crate) fn new() -> Self {
        Self { last: None }
    }

    pub(crate) fn due(&mut self, now: Instant, enabled: bool) -> bool {
        if !enabled
            || self
                .last
                .is_some_and(|last| now.duration_since(last) < REFRESH_INTERVAL)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// The read-only performance panel.
#[derive(Serialize)]
pub(crate) struct DebugOverlay {
    enabled: bool,
    #[serde(skip)]
    snapshot: MetricsSnapshot,
}

impl DebugOverlay {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            snapshot: MetricsSnapshot::default(),
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub(crate) fn set_snapshot(&mut self, snapshot: &MetricsSnapshot) {
        self.snapshot = *snapshot;
    }

    fn lines(&self) -> Vec<String> {
        let snapshot = &self.snapshot;
        let phase = |step| format_timing(step, snapshot.phases.get(step));
        let state = if snapshot.active { "active" } else { "idle" };
        let age = snapshot
            .last_draw_age
            .map_or_else(|| "never".to_owned(), format_duration);
        let mut lines = vec![
            format!(
                "fps       draw {:.1}  loop {:.1}  cap {}",
                snapshot.drawn_fps, snapshot.loop_fps, FRAME_RATE_CAP
            ),
            phase(Phase::Draw),
            phase(Phase::Drain),
            phase(Phase::Refresh),
            phase(Phase::SyncChrome),
            phase(Phase::Tick),
            phase(Phase::Poll),
            phase(Phase::Process),
            format!(
                "frames    {} drawn  {} skipped",
                snapshot.frames_drawn, snapshot.frames_skipped
            ),
            format!(
                "events    {} srv  {} in  {} idle polls",
                snapshot.server_events, snapshot.input_events, snapshot.poll_timeouts
            ),
            format!(
                "state     {}  queue {}  last {}",
                state, snapshot.queued_events, age
            ),
            format!(
                "rss       {}",
                snapshot.process.map_or_else(
                    || "sampling".to_owned(),
                    |usage| format!(
                        "{:.1} MiB ({:+.1})",
                        usage.memory.current.mib(),
                        usage.memory.current.delta_mib(usage.memory.baseline)
                    ),
                )
            ),
            format!(
                "cpu       {}",
                snapshot.process.map_or_else(
                    || "sampling".to_owned(),
                    |usage| format!("{:.1}% avg", usage.cpu_percent),
                )
            ),
            format!("uptime    {}", format_duration(snapshot.uptime)),
        ];
        lines.truncate(usize::from(HEIGHT.saturating_sub(3)));
        lines
    }
}

impl View for DebugOverlay {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "debug_overlay"
    }
    fn focusable(&self) -> bool {
        false
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        if !self.enabled || area.width < 20 || area.height < 8 {
            return;
        }
        let width = WIDTH.min(area.width);
        let height = HEIGHT.min(area.height);
        let box_rect = Rect {
            x: area.x + area.width - width,
            y: area.y,
            width,
            height,
        };
        for y in box_rect.y..box_rect.y + box_rect.height {
            for x in box_rect.x..box_rect.x + box_rect.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(" ");
                }
            }
        }
        let chrome = Chrome::themed(
            theme,
            Some(theme.border),
            Some(theme.background),
            Padding {
                horizontal: 0,
                vertical: 0,
            },
        );
        let inner = draw_box(buf, box_rect, &chrome, theme);
        buf.set_stringn(
            inner.x + 1,
            inner.y,
            "performance",
            11,
            Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        );
        let text = Style::default().fg(theme.foreground).bg(theme.background);
        for (index, line) in self.lines().iter().enumerate() {
            let y = inner.y + 2 + u16::try_from(index).unwrap_or(u16::MAX);
            if y >= inner.y + inner.height {
                break;
            }
            buf.set_stringn(
                inner.x + 1,
                y,
                line,
                usize::from(inner.width.saturating_sub(2)),
                text,
            );
        }
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}

fn format_timing(step: Phase, timing: crate::metrics::PhaseTiming) -> String {
    format!(
        "{:<9} last {:>6.2} avg {:>6.2} max {:>6.2} ms",
        step.label(),
        timing.last.as_secs_f64() * 1000.0,
        timing.average.as_secs_f64() * 1000.0,
        timing.max.as_secs_f64() * 1000.0
    )
}

fn format_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds < 1.0 {
        format!("{:.0} ms", seconds * 1000.0)
    } else if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        let total = duration.as_secs();
        format!("{}m {:02}s", total / 60, total % 60)
    }
}

#[cfg(test)]
#[path = "debug_overlay_tests.rs"]
mod tests;
