//! The masked key prompt, shown when the host knows a provider's key is
//! missing.
//!
//! Type the key, press Enter: it is written to the same 0600 `secrets.json`
//! every other Norte Code client reads. Esc cancels. Nothing leaves this
//! process beyond the file it writes.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius};
use terminalui::{Context, InputEvent, Outcome, Theme, View};

use norte_code_protocol::keys::{environment_value, read_secret, save_secret, secrets_path};

use super::key_prompt_error::KeyPromptError;

thread_local! {
    /// Disk lookups are rare but happen on every catalog refresh, so keep
    /// the result until a key is saved or the secrets file moves.
    static KEY_STATUS: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

pub(crate) fn cache_key(path: &Path, provider_id: &str) -> String {
    format!("{}:{}", path.display(), provider_id)
}

/// Whether a provider has a usable key, on disk or in the environment.
pub(crate) fn provider_has_key(provider_id: &str, environment_keys: &[String]) -> bool {
    if environment_keys.is_empty() || environment_value(environment_keys).is_some() {
        return true;
    }
    let Some(path) = secrets_path() else {
        return false;
    };
    let key = cache_key(&path, provider_id);
    if let Some(cached) = KEY_STATUS.with(|cache| cache.borrow().get(&key).copied()) {
        return cached;
    }
    let present = read_secret(&path, provider_id).is_some();
    KEY_STATUS.with(|cache| {
        cache.borrow_mut().insert(key, present);
    });
    present
}

/// Forget the cached status after writing a key to disk.
pub(crate) fn invalidate_key(provider_id: &str) {
    let Some(path) = secrets_path() else {
        return;
    };
    let key = cache_key(&path, provider_id);
    KEY_STATUS.with(|cache| {
        cache.borrow_mut().remove(&key);
    });
}

/// The masked prompt state.
#[derive(Debug, Default, Serialize)]
struct KeyPromptState {
    /// The provider id the key is for; `None` when the prompt is closed.
    provider: Option<String>,
    /// The environment keys the provider names, for display.
    environment_keys: Vec<String>,
}

/// What to do after the key is saved.
#[derive(Clone, Debug)]
pub(crate) enum KeyIntent {
    /// Save the key, then switch the session to this model.
    SwitchModel {
        /// The provider the model belongs to.
        provider: String,
        /// The model id.
        model: String,
    },
    /// Save the key, then expand the provider's models.
    Expand {
        /// The provider to expand.
        provider: String,
    },
}

/// A full-area scrim with a centered masked field; used when a provider the
/// user picked has no key.
#[derive(Default, Serialize)]
pub(crate) struct KeyPrompt {
    state: KeyPromptState,
    buffer: String,
    /// The last save attempt's error, shown until the prompt closes.
    #[serde(skip)]
    error: Option<KeyPromptError>,
    /// The intent that armed this prompt, cleared when saved.
    #[serde(skip)]
    intent: Option<KeyIntent>,
    /// The intent satisfied by the last save, waiting for the app.
    #[serde(skip)]
    saved: Option<KeyIntent>,
}

impl KeyPrompt {
    /// Creates a closed prompt.
    pub(crate) fn new() -> Self {
        Self {
            state: KeyPromptState::default(),
            buffer: String::new(),
            error: None,
            intent: None,
            saved: None,
        }
    }

    /// Open the prompt for `provider_id`.
    pub(crate) fn open(&mut self, provider_id: impl Into<String>, environment_keys: Vec<String>) {
        self.state = KeyPromptState {
            provider: Some(provider_id.into()),
            environment_keys,
        };
        self.buffer.clear();
        self.error = None;
        self.intent = None;
    }

    /// Open the prompt armed with `intent`; the intent is drained after the
    /// key is saved.
    pub(crate) fn open_with_intent(
        &mut self,
        provider_id: impl Into<String>,
        environment_keys: Vec<String>,
        intent: KeyIntent,
    ) {
        self.open(provider_id, environment_keys);
        self.intent = Some(intent);
    }

    /// Take the saved intent, when the key was just saved.
    pub(crate) fn take_saved(&mut self) -> Option<KeyIntent> {
        self.saved.take()
    }

    /// Whether the prompt is collecting input.
    pub(crate) fn is_open(&self) -> bool {
        self.state.provider.is_some()
    }

    /// The provider the prompt is open for, when open.
    pub(crate) fn provider(&self) -> Option<&str> {
        self.state.provider.as_deref()
    }

    /// Close the prompt; buffers and the pending intent are cleared.
    pub(crate) fn close(&mut self) {
        self.state = KeyPromptState::default();
        self.buffer.clear();
        self.error = None;
        self.intent = None;
    }

    fn masked_visual(&self) -> String {
        "•".repeat(self.buffer.chars().count())
    }

    /// Save the current buffer to `secrets.json`.
    ///
    /// # Errors
    /// Returns a short human-readable reason when the key cannot be stored.
    fn save(&mut self) -> Result<(), KeyPromptError> {
        let Some(provider) = self.provider().map(str::to_owned) else {
            return Err(KeyPromptError::NoProvider);
        };
        let Some(path) = secrets_path() else {
            return Err(KeyPromptError::NoSecretsPath);
        };
        save_secret(&path, &provider, &self.buffer)
            .map(|()| {
                invalidate_key(&provider);
                self.saved = self.intent.take();
                self.close();
            })
            .map_err(|error| KeyPromptError::Save(error.to_string()))
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

    fn children(&self) -> &[terminalui::Node] {
        &[]
    }

    fn children_mut(&mut self) -> &mut [terminalui::Node] {
        &mut []
    }

    fn measure(&self) -> Option<(u16, u16)> {
        None
    }

    fn focusable(&self) -> bool {
        self.is_open()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        if !self.is_open() {
            return;
        }
        let Some(provider) = self.provider().map(str::to_owned) else {
            return;
        };
        // Dim the rest of the screen with a solid backdrop.
        let dim = Style::default().bg(theme.background).fg(theme.muted);
        for row in area.y..area.y + area.height {
            buf.set_stringn(
                area.x,
                row,
                " ".repeat(area.width as usize),
                area.width as usize,
                dim,
            );
        }

        let chrome = Chrome {
            border: Some(Border {
                set: BorderSet::Rounded,
                color: Some(theme.accent),
                radius: Radius::Small,
            }),
            background: Some(theme.secondary),
            radius: Radius::Small,
            padding: Padding {
                horizontal: 2,
                vertical: 1,
            },
            border_gradient: None,
            fill_gradient: None,
        };
        let width = area.width.min(64);
        let height = 7;
        let x = area.x + (area.width - width) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let panel = Rect::new(x, y, width, height);
        let inner = terminalui::components::chrome::draw_box(buf, panel, &chrome, theme);

        let mut row = inner.y;
        let headline = format!("{provider} · no key on file");
        buf.set_stringn(
            inner.x,
            row,
            &headline,
            headline.chars().count(),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        );
        row += 1;
        let env_line = if self.state.environment_keys.is_empty() {
            "this provider expects an API key".to_string()
        } else {
            format!("expects {}", self.state.environment_keys.join(" or "))
        };
        buf.set_stringn(
            inner.x,
            row,
            &env_line,
            env_line.chars().count(),
            Style::default().fg(theme.muted),
        );
        row += 1;

        let visual = format!("{}▏", self.masked_visual());
        buf.set_stringn(
            inner.x,
            row,
            &visual,
            visual.chars().count(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        row += 1;

        let hint = if let Some(error) = &self.error {
            format!("save failed: {error}")
        } else {
            "enter: save · ctrl+v: paste · esc: cancel".to_string()
        };
        let hint_style = if self.error.is_some() {
            Style::default().fg(theme.destructive)
        } else {
            Style::default().fg(theme.muted)
        };
        buf.set_stringn(inner.x, row, &hint, hint.chars().count(), hint_style);
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        if !self.is_open() {
            return Outcome::Ignored;
        }
        match event {
            InputEvent::Paste(text) => {
                self.buffer.push_str(text);
                self.error = None;
                Outcome::Consumed
            }
            InputEvent::Key(key) => {
                use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
                if key.kind != KeyEventKind::Press {
                    return Outcome::Ignored;
                }
                match (key.modifiers, key.code) {
                    (KeyModifiers::NONE, KeyCode::Esc) => {
                        self.close();
                        Outcome::Consumed
                    }
                    (KeyModifiers::NONE, KeyCode::Enter) => {
                        if let Err(error) = self.save() {
                            self.error = Some(error);
                        }
                        Outcome::Consumed
                    }
                    (KeyModifiers::NONE, KeyCode::Backspace) => {
                        self.buffer.pop();
                        self.error = None;
                        Outcome::Consumed
                    }
                    (KeyModifiers::NONE, KeyCode::Char(character)) => {
                        self.buffer.push(character);
                        self.error = None;
                        Outcome::Consumed
                    }
                    _ => Outcome::Consumed,
                }
            }
            _ => Outcome::Consumed,
        }
    }
}
