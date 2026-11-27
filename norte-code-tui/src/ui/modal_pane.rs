//! Keeps the current page visible beneath the active modal.
use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Clear, Widget},
};
use serde::Serialize;
use std::any::Any;
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View};

use super::overlay_motion::OverlayMotion;

#[derive(Serialize)]
pub(crate) struct ModalPane {
    children: Vec<Node>,
    visible: usize,
    #[serde(skip)]
    motion: OverlayMotion,
}
impl ModalPane {
    pub(crate) fn new(children: Vec<Node>) -> Self {
        let mut pane = Self {
            children,
            visible: 0,
            motion: OverlayMotion::default(),
        };
        pane.set_visible(0);
        pane
    }
    pub(crate) fn visible(&self) -> usize {
        self.visible
    }
    pub(crate) fn set_visible(&mut self, index: usize) {
        if index >= self.children.len() {
            return;
        }
        self.visible = index;
        for (i, child) in self.children.iter_mut().enumerate() {
            child.set_hidden(i != index);
        }
    }
}
impl View for ModalPane {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "modal_pane"
    }
    fn children(&self) -> &[Node] {
        &self.children
    }
    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }
    fn focusable(&self) -> bool {
        self.children
            .get(self.visible)
            .is_some_and(|child| child.view.focusable())
    }
    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        Clear.render(area, buf);
        if self.visible != 0 {
            // The hidden page is painted with its own focus counter, so it does
            // not disturb focus traversal for the active modal.
            let mut background_cx = *cx;
            if let Some(background) = self.children.first_mut() {
                background.view.render(area, buf, &mut background_cx, theme);
            }
        }
        if self.visible == 0 {
            if let Some(child) = self.children.first_mut() {
                child.view.render(area, buf, cx, theme);
            }
            self.motion.paint(area, buf, false, 0, theme, |_| {});
        } else if let Some(child) = self.children.get_mut(self.visible) {
            self.motion
                .paint(area, buf, true, self.visible, theme, |layer| {
                    child.view.render(area, layer, cx, theme);
                });
        }
    }
    fn tick(&mut self, delta: Duration, cx: &mut Context) {
        self.motion.tick(delta);
        for child in &mut self.children {
            child.view.tick(delta, cx);
        }
    }
    fn handle_event(&mut self, _: &InputEvent, _: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}
