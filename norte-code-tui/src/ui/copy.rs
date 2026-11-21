//! Every user-facing string, in one place, so the interface speaks one voice.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde::Serialize;

use terminalui::components::kbd::{Kbd, KbdStyle};
use terminalui::{Context, InputEvent, Node, Outcome, Text, TextStyle, Theme, View, node};

pub(crate) const READY: &str = "● ready";
pub(crate) const READY_TEXT: &str = "ready";
pub(crate) const CONNECTING: &str = "connecting";
pub(crate) const THINKING: &str = "thinking";
pub(crate) const WORKING: &str = "working";
pub(crate) const WAITING: &str = "waiting";

pub(crate) const TAB_HINT: &str = "tabs";
pub(crate) const SEND: &str = "send";
pub(crate) const NEWLINE: &str = "newline";
pub(crate) const THEME_KEY: &str = "⌃T";

pub(crate) const CONTEXT: &str = "CONTEXT";
pub(crate) const PLANS: &str = "PLANS";
pub(crate) const SESSION: &str = "SESSION";
pub(crate) const CONNECTION: &str = "CONNECTION";
pub(crate) const COMPACT_AT: &str = "compact at";
pub(crate) const NONE: &str = "none";
pub(crate) const NO_SESSION: &str = "no session";
pub(crate) const DETACHED: &str = "detached";
pub(crate) const CONNECTING_DOT: &str = "● connecting…";
pub(crate) const ATTACHED: &str = "● attached";
pub(crate) const FAILED: &str = "✗ failed";

pub(crate) const COMPOSER_PLACEHOLDER: &str = "Ask NORTE to do anything...";

pub(crate) const COMMAND_GROUP_TAB: &str = "tab";
pub(crate) const COMMAND_GROUP_VIEW: &str = "view";
pub(crate) const COMMAND_GROUP_SESSION: &str = "session";
pub(crate) const COMMAND_CYCLE_THEME: &str = "Cycle theme";
pub(crate) const COMMAND_REFRESH: &str = "Refresh";
pub(crate) const COMMAND_NEW_SESSION: &str = "New session";
pub(crate) const COMMAND_COMPACT: &str = "Compact context";

pub(crate) fn switch_tab_command(tab: &str) -> String {
    format!("Go to {tab}")
}

pub(crate) const SESSIONS_CAPTION: &str = "sessions · newest first";
pub(crate) const SESSIONS_HINT: &str = "⏎ attach · n new · r rename · d delete";
pub(crate) const RENAME_TITLE: &str = "rename session";
pub(crate) const DELETE_TITLE: &str = "delete session";

pub(crate) fn delete_question(title: &str) -> String {
    format!("Delete {title}?")
}
pub(crate) const CONFIRM: &str = "confirm";
pub(crate) const CANCEL: &str = "cancel";
pub(crate) const TITLE_REQUIRED: &str = "a title is required";
pub(crate) const NO_SESSIONS: &str = "no sessions yet";

pub(crate) const KEY_PROMPT_TITLE: &str = "provider key";
pub(crate) const KEY_FIELD_LABEL: &str = "key";
pub(crate) const KEY_REQUIRED: &str = "a key is required";
pub(crate) const KEY_SAVE: &str = "save";
pub(crate) const KEY_SAVING: &str = "saving key...";

pub(crate) fn key_expectation(environment_keys: &[String]) -> String {
    if environment_keys.is_empty() {
        "this provider expects an API key".to_owned()
    } else {
        format!("expects {}", environment_keys.join(" or "))
    }
}

pub(crate) const ASK_USER: &str = "ask_user";
pub(crate) const ANSWER: &str = "answer";
pub(crate) const PLAN_REVIEW: &str = "plan review";
pub(crate) const FEEDBACK: &str = "feedback";

/// A row of key caps and labels, used for the status strip hints.
#[derive(Serialize)]
pub(crate) struct ShortcutHint {
    children: Vec<Node>,
}

impl ShortcutHint {
    pub(crate) fn chat(theme: &Theme) -> Self {
        let cap = KbdStyle {
            background: Some(theme.secondary),
            foreground: Some(theme.muted),
        };
        let label = TextStyle::default().fg(theme.muted);
        let children = vec![
            node("key_tabs", Kbd::new("Alt+0-9").with_style(cap.clone())),
            node("label_tabs", Text::new(TAB_HINT).with_style(label)),
            node("key_send", Kbd::new("↵").with_style(cap.clone())),
            node("label_send", Text::new(SEND).with_style(label)),
            node("key_newline", Kbd::new("⇧↵").with_style(cap)),
            node("label_newline", Text::new(NEWLINE).with_style(label)),
        ];
        Self { children }
    }

    pub(crate) fn restyle(&mut self, theme: &Theme) {
        let cap = KbdStyle {
            background: Some(theme.secondary),
            foreground: Some(theme.muted),
        };
        let label = TextStyle::default().fg(theme.muted);
        for child in &mut self.children {
            if let Some(kbd) = child.view.as_any().downcast_mut::<Kbd>() {
                kbd.style = cap.clone();
            } else if let Some(text) = child.view.as_any().downcast_mut::<Text>() {
                text.set_style(label);
            }
        }
    }
}

impl View for ShortcutHint {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "shortcut_hint"
    }

    fn children(&self) -> &[Node] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn measure(&self) -> Option<(u16, u16)> {
        let width: u16 = self
            .children
            .iter()
            .map(|child| child.view.measure().map_or(0, |(width, _)| width))
            .sum();
        Some((width.max(1), 1))
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        let mut x = area.x;
        for child in &mut self.children {
            let width = child.view.measure().map_or(0, |(width, _)| width);
            if x >= area.x + area.width || width == 0 {
                break;
            }
            let cell = Rect::new(x, area.y, width.min(area.x + area.width - x), 1);
            child.view.render(cell, buf, cx, theme);
            x += width;
        }
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}

#[cfg(test)]
#[path = "copy_tests.rs"]
mod tests;
