//! Instrumentation for the frame loop.
//!
//! The collector owns counters and bounded timing samples. The overlay reads
//! immutable snapshots, so rendering never mutates the metrics.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::process_stats::{MemoryUsage, ProcessUsage, read_process_usage};

const SAMPLE_LIMIT: usize = 240;
const FPS_WINDOW: Duration = Duration::from_secs(1);
const PROCESS_INTERVAL: Duration = Duration::from_secs(1);

/// A phase of one loop iteration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Phase {
    /// Draining the client's event channel.
    Drain,
    /// Rebuilding page data after new server state.
    Refresh,
    /// Copying state into the view tree.
    SyncChrome,
    /// Advancing animations.
    Tick,
    /// Waiting for terminal input.
    Poll,
    /// Painting a frame.
    Draw,
    /// Sampling process counters.
    Process,
}

impl Phase {
    pub(crate) const ALL: [Self; 7] = [
        Self::Drain,
        Self::Refresh,
        Self::SyncChrome,
        Self::Tick,
        Self::Poll,
        Self::Draw,
        Self::Process,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Drain => 0,
            Self::Refresh => 1,
            Self::SyncChrome => 2,
            Self::Tick => 3,
            Self::Poll => 4,
            Self::Draw => 5,
            Self::Process => 6,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Drain => "drain",
            Self::Refresh => "refresh",
            Self::SyncChrome => "sync",
            Self::Tick => "tick",
            Self::Poll => "poll",
            Self::Draw => "draw",
            Self::Process => "proc",
        }
    }
}

/// Last, mean and worst duration of one phase.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PhaseTiming {
    pub(crate) last: Duration,
    pub(crate) average: Duration,
    pub(crate) max: Duration,
}

/// Timing for every loop phase.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PhaseTimings {
    drain: PhaseTiming,
    refresh: PhaseTiming,
    sync_chrome: PhaseTiming,
    tick: PhaseTiming,
    poll: PhaseTiming,
    draw: PhaseTiming,
    process: PhaseTiming,
}

impl PhaseTimings {
    fn set(&mut self, phase: Phase, timing: PhaseTiming) {
        match phase {
            Phase::Drain => self.drain = timing,
            Phase::Refresh => self.refresh = timing,
            Phase::SyncChrome => self.sync_chrome = timing,
            Phase::Tick => self.tick = timing,
            Phase::Poll => self.poll = timing,
            Phase::Draw => self.draw = timing,
            Phase::Process => self.process = timing,
        }
    }

    pub(crate) fn get(self, phase: Phase) -> PhaseTiming {
        match phase {
            Phase::Drain => self.drain,
            Phase::Refresh => self.refresh,
            Phase::SyncChrome => self.sync_chrome,
            Phase::Tick => self.tick,
            Phase::Poll => self.poll,
            Phase::Draw => self.draw,
            Phase::Process => self.process,
        }
    }
}

/// An immutable reading for the overlay.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MetricsSnapshot {
    pub(crate) uptime: Duration,
    pub(crate) drawn_fps: f64,
    pub(crate) loop_fps: f64,
    pub(crate) frames_drawn: u64,
    pub(crate) frames_skipped: u64,
    pub(crate) server_events: u64,
    pub(crate) input_events: u64,
    pub(crate) poll_timeouts: u64,
    pub(crate) queued_events: usize,
    pub(crate) active: bool,
    pub(crate) last_draw_age: Option<Duration>,
    pub(crate) process: Option<ProcessUsage>,
    pub(crate) phases: PhaseTimings,
}

