//! The main frame: header, tab pane, optional composer and status strip.
//!
//! A custom view so the six composer rows go back to the tab pane outside Chat
//! instead of staying reserved as an empty box.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde::Serialize;
use terminalui::{Context, InputEvent, Node, Outcome, Theme, View};

/// The four bands of the main frame.
#[derive(Serialize)]
pub(crate) struct AppFrame {
    show_composer: bool,
    children: Vec<Node>,
}

impl AppFrame {
    /// Compose the frame from its four bands.
    pub(crate) fn new(children: Vec<Node>, show_composer: bool) -> Self {
        Self {
            show_composer,
            children,
        }
    }

    /// Show the composer band only on the chat tab.
    pub(crate) fn set_show_composer(&mut self, show: bool) {
        self.show_composer = show;
    }
}

impl View for AppFrame {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "app_frame"
    }
    fn children(&self) -> &[Node] {
        &self.children
    }
    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let composer_height = if self.show_composer { 6 } else { 0 };
        let header_height = 1u16.min(area.height);
        let status_height = 1u16.min(area.height.saturating_sub(header_height));
        let middle_height = area
            .height
            .saturating_sub(header_height + composer_height + status_height);

        let bands = [
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: header_height,
            },
            Rect {
                x: area.x,
                y: area.y + header_height,
                width: area.width,
                height: middle_height,
            },
            Rect {
                x: area.x,
                y: area.y + header_height + middle_height,
                width: area.width,
                height: composer_height,
            },
            Rect {
                x: area.x,
                y: area.y + header_height + middle_height + composer_height,
                width: area.width,
                height: status_height,
            },
        ];
        for (index, child) in self.children.iter_mut().enumerate() {
            if let Some(rect) = bands.get(index)
                && rect.height > 0
            {
                child.view.render(*rect, buf, cx, theme);
            } else if child.view.focusable() {
                // Keep the focus counter in step with the tree's count_focusable.
                let _ = cx.is_focused();
            }
        }
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}
