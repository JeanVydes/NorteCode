//! The models page: every catalogued provider and model, filterable.
//!
//! Typing filters providers by name and models by name or id. A provider
//! that names the query stays open with all of its models; a provider that
//! does not, shows only the models that match. Providers with nothing
//! matching are hidden. Enter on a provider toggles its models; Enter on a
//! model selects it, asking for the provider's key first when there is no
//! key on disk or in the environment.

use std::any::Any;
use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// How a provider is rendered: header row, then its models when expanded.
#[derive(Clone)]
pub(crate) enum Row {
    /// A provider header row.
    Provider {
        /// The provider id, e.g. `openrouter`.
        name: String,
        /// How many catalogue models the provider offers.
        count: usize,
        /// Whether the provider has a usable key.
        signed_in: bool,
    },
    /// One model under a provider.
    Model {
        /// The catalogue model.
        model: norte_code_wire::ModelInfo,
    },
}

/// The catalogue model entries, keyed by nothing but the provider id.
pub(crate) struct ModelsData {
    /// The catalogue models to list.
    pub(crate) models: Vec<norte_code_wire::ModelInfo>,
    /// Whether each provider has a usable key.
    pub(crate) key_status: std::collections::HashMap<String, bool>,
    /// The model the attached session uses, `(provider, id)` pairs.
    pub(crate) active: Option<(String, String)>,
}

/// A pending request to the app layer, drained each frame.
#[derive(Serialize)]
pub(crate) enum ModelsAction {
    /// Send a `SetModel` command for a model whose provider has a key.
    SetModel {
        /// The provider.
        provider: String,
        /// The model id.
        model: String,
    },
    /// Ask for the provider's key before selecting the model.
    AskKeyForModel {
        /// The provider.
        provider: String,
        /// The model id.
        model: String,
    },
    /// Ask for the provider's key before expanding its models.
    AskKeyForExpand {
        /// The provider.
        provider: String,
    },
    /// Toggle a provider's expansion.
    ToggleExpand {
        /// The provider.
        provider: String,
    },
}

/// The catalogue picker.
#[derive(Serialize)]
pub(crate) struct ModelsPage {
    /// The models of the attached server's catalogue.
    models: Vec<norte_code_wire::ModelInfo>,
    /// Whether each provider has a usable key.
    #[serde(skip)]
    key_status: std::collections::HashMap<String, bool>,
    /// The model the attached session uses.
    #[serde(skip)]
    active: Option<(String, String)>,
    /// The filter typed by the user.
    filter: String,
    /// The expanded providers.
    expanded: HashSet<String>,
    /// The selected row index.
    selected: usize,
    /// The first visible row.
    offset: usize,
    /// The last drawn area.
    #[serde(skip)]
    area: Rect,
    /// Actions to drain.
    #[serde(skip)]
    pending: Vec<ModelsAction>,
}

impl ModelsPage {
    /// Creates a page with an empty catalogue.
    pub(crate) fn new() -> Self {
        Self {
            models: Vec::new(),
            key_status: std::collections::HashMap::new(),
            active: None,
            filter: String::new(),
            expanded: HashSet::new(),
            selected: 0,
            offset: 0,
            area: Rect::new(0, 0, 0, 0),
            pending: Vec::new(),
        }
    }

    /// Replace the catalogue data; selection is preserved when possible.
    pub(crate) fn set_data(&mut self, data: ModelsData) {
        self.models = data.models;
        self.key_status = data.key_status;
        self.active = data.active;
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
        self.clamp_view();
    }

    /// Take any pending requests the host should action.
    pub(crate) fn take_pending(&mut self) -> Vec<ModelsAction> {
        std::mem::take(&mut self.pending)
    }

    /// Expand a provider, e.g. after its key prompt saved.
    pub(crate) fn expand(&mut self, provider: &str) {
        self.expanded.insert(provider.to_owned());
        self.clamp_view();
    }

    /// Toggle a provider's expansion; used by the host after a key prompt.
    pub(crate) fn toggle_expansion(&mut self, provider: &str) {
        self.toggle(provider);
        self.clamp_view();
    }

    fn query(&self) -> String {
        self.filter.trim().to_lowercase()
    }

    fn rows(&self) -> Vec<Row> {
        let query = self.query();
        let mut providers: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for model in &self.models {
            if seen.insert(model.provider.clone()) {
                providers.push(model.provider.clone());
            }
        }
        let mut rows = Vec::new();
        for provider in providers {
            let provider_matches = query.is_empty() || provider.to_lowercase().contains(&query);
            let all: Vec<_> = self
                .models
                .iter()
                .filter(|m| m.provider == provider)
                .cloned()
                .collect();
            let matching: Vec<_> = if provider_matches {
                all.clone()
            } else {
                all.into_iter()
                    .filter(|model| {
                        model.model.to_lowercase().contains(&query)
                            || model.name.to_lowercase().contains(&query)
                    })
                    .collect()
            };
            if !provider_matches && matching.is_empty() {
                continue;
            }
            let expanded = !query.is_empty() || self.expanded.contains(&provider);
            let signed_in = self.key_status.get(&provider).copied().unwrap_or(false);
            rows.push(Row::Provider {
                name: provider.clone(),
                count: matching.len(),
                signed_in,
            });
            if expanded {
                rows.extend(matching.into_iter().map(|model| Row::Model { model }));
            }
        }
        rows
    }

