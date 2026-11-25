//! The masked key prompt, shown when the host knows a provider's key is
//! missing.
//!
//! The prompt only collects the secret and reports it to the host. The host
//! owns the key store, so the TUI never writes a secret to disk itself.

use std::any::Any;
use std::fmt;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde::Serialize;

use terminalui::components::button::{Button, ButtonVariant};
use terminalui::components::dialog::Dialog;
use terminalui::components::form::FormField;
use terminalui::components::input::Input;
use terminalui::{Context, InputEvent, Node, Outcome, Stack, Text, TextStyle, Theme, View, node};

use super::copy::{
    CANCEL, KEY_FIELD_LABEL, KEY_PROMPT_TITLE, KEY_REQUIRED, KEY_SAVE, KEY_SAVING, key_expectation,
};
pub(crate) use super::key_prompt_action::{KeyIntent, KeyPromptAction};
use super::key_status::invalidate_key;
use super::node_lookup::{find_child_mut, find_node, find_node_mut};

/// A modal form that collects a provider key without persisting it.
#[derive(Serialize)]
pub(crate) struct KeyPrompt {
    provider: Option<String>,
    children: Vec<Node>,
    /// The intent that armed this prompt, drained after the host confirms.
    #[serde(skip)]
    intent: Option<KeyIntent>,
    /// Actions waiting for the host.
    #[serde(skip)]
    pending: Vec<KeyPromptAction>,
    /// A submit is already in flight; ignore further Enter presses.
    #[serde(skip)]
    saving: bool,
    /// Set by Enter or the save button, drained by [`KeyPrompt::drain`].
    #[serde(skip)]
    submit_requested: bool,
}

impl Default for KeyPrompt {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyPrompt {
    /// Creates a closed prompt.
    pub(crate) fn new() -> Self {
        Self {
            provider: None,
            children: vec![node("key_dialog", Dialog::new(Vec::new()))],
            intent: None,
            pending: Vec::new(),
            saving: false,
            submit_requested: false,
        }
    }

    fn dialog(environment_keys: &[String]) -> Dialog {
        Dialog::new(vec![
            node(
                "key_field",
                FormField::text("key", KEY_FIELD_LABEL).required_with_message(KEY_REQUIRED),
            ),
            node(
                "key_actions",
                Stack::horizontal().gap(2).children(vec![
                    node("key_cancel", Button::new(ButtonVariant::Secondary, CANCEL)),
                    node("key_confirm", Button::new(ButtonVariant::Primary, KEY_SAVE)),
                ]),
            ),
        ])
        .with_title(KEY_PROMPT_TITLE)
        .with_message(key_expectation(environment_keys))
    }

    /// Open the prompt for `provider_id`.
    pub(crate) fn open(&mut self, provider_id: impl Into<String>, environment_keys: &[String]) {
        if let Some(dialog) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
        {
            *dialog = Self::dialog(environment_keys);
            dialog.open();
        }
        self.provider = Some(provider_id.into());
        self.intent = None;
        self.pending.clear();
        self.saving = false;
        self.submit_requested = false;
    }

    /// Open the prompt armed with `intent`; the intent is drained after the
    /// host stores the key.
    pub(crate) fn open_with_intent(
        &mut self,
        provider_id: impl Into<String>,
        environment_keys: &[String],
        intent: KeyIntent,
    ) {
        self.open(provider_id, environment_keys);
        self.intent = Some(intent);
    }

    /// Whether the prompt is collecting input.
    pub(crate) fn is_open(&self) -> bool {
        self.provider.is_some()
    }

    /// The provider the prompt is open for, when open.
    pub(crate) fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    /// The view the host should focus while the prompt is open.
    pub(crate) fn focus_id(&self) -> Option<&'static str> {
        self.is_open().then_some("key:input")
    }

    /// Take the actions waiting for the host.
    pub(crate) fn take_action(&mut self) -> Option<KeyPromptAction> {
        self.pending.pop()
    }

