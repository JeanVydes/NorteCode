use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::{Animation, Easing, Effect, Theme};

#[derive(Default)]
pub(crate) struct OverlayMotion {
    previous_open: bool,
    key: usize,
    phase: Option<Phase>,
    animation: Option<Animation>,
    previous_layer: Option<Buffer>,
    changed: Option<Rect>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    In,
    Out,
}

impl OverlayMotion {
    pub(crate) fn tick(&mut self, delta: Duration) {
        let Some(animation) = &mut self.animation else {
            return;
        };
        if animation.tick(delta) {
            return;
        }
        self.animation = None;
        self.phase = None;
        if !self.previous_open {
            self.previous_layer = None;
            self.changed = None;
        }
    }

    pub(crate) fn paint(
        &mut self,
        area: Rect,
        base: &mut Buffer,
        open: bool,
        key: usize,
        theme: &Theme,
        mut render: impl FnMut(&mut Buffer),
    ) {
        let mut overlay = base.clone();
        render(&mut overlay);
        let changed = changed_rect(base, &overlay, area);
        let open = open || changed.is_some();
        if open {
            if !self.previous_open || self.key != key {
                self.animation = Some(Animation::new(
                    Duration::from_millis(150),
                    Easing::EaseOutCubic,
                ));
                self.phase = Some(Phase::In);
            }
            self.previous_open = true;
            self.key = key;
            self.changed = changed;
            self.previous_layer = Some(overlay.clone());
            self.paint_frame(base, &overlay, changed, theme);
            return;
        }

        if self.previous_open {
            self.previous_open = false;
            self.animation = Some(Animation::new(
                Duration::from_millis(130),
                Easing::EaseOutCubic,
            ));
            self.phase = Some(Phase::Out);
        }
        if self.phase == Some(Phase::Out)
            && let (Some(layer), Some(changed), Some(animation)) =
                (&self.previous_layer, self.changed, self.animation)
        {
            let mut output = layer.clone();
            Effect::FadeOut.apply(layer, &mut output, changed, theme, animation.progress());
            copy_area(&output, base, changed);
        }
    }

    fn paint_frame(
        &self,
        base: &mut Buffer,
        overlay: &Buffer,
        changed: Option<Rect>,
        theme: &Theme,
    ) {
        let Some(changed) = changed else {
            return;
        };
        if self.phase == Some(Phase::In)
            && let Some(animation) = self.animation
        {
            let mut output = overlay.clone();
            Effect::FadeIn.apply(overlay, &mut output, changed, theme, animation.progress());
            copy_area(&output, base, changed);
        } else {
            copy_area(overlay, base, changed);
        }
    }
}

fn changed_rect(base: &Buffer, overlay: &Buffer, area: Rect) -> Option<Rect> {
    let mut left = area.right();
    let mut top = area.bottom();
    let mut right = area.x;
    let mut bottom = area.y;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if base[(x, y)] == overlay[(x, y)] {
                continue;
            }
            left = left.min(x);
            top = top.min(y);
            right = right.max(x.saturating_add(1));
            bottom = bottom.max(y.saturating_add(1));
        }
    }
    (left < right && top < bottom).then(|| Rect::new(left, top, right - left, bottom - top))
}

fn copy_area(source: &Buffer, destination: &mut Buffer, area: Rect) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            destination[(x, y)] = source[(x, y)].clone();
        }
    }
}
