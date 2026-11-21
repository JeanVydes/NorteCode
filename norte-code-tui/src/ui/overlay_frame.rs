//! A container that paints a base pane and a floating overlay over it.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde::Serialize;
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View};

/// The base view first, then the overlay in the same area.
#[derive(Serialize)]
pub(crate) struct OverlayFrame {
    children: Vec<Node>,
}

impl OverlayFrame {
    pub(crate) fn new(base: Node, overlay: Node) -> Self {
        Self {
            children: vec![base, overlay],
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
        self.children[1].view.render(area, buf, cx, theme);
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}
