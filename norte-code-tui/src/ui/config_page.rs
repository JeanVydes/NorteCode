//! The preferences tab: theme, thinking, send mode and diff layout.
//!
//! The page shows the current values and reports which row the user changed;
//! the host owns every preference and applies the change.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// The setting a row controls.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum Setting {
    /// The palette set.
    Theme,
    /// The reasoning effort sent with turns.
    Thinking,
    /// What a message sent during a turn does.
    SendMode,
    /// How edit diffs are drawn.
    DiffMode,
    /// Where the session rail sits.
    RailPosition,
}

impl Setting {
    const ALL: [Self; 5] = [
        Self::Theme,
        Self::Thinking,
        Self::SendMode,
        Self::DiffMode,
        Self::RailPosition,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Thinking => "thinking",
            Self::SendMode => "send mode",
            Self::DiffMode => "diff mode",
            Self::RailPosition => "rail",
        }
    }
}

/// A request to advance one setting.
#[derive(Serialize)]
pub(crate) enum ConfigAction {
    /// Cycle the value of this setting.
    Cycle(Setting),
}

/// The values to show, supplied by the host.
pub(crate) struct ConfigData {
    /// The selected theme's name.
    pub(crate) theme: String,
    /// The reasoning effort.
    pub(crate) thinking: String,
    /// The send mode.
    pub(crate) send_mode: String,
    /// The diff layout.
    pub(crate) diff_mode: String,
    /// The rail position.
    pub(crate) rail_position: String,
}

/// The preferences page.
#[derive(Serialize)]
pub(crate) struct ConfigPage {
    /// The selected row.
    selected: usize,
    /// Actions waiting for the host.
    #[serde(skip)]
    pending: Vec<ConfigAction>,
    /// The values from the host.
    #[serde(skip)]
    values: ConfigData,
}

impl ConfigPage {
    /// Creates a page with no values loaded.
    pub(crate) fn new() -> Self {
        Self {
            selected: 0,
            pending: Vec::new(),
            values: ConfigData {
                theme: String::new(),
                thinking: String::new(),
                send_mode: String::new(),
                diff_mode: String::new(),
                rail_position: String::new(),
            },
        }
    }

    /// Replace the displayed values.
    pub(crate) fn set_data(&mut self, values: ConfigData) {
        self.values = values;
    }

    /// Take any pending requests the host should action.
    pub(crate) fn take_pending(&mut self) -> Vec<ConfigAction> {
        std::mem::take(&mut self.pending)
    }

    fn value(&self, setting: Setting) -> &str {
        match setting {
            Setting::Theme => &self.values.theme,
            Setting::Thinking => &self.values.thinking,
            Setting::SendMode => &self.values.send_mode,
            Setting::DiffMode => &self.values.diff_mode,
            Setting::RailPosition => &self.values.rail_position,
        }
    }

    fn handle_key(&mut self, key: &crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
        if key.kind != KeyEventKind::Press
            || !matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
        {
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(Setting::ALL.len() - 1);
            }
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Left => {
                self.pending
                    .push(ConfigAction::Cycle(Setting::ALL[self.selected]));
            }
            _ => {}
        }
    }
}

impl View for ConfigPage {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "config_page"
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
        true
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        buf.set_style(area, Style::default().bg(theme.background));
        let caption = "config · preferences";
        buf.set_stringn(
            area.x + 1,
            area.y,
            caption,
            caption.len(),
            Style::default().fg(theme.muted),
        );
        for (index, setting) in Setting::ALL.iter().enumerate() {
            let y = area.y + 2 + u16::try_from(index).unwrap_or(0);
            if y >= area.y + area.height {
                break;
            }
            let selected = index == self.selected;
            if selected {
                buf.set_style(
                    Rect {
                        x: area.x,
                        y,
                        width: area.width,
                        height: 1,
                    },
                    Style::default().bg(theme.secondary),
                );
            }
            let label = setting.label();
            let label_style = if selected {
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
            buf.set_stringn(area.x + 2, y, label, label.len(), label_style);
            let value = self.value(*setting);
            buf.set_stringn(
                area.x + 16,
                y,
                value,
                value.chars().count(),
                Style::default().fg(theme.accent),
            );
        }
        let hint = "↑↓ select · Enter/←/→ change · 0-9 tabs";
        let y = area.y + area.height.saturating_sub(1);
        buf.set_stringn(
            area.x + 1,
            y,
            hint,
            hint.len(),
            Style::default().fg(theme.muted),
        );
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        use crossterm::event::KeyEventKind;
        match event {
            InputEvent::Key(key) if key.kind == KeyEventKind::Press => {
                self.handle_key(key);
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }
}

#[cfg(test)]
#[path = "config_page_tests.rs"]
mod tests;
