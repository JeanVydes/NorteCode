//! The models page: every catalogued provider and model, filterable.
//!
//! Typing filters providers by name and models by name or id. A provider
//! that names the query stays open with all of its models; a provider that
//! does not, shows only the models that match. Providers with nothing
//! matching are hidden. Enter on a provider toggles its models; Enter on a
//! model selects it, asking for the provider's key first when there is no
//! key on disk or in the environment. Ctrl+D (or Ctrl+Delete where the
//! terminal reports it) asks to remove the selected provider's saved key
//! after a confirmation.

use std::any::Any;
use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use serde::Serialize;
use terminalui::components::autocomplete::Autocomplete;
use terminalui::components::button::{Button, ButtonVariant};
use terminalui::components::dialog::Dialog;
use terminalui::components::table::{Table, TableCell, TableRow};
use terminalui::{Context, InputEvent, Node, Outcome, Stack, Theme, View, node};

use super::copy::{
    CANCEL, CONFIRM, DELETE_KEY_TITLE, MODELS_KEY_HINT, delete_key_question,
    key_source_label as describe_key_source,
};
use super::detail_panel::{DetailPanel, Field};
use super::node_lookup::find_node_mut;

const TABLE_INDEX: usize = 0;
const DETAIL_INDEX: usize = 1;
const DELETE_DIALOG_INDEX: usize = 2;

/// The interaction mode of the page.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
enum Mode {
    /// Browsing the catalogue.
    #[default]
    Listing,
    /// Confirming the removal of a provider's saved key.
    ConfirmDelete {
        /// The provider whose key would be removed.
        provider: String,
    },
}

/// Which button in the delete confirmation is selected.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
enum DeleteChoice {
    /// Keep the key.
    Cancel,
    /// Remove the key.
    #[default]
    Confirm,
}

impl DeleteChoice {
    fn button_id(self) -> &'static str {
        match self {
            Self::Cancel => "delete_key_cancel",
            Self::Confirm => "delete_key_confirm",
        }
    }
}

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

/// What the host knows about one provider's key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderKeyStatus {
    /// Whether the row should present the key as usable.
    pub(crate) available: bool,
    /// Where the host resolved the key from.
    pub(crate) source: norte_code_wire::ProviderKeySource,
    /// The environment variables the host checks for this provider.
    pub(crate) environment_keys: Vec<String>,
}

impl ProviderKeyStatus {
    fn removable(&self) -> bool {
        self.source == norte_code_wire::ProviderKeySource::File
    }
}

/// The catalogue model entries, keyed by nothing but the provider id.
pub(crate) struct ModelsData {
    /// The catalogue models to list.
    pub(crate) models: Vec<norte_code_wire::ModelInfo>,
    /// The key status of each provider.
    pub(crate) key_status: std::collections::HashMap<String, ProviderKeyStatus>,
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
    /// Remove the provider's saved key from the host.
    DeleteKey {
        /// The provider.
        provider: String,
    },
}

/// The catalogue picker.
#[derive(Serialize)]
pub(crate) struct ModelsPage {
    /// The models of the attached server's catalogue.
    models: Vec<norte_code_wire::ModelInfo>,
    /// What each provider's key is and where it comes from.
    #[serde(skip)]
    key_status: std::collections::HashMap<String, ProviderKeyStatus>,
    /// The model the attached session uses.
    #[serde(skip)]
    active: Option<(String, String)>,
    /// The filter typed by the user.
    filter: Autocomplete,
    /// The expanded providers.
    expanded: HashSet<String>,
    /// The selected row index.
    selected: usize,
    /// The last drawn area.
    #[serde(skip)]
    area: Rect,
    /// What the page is doing over the catalogue.
    #[serde(default)]
    mode: Mode,
    /// Which delete confirmation button is selected.
    #[serde(default)]
    dialog_choice: DeleteChoice,
    /// Actions to drain.
    #[serde(skip)]
    pending: Vec<ModelsAction>,
    /// The catalogue table and the selected row's detail panel.
    children: Vec<Node>,
}

