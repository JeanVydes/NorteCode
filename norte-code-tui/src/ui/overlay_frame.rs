//! A container that paints a base pane and a floating overlay over it.

use std::any::Any;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde::Serialize;
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View};

use super::overlay_motion::OverlayMotion;

/// The base view first, then the overlay in the same area.
#[derive(Serialize)]
pub(crate) struct OverlayFrame {
    children: Vec<Node>,
    animated: bool,
    #[serde(skip)]
    motion: OverlayMotion,
}

impl OverlayFrame {
    pub(crate) fn new(base: Node, overlay: Node) -> Self {
        Self {
            children: vec![base, overlay],
            animated: false,
            motion: OverlayMotion::default(),
        }
    }

    pub(crate) fn animated(base: Node, overlay: Node) -> Self {
        Self {
            children: vec![base, overlay],
            animated: true,
            motion: OverlayMotion::default(),
        }
    }
}

impl View for OverlayFrame {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "overlay_frame"
    }
    fn children(&self) -> &[Node] {
        &self.children
    }
    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        self.children[0].view.render(area, buf, cx, theme);
        if self.animated {
            let open = self.children[1].view.focusable();
            self.motion.paint(area, buf, open, 1, theme, |layer| {
                self.children[1].view.render(area, layer, cx, theme);
            });
        } else {
            self.children[1].view.render(area, buf, cx, theme);
        }
    }

    fn tick(&mut self, delta: Duration, cx: &mut Context) {
        self.motion.tick(delta);
        for child in &mut self.children {
            child.view.tick(delta, cx);
        }
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}
