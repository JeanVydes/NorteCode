//! The chat body: the stream with the rail on any of its four sides.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde::Serialize;

use terminalui::{Context, InputEvent, Node, Outcome, Theme, View, node};

use super::Transcript;
use super::rail::{Rail, RailPosition};

const SIDE: u16 = 28;
const STRIP: u16 = 9;

/// The stream and its rail, arranged by the configured position.
#[derive(Serialize)]
pub(crate) struct ChatLayout {
    position: RailPosition,
    children: Vec<Node>,
}

impl ChatLayout {
    /// A fresh stream and detached rail in the given position.
    pub(crate) fn new(position: RailPosition) -> Self {
        Self {
            position,
            children: vec![
                node("rail", Rail::detached()),
                node("stream", Transcript::new()),
            ],
        }
    }

    /// Move the rail to another side.
    pub(crate) fn set_position(&mut self, position: RailPosition) {
        self.position = position;
    }

    /// Split the area into the rail rect and the stream rect.
    fn split(&self, area: Rect) -> (Option<Rect>, Rect) {
        match self.position {
            RailPosition::Hide => (None, area),
            RailPosition::Left => {
                let width = SIDE.min(area.width);
                (
                    Some(Rect { width, ..area }),
                    Rect {
                        x: area.x + width,
                        width: area.width - width,
                        ..area
                    },
                )
            }
            RailPosition::Right => {
                let width = SIDE.min(area.width);
                (
                    Some(Rect {
                        x: area.x + area.width - width,
                        width,
                        ..area
                    }),
                    Rect {
                        width: area.width - width,
                        ..area
                    },
                )
            }
            RailPosition::Top => {
                let height = STRIP.min(area.height);
                (
                    Some(Rect { height, ..area }),
                    Rect {
                        y: area.y + height,
                        height: area.height - height,
                        ..area
                    },
                )
            }
            RailPosition::Bottom => {
                let height = STRIP.min(area.height);
                (
                    Some(Rect {
                        y: area.y + area.height - height,
                        height,
                        ..area
                    }),
                    Rect {
                        height: area.height - height,
                        ..area
                    },
                )
            }
        }
    }
}

impl View for ChatLayout {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "chat"
    }

    fn children(&self) -> &[Node] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let (rail, stream) = self.split(area);
        if let Some(rail) = rail
            && let Some(node) = self.children.first_mut()
        {
            node.view.render(rail, buf, cx, theme);
        }
        if let Some(node) = self.children.get_mut(1) {
            node.view.render(stream, buf, cx, theme);
        }
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}

#[cfg(test)]
#[path = "chat_layout_tests.rs"]
mod tests;