impl ModelsPage {
    /// Creates a page with an empty catalogue.
    pub(crate) fn new() -> Self {
        Self {
            models: Vec::new(),
            key_status: std::collections::HashMap::new(),
            active: None,
            filter: Autocomplete::new(Vec::new()),
            expanded: HashSet::new(),
            selected: 0,
            area: Rect::new(0, 0, 0, 0),
            mode: Mode::Listing,
            dialog_choice: DeleteChoice::default(),
            pending: Vec::new(),
            children: vec![
                node(
                    "models_table",
                    Table::new().with_header(node(
                        "header",
                        TableRow::header(vec![
                            node("h-name", TableCell::text("provider / model").with_weight(6)),
                            node("h-context", TableCell::text("context").with_weight(2)),
                            node("h-tools", TableCell::text("tools").with_weight(1)),
                        ]),
                    )),
                ),
                node("model_detail", DetailPanel::new()),
                node("delete_key_dialog", Dialog::new(Vec::new())),
            ],
        }
    }

    fn delete_dialog(provider: &str) -> Dialog {
        Dialog::new(vec![node(
            "actions",
            Stack::horizontal().gap(2).children(vec![
                node(
                    "delete_key_cancel",
                    Button::new(ButtonVariant::Secondary, CANCEL),
                ),
                node(
                    "delete_key_confirm",
                    Button::new(ButtonVariant::Destructive, CONFIRM),
                ),
            ]),
        )])
        .with_title(DELETE_KEY_TITLE)
        .with_message(delete_key_question(provider))
    }

    fn open_delete(&mut self, provider: &str) {
        if !self.key_is_removable(provider) {
            return;
        }
        if let Some(dialog) = self
            .children
            .get_mut(DELETE_DIALOG_INDEX)
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
        {
            *dialog = Self::delete_dialog(provider);
            dialog.open();
        }
        self.mode = Mode::ConfirmDelete {
            provider: provider.to_owned(),
        };
        self.set_dialog_choice(DeleteChoice::Confirm);
    }

