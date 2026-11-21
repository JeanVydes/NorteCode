//! The floating performance box.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{DebugOverlay, OverlayClock};
use crate::metrics::MetricsSnapshot;
use crate::process_stats::{Kilobytes, MemoryUsage, ProcessUsage};
use crate::ui::build_tree;
use crate::ui_model::UiModel;

fn overlay_text(enabled: bool) -> String {
    let model = UiModel::detached();
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    let snapshot = MetricsSnapshot {
        uptime: Duration::from_secs(75),
        drawn_fps: 12.5,
        loop_fps: 12.4,
        frames_drawn: 90,
        frames_skipped: 3,
        server_events: 17,
        input_events: 4,
        poll_timeouts: 2,
        queued_events: 1,
        active: true,
        last_draw_age: Some(Duration::from_millis(80)),
        process: Some(ProcessUsage {
            memory: MemoryUsage {
                current: Kilobytes::new(64 * 1024),
                baseline: Kilobytes::new(60 * 1024),
            },
            cpu_percent: 0.7,
        }),
        ..Default::default()
    };
    if let Some(overlay) = tree.find_mut::<DebugOverlay>("debug_overlay") {
        overlay.set_enabled(enabled);
        overlay.set_snapshot(&snapshot);
    }
    let area = Rect::new(0, 0, 100, 30);
    let right = area.width.saturating_sub(58);
    let mut buf = Buffer::empty(area);
    for y in 0..area.height {
        for x in right..area.width {
            buf[(x, y)].set_symbol("X");
        }
    }
    tree.render(area, &mut buf, &theme);
    (0..18)
        .map(|y| {
            (right..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_overlay_shows_frame_timings_and_memory_when_enabled() {
    let text = overlay_text(true);

    assert!(text.contains("performance"), "{text}");
    assert!(!text.contains('X'), "{text}");
    assert!(text.contains("draw 12.5"), "{text}");
    assert!(text.contains("90 drawn"), "{text}");
    assert!(text.contains("rss"), "{text}");
    assert!(text.contains("64.0 MiB (+4.0)"), "{text}");
    assert!(text.contains("cpu       0.7% avg"), "{text}");
    assert!(text.contains("1m 15s"), "{text}");
}

#[test]
fn the_clock_paces_overlay_repaints() {
    let now = Instant::now() + Duration::from_secs(1);
    let mut clock = OverlayClock::new();

    assert!(!clock.due(now, false));
    assert!(clock.due(now, true));
    assert!(!clock.due(now + Duration::from_millis(100), true));
    assert!(clock.due(now + Duration::from_millis(250), true));
}

#[test]
fn the_overlay_leaves_no_ink_when_disabled() {
    let text = overlay_text(false);

    assert!(!text.contains("performance"), "{text}");
    assert!(!text.contains("rss"), "{text}");
    assert!(!text.contains("cpu"), "{text}");
}
