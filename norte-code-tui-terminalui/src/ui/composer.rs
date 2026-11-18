//! The multiline composer.
//!
//! Wraps the library `Input` with a rounded border in the theme accent and
//! a caret that breathes; typed events flow through the tree and land here
//! when it has focus.

use std::any::Any;

use serde::Serialize;
use terminalui::Node;
use terminalui::components::chrome::{Border, BorderSet, Radius};
use terminalui::components::cursor::{CursorMode, CursorShape, CursorStyle};
use terminalui::components::input::{Input, InputStyle};
use terminalui::components::scroll::ScrollbarStyle;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// The composed composer box; the tree knows it as one node.
#[derive(Serialize)]
pub(crate) struct Composer {
    input: Input,
}

impl Composer {
    /// Creates a composer reading from the given theme.
    pub(crate) fn new(theme: &Theme) -> Self {
        let style = InputStyle {
            border: Some(Border {
                set: BorderSet::Rounded,
                color: Some(theme.accent),
                radius: Radius::Small,
            }),
            radius: Radius::Small,
            background: Some(theme.secondary),
            foreground: Some(theme.foreground),
            placeholder: Some(theme.muted),
            cursor: Some(theme.accent),
            scrollbar: ScrollbarStyle::default(),
            cursor_style: CursorStyle {
                shape: CursorShape::Line,
                mode: CursorMode::Pulse,
                blink_ms: 530,
                opacity: 1.0,
                easing: terminalui::Easing::EaseInOutSine,
            },
        };
        Self {
            input: Input::new()
                .multiline(true)
                .with_placeholder("Ask NORTE to do anything...")
                .with_style(style),
        }
    }

    /// The current text, trimmed for dispatch.
    pub(crate) fn text(&self) -> &str {
        self.input.value()
    }

    /// Replace the text; used to clear after sending.
    pub(crate) fn clear(&mut self) {
        self.input.clear();
    }

    /// Disable or enable editing; used when a page other than Chat is active.
    pub(crate) fn set_disabled(&mut self, disabled: bool) {
        self.input.state.disabled = disabled;
    }
}

impl View for Composer {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "composer"
    }

    fn children(&self) -> &[Node] {
        &[]
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut []
    }

    fn measure(&self) -> Option<(u16, u16)> {
        None
    }

    fn render(
        &mut self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::buffer::Buffer,
        cx: &mut Context,
        theme: &Theme,
    ) {
        self.input.render(area, buf, cx, theme);
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        self.input.handle_event(event, cx)
    }

    fn focusable(&self) -> bool {
        self.input.focusable()
    }
}