/// Collects loop timings and counters.
#[derive(Debug)]
pub(crate) struct Metrics {
    started: Instant,
    last_draw: Option<Instant>,
    frames: VecDeque<Instant>,
    iterations: VecDeque<Instant>,
    samples: [VecDeque<Duration>; Phase::ALL.len()],
    frames_drawn: u64,
    frames_skipped: u64,
    server_events: u64,
    input_events: u64,
    poll_timeouts: u64,
    process: Option<ProcessUsage>,
    last_process_sample: Option<Instant>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            last_draw: None,
            frames: VecDeque::new(),
            iterations: VecDeque::new(),
            samples: std::array::from_fn(|_| VecDeque::new()),
            frames_drawn: 0,
            frames_skipped: 0,
            server_events: 0,
            input_events: 0,
            poll_timeouts: 0,
            process: None,
            last_process_sample: None,
        }
    }

    pub(crate) fn record(&mut self, phase: Phase, duration: Duration) {
        let samples = &mut self.samples[phase.index()];
        samples.push_back(duration);
        if samples.len() > SAMPLE_LIMIT {
            samples.pop_front();
        }
    }

    pub(crate) fn frame_drawn(&mut self, now: Instant, duration: Duration) {
        self.frames.push_back(now);
        self.last_draw = Some(now);
        self.frames_drawn += 1;
        self.record(Phase::Draw, duration);
    }

    pub(crate) fn frame_skipped(&mut self) {
        self.frames_skipped += 1;
    }

    pub(crate) fn iteration(&mut self, now: Instant) {
        self.iterations.push_back(now);
        self.prune(now);
    }

    pub(crate) fn server_events(&mut self, count: usize) {
        self.server_events += u64::try_from(count).unwrap_or(u64::MAX);
    }

    pub(crate) fn input_event(&mut self) {
        self.input_events += 1;
    }

    pub(crate) fn poll_timeout(&mut self) {
        self.poll_timeouts += 1;
    }

    /// Refresh process counters at most once a second; `ps` spawns a process.
    pub(crate) fn sample_process(&mut self, now: Instant) {
        if self
            .last_process_sample
            .is_some_and(|last| now.duration_since(last) < PROCESS_INTERVAL)
        {
            return;
        }
        let started = Instant::now();
        let sample = read_process_usage();
        self.record(Phase::Process, started.elapsed());
        self.last_process_sample = Some(now);
        if let Some(sample) = sample {
            let baseline = self
                .process
                .map_or(sample.rss, |usage| usage.memory.baseline);
            self.process = Some(ProcessUsage {
                memory: MemoryUsage {
                    current: sample.rss,
                    baseline,
                },
                cpu_percent: sample.cpu_percent,
            });
        }
    }

    pub(crate) fn snapshot(
        &self,
        now: Instant,
        queued_events: usize,
        active: bool,
    ) -> MetricsSnapshot {
        let mut phases = PhaseTimings::default();
        for phase in Phase::ALL {
            phases.set(phase, summarize(&self.samples[phase.index()]));
        }
        MetricsSnapshot {
            uptime: now.saturating_duration_since(self.started),
            drawn_fps: rate(&self.frames, now),
            loop_fps: rate(&self.iterations, now),
            frames_drawn: self.frames_drawn,
            frames_skipped: self.frames_skipped,
            server_events: self.server_events,
            input_events: self.input_events,
            poll_timeouts: self.poll_timeouts,
            queued_events,
            active,
            last_draw_age: self
                .last_draw
                .map(|last| now.saturating_duration_since(last)),
            process: self.process,
            phases,
        }
    }

    fn prune(&mut self, now: Instant) {
        let cutoff = now.checked_sub(FPS_WINDOW);
        for log in [&mut self.frames, &mut self.iterations] {
            if let Some(cutoff) = cutoff {
                while log.front().is_some_and(|time| *time < cutoff) {
                    log.pop_front();
                }
            }
        }
    }
}

fn summarize(samples: &VecDeque<Duration>) -> PhaseTiming {
    let Some(last) = samples.back().copied() else {
        return PhaseTiming::default();
    };
    let max = samples.iter().copied().max().unwrap_or(last);
    let count = u32::try_from(samples.len()).unwrap_or(u32::MAX);
    let total = samples.iter().copied().sum::<Duration>();
    PhaseTiming {
        last,
        average: total / count,
        max,
    }
}

fn rate(log: &VecDeque<Instant>, now: Instant) -> f64 {
    let Some(first) = log.front() else {
        return 0.0;
    };
    let span = now
        .saturating_duration_since(*first)
        .max(Duration::from_millis(1));
    let count = u32::try_from(log.len()).unwrap_or(u32::MAX);
    f64::from(count) / span.as_secs_f64()
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