    /// The view the host should focus while a dialog is open.
    pub(crate) fn focus_id(&self) -> Option<&'static str> {
        match self.mode {
            Mode::Listing => None,
            Mode::ConfirmDelete { .. } => Some(self.dialog_choice.button_id()),
        }
    }

    /// Whether the delete confirmation is open.
    pub(crate) fn confirming_delete(&self) -> bool {
        matches!(self.mode, Mode::ConfirmDelete { .. })
    }

    /// Whether the highlighted provider's key is one a host can remove.
    pub(crate) fn can_delete_selected_key(&self) -> bool {
        self.selected_provider()
            .is_some_and(|provider| self.key_is_removable(&provider))
    }

    fn key_is_removable(&self, provider: &str) -> bool {
        self.key_status
            .get(provider)
            .is_some_and(ProviderKeyStatus::removable)
    }

    fn delete_provider(&self) -> Option<String> {
        match &self.mode {
            Mode::Listing => None,
            Mode::ConfirmDelete { provider } => Some(provider.clone()),
        }
    }

    /// Switch the selected button in the delete confirmation.
    pub(crate) fn toggle_dialog_choice(&mut self) {
        let next = match self.dialog_choice {
            DeleteChoice::Cancel => DeleteChoice::Confirm,
            DeleteChoice::Confirm => DeleteChoice::Cancel,
        };
        self.set_dialog_choice(next);
    }

    fn dialog_button_mut(&mut self, button_id: &str) -> Option<&mut Button> {
        self.children
            .get_mut(DELETE_DIALOG_INDEX)
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
            .and_then(|dialog| find_node_mut(dialog.children_mut(), button_id))
            .and_then(|node| node.view.as_any().downcast_mut::<Button>())
    }

    fn set_dialog_choice(&mut self, choice: DeleteChoice) {
        self.dialog_choice = choice;
        for id in [
            DeleteChoice::Cancel.button_id(),
            DeleteChoice::Confirm.button_id(),
        ] {
            if let Some(button) = self.dialog_button_mut(id) {
                button.state.active = choice.button_id() == id;
            }
        }
    }

    fn dialog_button_pressed(&mut self, button_id: &str) -> bool {
        self.dialog_button_mut(button_id)
            .is_some_and(Button::take_pressed)
    }

    fn dialog_open(&self) -> bool {
        self.children
            .get(DELETE_DIALOG_INDEX)
            .and_then(|node| node.view.as_any_ref().downcast_ref::<Dialog>())
            .is_some_and(Dialog::is_open)
    }

    /// Close a finished dialog and turn its outcome into an action.
    fn drain_modal(&mut self) {
        let Mode::ConfirmDelete { provider } = self.mode.clone() else {
            return;
        };
        let confirmed = self.dialog_button_pressed(DeleteChoice::Confirm.button_id());
        let cancelled = self.dialog_button_pressed(DeleteChoice::Cancel.button_id());
        if !confirmed && !cancelled && self.dialog_open() {
            return;
        }
        self.finish_delete(&provider, confirmed);
    }

    fn finish_delete(&mut self, provider: &str, confirmed: bool) {
        if let Some(dialog) = self
            .children
            .get_mut(DELETE_DIALOG_INDEX)
            .and_then(|node| node.view.as_any().downcast_mut::<Dialog>())
        {
            dialog.close();
        }
        self.mode = Mode::Listing;
        if confirmed {
            self.pending.push(ModelsAction::DeleteKey {
                provider: provider.to_owned(),
            });
        }
    }

    fn selected_provider(&self) -> Option<String> {
        match self.rows().into_iter().nth(self.selected)? {
            Row::Provider { name, .. } => Some(name),
            Row::Model { model } => Some(model.provider),
        }
    }

    /// Replace the catalogue data; selection is preserved when possible.
    pub(crate) fn set_data(&mut self, data: ModelsData) {
        if self.models == data.models
            && self.key_status == data.key_status
            && self.active == data.active
        {
            return;
        }
        self.models = data.models;
        self.key_status = data.key_status;
        self.active = data.active;
        let mut suggestions: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for model in &self.models {
            if seen.insert(model.provider.clone()) {
                suggestions.push(model.provider.clone());
            }
            suggestions.push(model.model.clone());
            suggestions.push(model.name.clone());
        }
        self.filter.set_suggestions(suggestions);
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
        self.clamp_view();
    }

    /// Take any pending requests the host should action.
    pub(crate) fn take_pending(&mut self) -> Vec<ModelsAction> {
        self.drain_modal();
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
        self.filter.value().trim().to_lowercase()
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
            let expanded = self.is_expanded(&provider);
            let signed_in = self.key_status(&provider);
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
        self.selected = self.selected.min(self.last_row());
    }

    fn body_rows(&self) -> usize {
        self.area.height.saturating_sub(3) as usize
    }

    fn key_status(&self, provider: &str) -> bool {
        self.key_status
            .get(provider)
            .is_some_and(|status| status.available)
    }

    fn key_source(&self, provider: &str) -> String {
        self.key_status.get(provider).map_or_else(
            || describe_key_source(norte_code_wire::ProviderKeySource::Unknown, &[]),
            |status| describe_key_source(status.source, &status.environment_keys),
        )
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

    fn handle_delete_key(&mut self, key: &crossterm::event::KeyEvent) -> Outcome {
        use crossterm::event::KeyCode;
        let Some(provider) = self.delete_provider() else {
            return Outcome::Ignored;
        };
        match key.code {
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h' | 'k') => {
                self.set_dialog_choice(DeleteChoice::Cancel);
                Outcome::Consumed
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l' | 'j') => {
                self.set_dialog_choice(DeleteChoice::Confirm);
                Outcome::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.finish_delete(&provider, self.dialog_choice == DeleteChoice::Confirm);
                Outcome::Consumed
            }
            KeyCode::Esc => {
                self.finish_delete(&provider, false);
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }

    fn handle_key(&mut self, key: &crossterm::event::KeyEvent) -> Outcome {
        use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
        if key.kind != KeyEventKind::Press {
            return Outcome::Ignored;
        }
        if self.confirming_delete() {
            return self.handle_delete_key(key);
        }
        if self.mode != Mode::Listing {
            return Outcome::Ignored;
        }
        if key.modifiers == KeyModifiers::CONTROL
            && matches!(key.code, KeyCode::Delete | KeyCode::Char('d'))
        {
            if let Some(provider) = self.selected_provider() {
                self.open_delete(&provider);
            }
            return Outcome::Consumed;
        }
        if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.select(self.selected.saturating_sub(1));
                    return Outcome::Consumed;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.select(self.selected + 1);
                    return Outcome::Consumed;
                }
                KeyCode::PageUp => {
                    self.select(self.selected.saturating_sub(self.body_rows().max(1)));
                    return Outcome::Consumed;
                }
                KeyCode::PageDown => {
                    self.select(self.selected + self.body_rows().max(1));
                    return Outcome::Consumed;
                }
                KeyCode::Home => {
                    self.select(0);
                    return Outcome::Consumed;
                }
                KeyCode::End => {
                    self.select(self.last_row());
                    return Outcome::Consumed;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.activate();
                    return Outcome::Consumed;
                }
                KeyCode::Backspace => {
                    if self.filter.value().is_empty() {
                        return Outcome::Ignored;
                    }
                    let before = self.filter.value().to_owned();
                    let outcome = self.filter.handle_key(key);
                    if self.filter.value() != before {
                        self.selected = 0;
                        self.clamp_view();
                    }
                    return outcome;
                }
                KeyCode::Esc => {
                    if self.filter.value().is_empty() {
                        return Outcome::Ignored;
                    }
                    self.filter.clear();
                    self.selected = 0;
                    self.clamp_view();
                    return Outcome::Consumed;
                }
                KeyCode::Char(_) => {
                    let before = self.filter.value().to_owned();
                    let outcome = self.filter.handle_key(key);
                    if self.filter.value() != before {
                        self.selected = 0;
                        self.clamp_view();
                    }
                    return outcome;
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

    fn last_row(&self) -> usize {
        self.rows().len().saturating_sub(1)
    }

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.last_row());
        self.clamp_view();
    }

    fn is_expanded(&self, provider: &str) -> bool {
        !self.query().is_empty() || self.expanded.contains(provider)
    }

    fn model_detail_tail(source: String) -> Vec<Field> {
        vec![
            Field::new("source", source),
            Field::new("hint", MODELS_KEY_HINT),
        ]
    }

    fn provider_detail_fields(&self, name: &str, count: usize, signed_in: bool) -> Vec<Field> {
        let source = self.key_source(name);
        vec![
            Field::new("provider", name.to_owned()),
            Field::new("models", count.to_string()),
            Field::new("key", if signed_in { "available" } else { "missing" }),
            Field::new("source", source),
            Field::new("hint", MODELS_KEY_HINT),
        ]
    }

    fn sync_detail(&mut self) {
        let rows = self.rows();
        let fields = rows.into_iter().nth(self.selected).map_or_else(
            || vec![Field::new("no model", "nothing selected")],
            |row| match row {
                Row::Provider {
                    name,
                    count,
                    signed_in,
                } => self.provider_detail_fields(&name, count, signed_in),
                Row::Model { model } => {
                    let source = self.key_source(&model.provider);
                    let mut fields = vec![
                        Field::new("provider", model.provider),
                        Field::new("id", model.model),
                        Field::new("name", model.name),
                    ];
                    if let Some(description) = model.description.filter(|d| !d.is_empty()) {
                        fields.push(Field::new("about", description));
                    }
                    fields.push(Field::new(
                        "context",
                        model
                            .max_context
                            .map_or_else(|| "unknown".to_owned(), crate::context::human_count),
                    ));
                    fields.push(Field::new(
                        "output",
                        model
                            .max_output
                            .map_or_else(|| "unknown".to_owned(), crate::context::human_count),
                    ));
                    fields.push(Field::new("tools", model.tools.to_string()));
                    if !model.input.is_empty() {
                        fields.push(Field::new("input", format!("{:?}", model.input)));
                    }
                    if !model.output.is_empty() {
                        fields.push(Field::new("output", format!("{:?}", model.output)));
                    }
                    if model.reasoning.supported {
                        fields.push(Field::new("reasoning", "supported"));
                    }
                    if model.deprecated {
                        fields.push(Field::new("status", "deprecated"));
                    }
                    fields.extend(Self::model_detail_tail(source));
                    fields
                }
            },
        );
        let title = "model";
        if let Some(panel) = self
            .children
            .get_mut(DETAIL_INDEX)
            .and_then(|node| node.view.as_any().downcast_mut::<DetailPanel>())
        {
            panel.set(title, fields);
        }
    }
}

