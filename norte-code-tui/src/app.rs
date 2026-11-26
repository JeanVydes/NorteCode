//! The run loop: terminal, events (input and server), view tree, frames.
//!
//! Each iteration drains the server's events into the model, folds the
//! user's input into the tree, paints, then polls for the next input.
//! Views never call back into the client; the app mediates between them.

use std::io;
use std::time::{Duration, Instant};

use norte_code_client::config::RetentionConfig;
use ratatui::style::Style;

use crate::client::{ClientEvent, UiCommand, start};
use crate::config::ClientConfig;
use crate::frame_limiter::FrameLimiter;
use crate::metrics::{Metrics, Phase};
use crate::terminal::TerminalSession;
use crate::theme::{CustomTheme, ThemeCatalog};
use crate::ui;
use crate::ui_model::{SessionAttachmentState, TurnState, UiModel};

mod chrome;
mod drain;
mod event_fold;
mod input;

use chrome::{sync_chrome, sync_debug_overlay};
use event_fold::drain_server_events;
use input::poll_event;

/// Idle waits block this long, so an untouched TUI does not spin a core.
const IDLE_POLL_MS: u64 = 250;
/// Shorter waits keep a streaming turn responsive.
const ACTIVE_POLL_MS: u64 = 80;

/// What draining the client channel produced.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct DrainOutcome {
    changed: bool,
    events: usize,
}

/// What one input poll produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PollOutcome {
    /// The poll timed out with no input.
    Idle,
    /// Input was handled, so the frame needs repainting.
    Event(InputKind),
    /// The user asked to quit.
    Quit,
}

/// Which terminal event woke the loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputKind {
    Key,
    Mouse,
    Other,
}

/// Borrowed state shared by the per-frame helpers.
struct FrameContext<'a> {
    tree: &'a mut terminalui::Tree,
    metrics: &'a mut Metrics,
    model: &'a UiModel,
}

/// Entry point. `run` renders the north star until the user quits.
///
/// # Errors
/// Returns the error the loop reported.
pub(super) fn run() -> Result<(), crate::TuiError> {
    let mut session = TerminalSession::enter()?;
    crate::terminal::watch_exit_signals();

    let mut model = UiModel::detached();
    let (config, mut commands, mut events) = attach_client(&mut model);
    let mut plugin_rail = crate::plugin_widgets::Subscription::default();
    let retention = config
        .as_ref()
        .map_or_else(RetentionConfig::default, |config| config.retention);

    let mut tree = ui::build_tree_with_retention(&model, retention);
    let mut metrics = Metrics::new();
    let mut frame_limiter = FrameLimiter::new();
    let mut overlay_clock = ui::OverlayClock::new();
    let mut views_dirty = true;
    let mut redraw = true;
    let mut last_tick = Instant::now();

    loop {
        let now = Instant::now();
        if crate::terminal::SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }

        let drain_started = Instant::now();
        let drained = drain_server_events(&mut events, &mut commands, &mut tree, &mut model);
        metrics.record(Phase::Drain, drain_started.elapsed());
        metrics.server_events(drained.events);
        if drained.changed {
            views_dirty = true;
            redraw = true;
        }

        if views_dirty {
            let refresh_started = Instant::now();
            ui::refresh_views(&mut tree, &model);
            ui::sync_views(&mut tree, &model);
            metrics.record(Phase::Refresh, refresh_started.elapsed());
            views_dirty = false;
        }
        let sync_started = Instant::now();
        plugin_rail.sync(commands.as_ref(), &model);
        sync_chrome(&mut tree, &model);
        metrics.record(Phase::SyncChrome, sync_started.elapsed());

        let animating = animating(&tree, &model);
        if overlay_clock.due(now, model.preferences.debug.enabled()) {
            redraw = true;
        }
        let mut frame = FrameContext {
            tree: &mut tree,
            metrics: &mut metrics,
            model: &model,
        };
        prepare_debug(&mut frame, events.as_ref(), now, animating);
        draw_if_needed(
            &mut session,
            &mut frame,
            &mut frame_limiter,
            now,
            &mut redraw,
        )?;
        tick_tree(&mut tree, &mut metrics, now, &mut last_tick);

        let timeout = poll_timeout(animating, redraw, &frame_limiter, now);
        let poll_started = Instant::now();
        let outcome = poll_event(
            &mut tree,
            &mut commands,
            &mut events,
            &mut model,
            config.as_ref(),
            timeout,
        )?;
        metrics.record(Phase::Poll, poll_started.elapsed());
        match outcome {
            PollOutcome::Quit => {
                metrics.iteration(now);
                break;
            }
            PollOutcome::Event(_) => {
                metrics.input_event();
                redraw = true;
            }
            PollOutcome::Idle => metrics.poll_timeout(),
        }
        metrics.iteration(now);
    }

    session.leave();
    Ok(())
}