    /// Close the prompt after the host stored the key and return its intent.
    pub(crate) fn saved(&mut self) -> Option<KeyIntent> {
        let provider = self.provider.take()?;
        invalidate_key(&provider);
        self.close();
        self.intent.take()
    }

    /// Keep the prompt open and show why the host refused the key.
    pub(crate) fn fail(&mut self, reason: impl fmt::Display, color: Color) {
        self.saving = false;
        self.set_message(reason.to_string(), TextStyle::default().fg(color));
    }

    fn close(&mut self) {
        if let Some(dialog) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
        {
            dialog.close();
        }
        self.provider = None;
        self.pending.clear();
        self.saving = false;
        self.submit_requested = false;
    }

    fn set_message(&mut self, message: String, style: TextStyle) {
        if let Some(dialog) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
            && let Some(message_node) = find_node_mut(dialog.children_mut(), "message")
            && let Some(text) = message_node.view.as_any().downcast_mut::<Text>()
        {
            text.set_text(message);
            text.set_style(style);
        }
    }

    /// Turn component outcomes into host actions.
    pub(crate) fn drain(&mut self) {
        let Some(provider) = self.provider.clone() else {
            return;
        };
        let open = self
            .children
            .first()
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Dialog>())
            .is_some_and(Dialog::is_open);
        let cancelled = find_child_mut::<Button>(self.children.first_mut(), "key_cancel")
            .is_some_and(Button::take_pressed);
        if cancelled || !open {
            self.close();
            self.pending.push(KeyPromptAction::Cancel);
            return;
        }
        if self.saving {
            return;
        }
        let confirmed = find_child_mut::<Button>(self.children.first_mut(), "key_confirm")
            .is_some_and(Button::take_pressed);
        if !confirmed && !self.submit_requested {
            return;
        }
        self.submit_requested = false;
        let field = find_child_mut::<FormField>(self.children.first_mut(), "key_field");
        let Some(field) = field else {
            return;
        };
        let key = field.value();
        if key.trim().is_empty() {
            field.set_error(Some(KEY_REQUIRED));
            return;
        }
        field.set_error(None::<String>);
        self.saving = true;
        self.pending.push(KeyPromptAction::Submit { provider, key });
    }
}

impl View for KeyPrompt {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "key_prompt"
    }

    fn children(&self) -> &[Node] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn measure(&self) -> Option<(u16, u16)> {
        None
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        if self.saving {
            self.set_message(KEY_SAVING.to_owned(), TextStyle::default().fg(theme.accent));
        }
        let Some(dialog) = self
            .children
            .first_mut()
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
        else {
            return;
        };
        if !dialog.is_open() {
            return;
        }
        dialog.render(area, buf, cx, theme);
        let Some(input) = find_node(&self.children, "key:input")
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Input>())
        else {
            return;
        };
        if input.state.value.is_empty() {
            return;
        }
        let field = input.state.area;
        for y in field.y..field.y + field.height {
            for x in field.x..field.x + field.width {
                let Some(cell) = buf.cell_mut((x, y)) else {
                    continue;
                };
                if cell.symbol() != " " && !cell.symbol().is_empty() {
                    cell.set_symbol("*");
                }
            }
        }
        let cursor = u16::try_from(input.state.cursor.saturating_sub(input.state.scroll_x))
            .unwrap_or(u16::MAX);
        let x = field.x.saturating_add(1).saturating_add(cursor);
        if field.height > 0
            && x < field.x + field.width
            && let Some(cell) = buf.cell_mut((x, field.y + field.height / 2))
        {
            cell.set_symbol("|");
        }
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        let InputEvent::Key(key) = event else {
            return Outcome::Ignored;
        };
        if self.provider.is_some()
            && key.kind == crossterm::event::KeyEventKind::Press
            && key.code == crossterm::event::KeyCode::Enter
        {
            self.submit_requested = true;
            return Outcome::Consumed;
        }
        Outcome::Ignored
    }
}