impl ModelsPage {
    fn table_rows(&self, rows: &[Row]) -> Vec<Node> {
        rows.iter()
            .map(|row| match row {
                Row::Provider {
                    name,
                    count,
                    signed_in,
                } => {
                    let expanded = !self.query().is_empty() || self.expanded.contains(name);
                    let marker = if expanded { "▾" } else { "▸" };
                    let key = if *signed_in { "✓" } else { "·" };
                    node(
                        format!("provider-{name}"),
                        TableRow::body(vec![
                            node(
                                "name",
                                TableCell::text(format!("{marker} {key} {name}")).with_weight(6),
                            ),
                            node(
                                "context",
                                TableCell::text(format!("{count} models")).with_weight(2),
                            ),
                            node("tools", TableCell::text("").with_weight(1)),
                        ]),
                    )
                }
                Row::Model { model } => {
                    let current = self.active.as_ref().is_some_and(|(provider, id)| {
                        provider == &model.provider && id == &model.model
                    });
                    let marker = if current { "●" } else { " " };
                    let label = if model.deprecated {
                        format!("{marker} {} (deprecated)", model.name)
                    } else {
                        format!("{marker} {}", model.name)
                    };
                    node(
                        format!("model-{}-{}", model.provider, model.model),
                        TableRow::body(vec![
                            node("name", TableCell::text(label).with_weight(6)),
                            node(
                                "context",
                                TableCell::text(
                                    model.max_context.map_or_else(
                                        || "–".to_owned(),
                                        crate::context::human_count,
                                    ),
                                )
                                .with_weight(2),
                            ),
                            node(
                                "tools",
                                TableCell::text(if model.tools { "yes" } else { "–" }.to_owned())
                                    .with_weight(1),
                            ),
                        ]),
                    )
                }
            })
            .collect()
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

    fn children(&self) -> &[Node] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Node] {
        &mut self.children
    }

