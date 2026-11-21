//! One transient toast at a time, rendered above the screen.

use std::any::Any;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde::Serialize;

use terminalui::components::toast::{Toast, ToastKind};
use terminalui::{Context, InputEvent, Node, Outcome, Text, Theme, View, node};

/// Holds the current toast and retires it when its time-to-live ends.
#[derive(Serialize)]
pub(crate) struct ToastHost {
    #[serde(skip)]
    toast: Option<Toast>,
}

impl ToastHost {
    pub(crate) fn new() -> Self {
        Self { toast: None }
    }

    pub(crate) fn show(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.toast = Some(Toast::with_children(
            kind,
            vec![node("message", Text::new(message))],
        ));
    }

    pub(crate) fn is_active(&self) -> bool {
        self.toast.is_some()
    }
}

impl View for ToastHost {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "toast_host"
    }

    fn children(&self) -> &[Node] {
        &[]
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut []
    }

    fn focusable(&self) -> bool {
        false
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        if let Some(toast) = &mut self.toast {
            toast.render(area, buf, cx, theme);
        }
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }

    fn tick(&mut self, delta: Duration, cx: &mut Context) {
        let mut expired = false;
        if let Some(toast) = &mut self.toast {
            toast.tick(delta, cx);
            expired = toast.expired();
        }
        if expired {
            self.toast = None;
        }
    }
}