    fn clamp_view(&mut self) {
        let rows = self.rows();
        let body = self.body_rows();
        if rows.len() <= body {
            self.offset = 0;
            return;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + body {
            self.offset = self.selected + 1 - body;
        }
    }

    fn body_rows(&self) -> usize {
        self.area.height.saturating_sub(1) as usize
    }

    fn key_status(&self, provider: &str) -> bool {
        self.key_status.get(provider).copied().unwrap_or(false)
    }

    fn activate(&mut self) {
        let rows = self.rows();
        let Some(row) = rows.into_iter().nth(self.selected) else {
            return;
        };
        match row {
            Row::Provider { name, .. } => {
                if self.key_status(&name) {
                    self.pending
                        .push(ModelsAction::ToggleExpand { provider: name });
                } else {
                    self.pending
                        .push(ModelsAction::AskKeyForExpand { provider: name });
                }
            }
            Row::Model { model } => {
                if self.key_status(&model.provider) {
                    self.pending.push(ModelsAction::SetModel {
                        provider: model.provider.clone(),
                        model: model.model.clone(),
                    });
                } else {
                    self.pending.push(ModelsAction::AskKeyForModel {
                        provider: model.provider.clone(),
                        model: model.model.clone(),
                    });
                }
            }
        }
    }

    fn handle_key(&mut self, key: &crossterm::event::KeyEvent) -> Outcome {
        use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
        if key.kind != KeyEventKind::Press {
            return Outcome::Ignored;
        }
        if key.modifiers == KeyModifiers::NONE {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.selected = self.selected.saturating_sub(1);
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let last = self.rows().len().saturating_sub(1);
                    self.selected = (self.selected + 1).min(last);
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                KeyCode::PageUp => {
                    self.selected = self.selected.saturating_sub(self.body_rows().max(1));
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                KeyCode::PageDown => {
                    let last = self.rows().len().saturating_sub(1);
                    self.selected = (self.selected + self.body_rows().max(1)).min(last);
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                KeyCode::Home => {
                    self.selected = 0;
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                KeyCode::End => {
                    self.selected = self.rows().len().saturating_sub(1);
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.activate();
                    return Outcome::Consumed;
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.selected = 0;
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                KeyCode::Esc => {
                    if self.filter.is_empty() {
                        return Outcome::Ignored;
                    }
                    self.filter.clear();
                    self.selected = 0;
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                KeyCode::Char(character) => {
                    self.filter.push(character);
                    self.selected = 0;
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                _ => return Outcome::Ignored,
            }
        }
        Outcome::Ignored
    }

    fn toggle(&mut self, provider: &str) {
        if !self.expanded.remove(provider) {
            self.expanded.insert(provider.to_owned());
        }
    }
}

impl View for ModelsPage {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "models_page"
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
        self.area = area;
        let muted = Style::default().fg(theme.muted);
        let filter_label = if self.filter.is_empty() {
            " / filter providers and models".to_string()
        } else {
            format!(" /{}  · Esc clears", self.filter)
        };
        buf.set_stringn(
            area.x,
            area.y,
            &filter_label,
            filter_label.chars().count(),
            muted,
        );

        let rows = self.rows();
        self.clamp_view();
        let body = self.body_rows();
        let end = (self.offset + body).min(rows.len());
        for (offset, index) in (self.offset..end).enumerate() {
            let y = area.y + 1 + u16::try_from(offset).unwrap_or(0);
            let selected = self.selected == index;
            let row_style = if selected {
                Style::default()
                    .fg(theme.foreground)
                    .bg(theme.secondary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground)
            };
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
            match &rows[index] {
                Row::Provider {
                    name,
                    count,
                    signed_in,
                } => {
                    let expanded = !self.query().is_empty() || self.expanded.contains(name);
                    let marker = if expanded { "▾" } else { "▸" };
                    let key = if *signed_in { "✓ " } else { "  " };
                    let text = format!("{marker}{key}{name}  ·  {count} models");
                    buf.set_stringn(area.x + 1, y, &text, text.chars().count(), row_style);
                }
                Row::Model { model } => {
                    let current = self.active.as_ref().is_some_and(|(provider, id)| {
                        provider == &model.provider && id == &model.model
                    });
                    let suffix = if current {
                        "  current"
                    } else if model.deprecated {
                        "  deprecated"
                    } else {
                        ""
                    };
                    let text = format!("    {}  ({}){suffix}", model.name, model.model);
                    buf.set_stringn(area.x + 2, y, &text, text.chars().count(), row_style);
                }
            }
        }
        if rows.is_empty() {
            let text = if self.filter.is_empty() {
                "catalogue not loaded yet"
            } else {
                "nothing matches"
            };
            buf.set_stringn(area.x + 1, area.y + 1, text, text.chars().count(), muted);
        }
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        match event {
            InputEvent::Mouse(mouse) => {
                use crossterm::event::{MouseButton, MouseEventKind};
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        self.selected = self.selected.saturating_sub(3);
                    }
                    MouseEventKind::ScrollDown => {
                        let last = self.rows().len().saturating_sub(1);
                        self.selected = (self.selected + 3).min(last);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let y = mouse.row;
                        if y > self.area.y && y < self.area.y + self.area.height {
                            let index = self.offset + (y - self.area.y - 1) as usize;
                            if index < self.rows().len() {
                                self.selected = index;
                            }
                        }
                    }
                    _ => return Outcome::Ignored,
                }
                self.clamp_view();
                Outcome::Consumed
            }
            InputEvent::Key(key) => self.handle_key(key),
            _ => Outcome::Ignored,
        }
    }
}