    fn measure(&self) -> Option<(u16, u16)> {
        None
    }

    fn focusable(&self) -> bool {
        self.mode == Mode::Listing
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        self.area = area;
        let muted = Style::default().fg(theme.muted);
        let filter_label = if self.filter.value().is_empty() {
            " / filter providers and models".to_string()
        } else {
            format!(" /{}  · Esc clears", self.filter.value())
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
        self.sync_detail();

        let detail_width = (area.width / 2)
            .clamp(30, 52)
            .min(area.width.saturating_sub(30));
        let table_width = area.width.saturating_sub(detail_width + 1);
        let table_rect = Rect {
            x: area.x,
            y: area.y + 1,
            width: table_width,
            height: area.height.saturating_sub(1),
        };
        let detail_rect = Rect {
            x: area.x + table_width + 1,
            y: area.y,
            width: detail_width,
            height: area.height,
        };

        let table_rows = self.table_rows(&rows);
        if let Some(table) = self
            .children
            .get_mut(TABLE_INDEX)
            .and_then(|node| node.view.as_any().downcast_mut::<Table>())
        {
            table.set_rows(table_rows);
            table.state.selected = self.selected.min(rows.len().saturating_sub(1));
        }
        if let Some(child) = self.children.get_mut(TABLE_INDEX) {
            child.view.render(table_rect, buf, cx, theme);
        }
        if let Some(child) = self.children.get_mut(DETAIL_INDEX) {
            child.view.render(detail_rect, buf, cx, theme);
        }
        if self.mode != Mode::Listing
            && let Some(dialog) = self.children.get_mut(DELETE_DIALOG_INDEX)
        {
            dialog.view.render(area, buf, cx, theme);
        }
    }

    fn handle_event(&mut self, event: &InputEvent, cx: &mut Context) -> Outcome {
        use crossterm::event::{MouseButton, MouseEventKind};

        let _ = cx.is_focused();
        match event {
            InputEvent::Mouse(mouse) => {
                if self.mode != Mode::Listing {
                    return Outcome::Ignored;
                }
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        self.selected = self.selected.saturating_sub(3);
                    }
                    MouseEventKind::ScrollDown => {
                        let last = self.rows().len().saturating_sub(1);
                        self.selected = (self.selected + 3).min(last);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let Some(table) = self.children[TABLE_INDEX]
                            .view
                            .as_any_ref()
                            .downcast_ref::<Table>()
                        else {
                            return Outcome::Ignored;
                        };
                        let area = table.state.area;
                        let body_top = area.y.saturating_add(2);
                        let row_width = area.width.saturating_sub(u16::from(
                            self.rows().len() > usize::from(area.height.saturating_sub(2)),
                        ));
                        if mouse.column < area.x
                            || mouse.column >= area.x.saturating_add(row_width)
                            || mouse.row < body_top
                            || mouse.row >= area.bottom()
                        {
                            return Outcome::Ignored;
                        }
                        let index = table.state.offset + usize::from(mouse.row - body_top);
                        if index >= self.rows().len() {
                            return Outcome::Ignored;
                        }
                        self.selected = index;
                        self.activate();
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

#[cfg(test)]
#[path = "models_page_tests.rs"]
mod tests;