fn attach_client(
    model: &mut UiModel,
) -> (
    Option<ClientConfig>,
    Option<tokio::sync::mpsc::Sender<UiCommand>>,
    Option<tokio::sync::mpsc::Receiver<ClientEvent>>,
) {
    match ClientConfig::load() {
        Ok(loaded) => {
            model.preferences.catalog = ThemeCatalog {
                custom: loaded
                    .ui
                    .themes
                    .iter()
                    .cloned()
                    .map(CustomTheme::from_config)
                    .collect(),
            };
            model.preferences.theme = model.preferences.catalog.choose(loaded.ui.theme.as_deref());
            model.preferences.rail_position = loaded.ui.rail_position.unwrap_or_default();
            model.preferences.debug = loaded.ui.debug.into();
            model.preferences.show_tool_output = loaded.ui.show_tool_output;
            model.preferences.tool_verbosity = loaded.ui.tool_verbosity;
            if let Some(value) = loaded.ui.reasoning {
                model.preferences.reasoning = value;
            }
            if let Some(value) = loaded.ui.send_mode {
                model.preferences.send_mode = value;
            }
            if let Some(value) = loaded.ui.diff_mode {
                model.preferences.diff_mode = value;
            }
            model.preferences.chart_density = loaded.ui.chart_density.clamp(2, 6);
            model.preferences.chart_contrast = contrast_quarters(loaded.ui.chart_contrast);
            model.preferences.chart_dot_size = match loaded.ui.chart_dot_size {
                terminalui::components::presentation::DotSize::Small => 0,
                terminalui::components::presentation::DotSize::Large => 2,
                _ => 1,
            };
            model.service_url.clone_from(&loaded.service.url);
            let (commands, events) = start(loaded.clone());
            model.attachment = SessionAttachmentState::Connecting;
            (Some(loaded), Some(commands), Some(events))
        }
        Err(error) => {
            model.attachment = SessionAttachmentState::Failed {
                reason: format!("{error}"),
            };
            (None, None, None)
        }
    }
}

fn contrast_quarters(value: f32) -> u8 {
    const CUTS: &[f32] = &[
        0.375, 0.625, 0.875, 1.125, 1.375, 1.625, 1.875, 2.125, 2.375, 2.625, 2.875, 3.125, 3.375,
        3.625, 3.875,
    ];
    let value = value.clamp(0.25, 4.0);
    let step = CUTS
        .iter()
        .position(|cut| value < *cut)
        .unwrap_or(CUTS.len());
    u8::try_from(step + 1).unwrap_or(4)
}

fn prepare_debug(
    frame: &mut FrameContext<'_>,
    events: Option<&tokio::sync::mpsc::Receiver<ClientEvent>>,
    now: Instant,
    animating: bool,
) {
    let debug = frame.model.effective_debug_overlay().enabled();
    if debug {
        frame.metrics.sample_process(now);
    }
    let queued = events.map_or(0, tokio::sync::mpsc::Receiver::len);
    let snapshot = frame.metrics.snapshot(now, queued, animating);
    sync_debug_overlay(frame.tree, &snapshot, debug);
}

fn draw_if_needed(
    session: &mut TerminalSession,
    frame: &mut FrameContext<'_>,
    frame_limiter: &mut FrameLimiter,
    now: Instant,
    redraw: &mut bool,
) -> io::Result<()> {
    if (!*redraw && !animating(frame.tree, frame.model)) || !frame_limiter.allow(now) {
        frame.metrics.frame_skipped();
        return Ok(());
    }
    let theme = frame
        .model
        .preferences
        .catalog
        .theme(&frame.model.preferences.theme);
    let draw_started = Instant::now();
    session.terminal().draw(|draw_frame| {
        let area = draw_frame.area();
        draw_frame
            .buffer_mut()
            .set_style(area, Style::default().bg(theme.background));
        frame.tree.render(area, draw_frame.buffer_mut(), &theme);
    })?;
    if let Some(graphics) = session.graphics.as_mut() {
        let next = frame
            .tree
            .find_mut::<ui::Composer>("composer")
            .and_then(ui::Composer::take_native_preview);
        graphics.sync(next, &mut std::io::stdout())?;
    }
    frame.metrics.frame_drawn(now, draw_started.elapsed());
    *redraw = false;
    Ok(())
}

fn tick_tree(
    tree: &mut terminalui::Tree,
    metrics: &mut Metrics,
    now: Instant,
    last_tick: &mut Instant,
) {
    let started = Instant::now();
    tree.tick(now.duration_since(*last_tick));
    metrics.record(Phase::Tick, started.elapsed());
    *last_tick = now;
}

fn poll_timeout(
    animating: bool,
    redraw: bool,
    frame_limiter: &FrameLimiter,
    now: Instant,
) -> Duration {
    let default = Duration::from_millis(if animating {
        ACTIVE_POLL_MS
    } else {
        IDLE_POLL_MS
    });
    if redraw {
        frame_limiter.remaining(now).min(default)
    } else {
        default
    }
}

/// Whether anything on screen moves on its own.
fn animating(tree: &terminalui::Tree, model: &UiModel) -> bool {
    model.turn != TurnState::Idle
        || tree
            .find::<ui::PlanOverlay>("plan_overlay")
            .is_some_and(ui::PlanOverlay::is_open)
        || (model.tab == crate::ui_model::Tab::Sessions
            && tree
                .find::<ui::SessionsPage>("sessions")
                .is_some_and(ui::SessionsPage::animating))
        || (model.tab == crate::ui_model::Tab::Chat
            && tree
                .find::<ui::Transcript>("stream")
                .is_some_and(ui::Transcript::welcome_animating))
        || tree
            .find::<ui::ToastHost>("toast")
            .is_some_and(ui::ToastHost::is_active)
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
