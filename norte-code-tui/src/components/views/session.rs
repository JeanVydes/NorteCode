use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
};

use crate::{
    components::{
        Component, ComponentAction, ComponentEvent, ComponentId, ComponentLifecycle, HeaderModal,
        ViewAction, palette,
    },
    keys,
    tools::DiffMode,
    ui_model::UiModel,
};

mod composer;
mod todos;
mod transcript;

use composer::{Composer, SessionFocus};

/// A menu opened over a turn with a right click.
#[derive(Clone)]
struct ContextMenu {
    key: transcript::TurnKey,
    selected: usize,
}

impl ContextMenu {
    /// How many items the menu offers.
    const ITEMS: usize = 3;

    /// The turn's id, the way the host names it for the rewind and pin
    /// actions: live turns are `turn_<number>`.
    fn turn_id(key: &transcript::TurnKey) -> String {
        match key {
            transcript::TurnKey::Live(turn) => format!("turn_{turn}"),
            transcript::TurnKey::Named(id) => id.clone(),
        }
    }

    /// A label for the turn: what the menu's title shows.
    fn label(key: &transcript::TurnKey) -> String {
        match key {
            transcript::TurnKey::Live(turn) => format!("turn {turn}"),
            transcript::TurnKey::Named(_) => "turn".to_owned(),
        }
    }
}

/// Draws the menu over the transcript.
fn render_context_menu(
    frame: &mut Frame<'_>,
    area: Rect,
    menu: &ContextMenu,
    folded: &[transcript::TurnKey],
    pinned_turns: &[String],
    theme: crate::ui_model::ThemeMode,
) {
    let fold_turn = folded.iter().any(|key| key == &menu.key);
    let turn_id = ContextMenu::turn_id(&menu.key);
    let pinned = pinned_turns.iter().any(|id| id == &turn_id);
    let labels = [
        "rewind to here".to_owned(),
        if pinned {
            "unpin this turn".to_owned()
        } else {
            "pin this turn".to_owned()
        },
        if fold_turn {
            "unfold this turn".to_owned()
        } else {
            "fold this turn".to_owned()
        },
    ];
    let line_count = u16::try_from(labels.len() + 1).unwrap_or(u16::MAX);
    let style = palette::modal_style(theme);
    let rect = crate::components::rect::centered_rect(
        32.min(area.width),
        line_count.min(area.height),
        area,
    );
    frame.render_widget(Clear, rect);
    let lines: Vec<Line<'static>> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            if index == menu.selected {
                Line::from(vec![
                    Span::styled(
                        "> ".to_owned(),
                        Style::default()
                            .fg(palette::brand())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        label.clone(),
                        palette::selected_style(theme).add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(Span::raw(format!("  {label}")))
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(
            Block::default()
                .title(format!(" {} ", ContextMenu::label(&menu.key)))
                .title_style(
                    Style::default()
                        .fg(palette::brand())
                        .add_modifier(Modifier::BOLD),
                )
                .style(style)
                .padding(Padding::horizontal(1)),
        ),
        rect,
    );
}

impl Default for SessionView {
    fn default() -> Self {
        Self {
            lifecycle: ComponentLifecycle::default(),
            composer: Composer::default(),
            scroll_from_bottom: 0,
            pending_action: None,
            import: None,
            shell_output_expanded: crate::ui_model::Toggle::Off,
            folded: Vec::new(),
            layout: transcript::TranscriptLayout::default(),
            context_menu: None,
            last_render_area: Rect::default(),
            animation_phase: 0,
        }
    }
}

pub(in crate::components) struct SessionView {
    lifecycle: ComponentLifecycle,
    composer: Composer,
    scroll_from_bottom: usize,
    pending_action: Option<ComponentAction>,
    /// The path of the bundle being imported, when the reader is typing one.
    import: Option<String>,
    /// Whether shell log boxes show everything rather than the last lines.
    shell_output_expanded: crate::ui_model::Toggle,
    /// The turns the reader folded away.
    folded: Vec<transcript::TurnKey>,
    /// What the last draw measured.
    layout: transcript::TranscriptLayout,
    /// A menu the reader opened over a turn with a right click.
    context_menu: Option<ContextMenu>,
    /// Where the view last drew, so the menu knows its bounds.
    last_render_area: Rect,
    /// The tick of the thinking sweep; only meaningful while thinking
    /// streams, the style freezes past that.
    animation_phase: u16,
}

impl Component for SessionView {
    fn id(&self) -> ComponentId {
        ComponentId::Session
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn captures_global_keys(&self) -> bool {
        self.composer.focus == SessionFocus::Composer
            || self.composer.budget_entry.is_some()
            || self.context_menu.is_some()
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        if let Some(menu) = self.context_menu.take() {
            self.handle_context_menu_key(key, model, menu);
            return;
        }
        if self.handle_budget_approval(key, model)
            || self.handle_budget_entry(key, model)
            || self.handle_import_entry(key, model)
        {
            return;
        }
        if self.composer.focus == SessionFocus::Composer {
            if let Some(action) = self.composer.handle_key(key, model) {
                self.pending_action = Some(action);
            }
        } else {
            self.handle_transcript_key(key, model);
        }
    }

    fn handle_keys(&mut self, keys: &[KeyEvent], model: &UiModel) {
        if self.composer.focus == SessionFocus::Composer
            && let Some(text) = pasted_text(keys)
        {
            self.composer.paste(&text);
            return;
        }
        for key in keys {
            self.handle_key(*key, model);
        }
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }

    fn accept(&mut self, event: ComponentEvent) {
        match event {
            ComponentEvent::Uploaded(chip) => self.composer.add_chip(chip),
            ComponentEvent::SessionOpened => {
                self.composer.reset();
                // The folds belong to the session that was open.
                self.folded.clear();
                self.context_menu = None;
            }
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, model: &UiModel) {
        if self.context_menu.is_some() {
            self.handle_context_menu_mouse(mouse, model);
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_transcript(3);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_transcript(-3);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(key) = self.turn_at(mouse.column, mouse.row) {
                    self.fold_turn_scrollable(model, &key);
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(key) = self.turn_at(mouse.column, mouse.row) {
                    self.context_menu = Some(ContextMenu { key, selected: 0 });
                }
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        self.last_render_area = area;
        // Advance the sweep only enough to feel alive; it animates only while
        // thinking streams, so the extra ticks cost nothing the rest of the time.
        self.animation_phase = self.animation_phase.wrapping_add(3) % area.width.max(8);
        // Two cases render a hint line: the import prompt and the budget
        // approval a lone other view does not carry.
        let hint = if self.import.is_some() {
            Some("Enter import · Esc cancel".to_owned())
        } else {
            Composer::hint(model)
        };
        let content = session_regions(
            area,
            todos::TodosPanel::height(&model.todos),
            self.composer.height(hint.is_some()),
        );
        self.layout = transcript::TranscriptRenderer::render(
            frame,
            content[0],
            transcript::TranscriptContext {
                items: &model.transcript,
                session: active_session_label(model),
                turn_state: model.turn_state,
                model: model.session_model.as_ref(),
                queued_model: model.queued_model.as_ref(),
                diff_mode: model.cli.diff_style,
                scroll_from_bottom: self.scroll_from_bottom,
                focused: self.composer.focus == SessionFocus::Transcript,
                thinking_visibility: model.preferences.thinking,
                show_tool_output: model.preferences.show_tool_output,
                show_timestamps: model.preferences.show_timestamps,
                theme: model.preferences.theme,
                attachment_state: model.attachment_state,
                developer_mode: model.preferences.developer_mode,
                shell_output_expanded: self.shell_output_expanded,
                thinking_phase: self.animation_phase,
                folded: &self.folded,
            },
        );
        let mut lines = if let Some(path) = &self.import {
            vec![Line::from(format!("Import bundle: {path}"))]
        } else {
            self.composer.visible_lines(model)
        };
        todos::TodosPanel::render(frame, content[1], &model.todos, model.preferences.theme);
        lines.extend(self.composer.chip_lines());
        if let Some(hint) = hint {
            lines.push(Line::from(Span::styled(hint, Composer::hint_style())));
        }
        let composer_style = Style::default()
            .fg(palette::foreground(model.preferences.theme))
            .bg(if self.composer.focus == SessionFocus::Composer {
                palette::composer_style(model.preferences.theme)
            } else {
                match model.preferences.theme {
                    crate::ui_model::ThemeMode::Dark => Color::Rgb(30, 33, 40),
                    crate::ui_model::ThemeMode::Light => Color::Rgb(236, 239, 244),
                }
            });
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(composer_style)
                .block(
                    // `px-4 py-1`: four cells of air on the sides, one above
                    // and below the text, behind a single rule.
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(palette::muted()))
                        .padding(Padding::new(4, 4, 1, 1))
                        .style(composer_style),
                ),
            content[2],
        );
        if let Some(menu) = &self.context_menu {
            render_context_menu(
                frame,
                area,
                menu,
                &self.folded,
                &model.pinned_turns,
                model.preferences.theme,
            );
        }
    }
}

impl SessionView {
    /// Scrolls the transcript by `lines` rows, negative towards the newest.
    fn scroll_transcript(&mut self, lines: isize) {
        if lines.is_negative() {
            self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(lines.unsigned_abs());
        } else {
            self.scroll_from_bottom = self
                .scroll_from_bottom
                .saturating_add(usize::try_from(lines).unwrap_or_default())
                .min(self.layout.max_scroll);
        }
    }

    /// The turn whose separator sits at the row the reader clicked.
    fn turn_at(&self, column: u16, row: u16) -> Option<transcript::TurnKey> {
        let Rect {
            x,
            y,
            width,
            height,
        } = self.layout.text_area;
        if column < x
            || column >= x.saturating_add(width)
            || row < y
            || row >= y.saturating_add(height)
        {
            return None;
        }
        let scroll = self
            .layout
            .max_scroll
            .saturating_sub(self.scroll_from_bottom);
        let absolute = usize::from(row - y) + scroll;
        self.layout
            .separators
            .iter()
            .find(|(row, _)| *row == absolute)
            .map(|(_, key)| key.clone())
    }

    /// The menu's keys: close it, move its cursor or run the chosen item.
    fn handle_context_menu_key(&mut self, key: KeyEvent, model: &UiModel, mut menu: ContextMenu) {
        match key.code {
            keys::CANCEL => {}
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                menu.selected = menu.selected.saturating_sub(1);
                self.context_menu = Some(menu);
            }
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                menu.selected = (menu.selected + 1).min(ContextMenu::ITEMS - 1);
                self.context_menu = Some(menu);
            }
            KeyCode::Enter => {
                self.confirm_context_menu(model, &menu.key, menu.selected);
            }
            _ => {
                self.context_menu = Some(menu);
            }
        }
    }

    /// The menu's mouse: click outside closes it, click on an item runs it.
    fn handle_context_menu_mouse(&mut self, mouse: MouseEvent, model: &UiModel) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let rect = crate::components::rect::centered_rect(
            32.min(self.last_render_area.width),
            4.min(self.last_render_area.height),
            self.last_render_area,
        );
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let position = ratatui::layout::Position {
                    x: mouse.column,
                    y: mouse.row,
                };
                if rect.contains(position) {
                    let item = mouse
                        .row
                        .checked_sub(rect.y + 1)
                        .map(|offset| offset as usize)
                        .filter(|index| *index < ContextMenu::ITEMS);
                    if let Some(index) = item {
                        self.confirm_context_menu(model, &menu.key, index);
                        return;
                    }
                    self.context_menu = Some(menu);
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {}
            _ => self.context_menu = Some(menu),
        }
    }

    /// Runs one item of the menu over a turn.
    fn confirm_context_menu(&mut self, model: &UiModel, key: &transcript::TurnKey, item: usize) {
        let turn_id = ContextMenu::turn_id(key);
        match item {
            0 => {
                self.pending_action = Some(ComponentAction::Rewind { turn_id });
            }
            1 => {
                let pinned = !model.pinned_turns.contains(&turn_id);
                self.pending_action = Some(ComponentAction::PinTurn { turn_id, pinned });
            }
            _ => self.fold_turn_scrollable(model, key),
        }
    }

    /// The options the transcript is drawn with.
    fn transcript_options<'a>(
        &'a self,
        model: &'a UiModel,
        width: u16,
    ) -> transcript::TranscriptOptions<'a> {
        transcript::TranscriptOptions {
            diff_mode: model.cli.diff_style,
            width,
            thinking_visibility: model.preferences.thinking,
            show_tool_output: model.preferences.show_tool_output,
            show_timestamps: model.preferences.show_timestamps,
            theme: model.preferences.theme,
            developer_mode: model.preferences.developer_mode,
            shell_output_expanded: self.shell_output_expanded,
            thinking_phase: self.animation_phase,
            thinking_streaming: matches!(model.turn_state, crate::ui_model::TurnState::Thinking),
            folded: &self.folded,
        }
    }

    /// The turn the reader is looking at: the one the viewport starts in.
    fn current_turn(&self, layout: &[transcript::TurnLayout]) -> Option<transcript::TurnKey> {
        let top = self
            .layout
            .max_scroll
            .saturating_sub(self.scroll_from_bottom);
        layout
            .iter()
            .rev()
            .find(|turn| turn.start <= top)
            .map(|turn| turn.key.clone())
    }

    /// Folds the turn the reader is looking at, or unfolds it.
    /// Folds the turn the reader is looking at, or unfolds it.
    fn toggle_fold(&mut self, model: &UiModel) {
        let before = transcript::turn_layout(
            &model.transcript,
            self.layout.width,
            self.transcript_options(model, self.layout.width),
        );
        let Some(current) = self.current_turn(&before) else {
            return;
        };
        self.fold_turn_scrollable(model, &current);
    }

    /// Folds a turn, or unfolds it, and keeps the reader where they were:
    /// what follows the turn moves up by the rows it lost.
    fn fold_turn_scrollable(&mut self, model: &UiModel, key: &transcript::TurnKey) {
        let before = transcript::turn_layout(
            &model.transcript,
            self.layout.width,
            self.transcript_options(model, self.layout.width),
        );
        if let Some(position) = self.folded.iter().position(|turn| turn == key) {
            self.folded.remove(position);
        } else {
            self.folded.push(key.clone());
        }
        let after = transcript::turn_layout(
            &model.transcript,
            self.layout.width,
            self.transcript_options(model, self.layout.width),
        );
        let lost = Self::turn_rows(&before, key).saturating_sub(Self::turn_rows(&after, key));
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(lost);
    }

    /// The rows a turn takes in a layout.
    fn turn_rows(layout: &[transcript::TurnLayout], key: &transcript::TurnKey) -> usize {
        let index = layout.iter().position(|turn| turn.key == *key);
        let Some(index) = index else {
            return 0;
        };
        let start = layout[index].start;
        let end = layout.get(index + 1).map_or(start, |next| next.start);
        end.saturating_sub(start)
    }

    /// Moves the reader to the turn before or after the one they are in.
    /// Moves the reader to the beginning of a turn, opening it when it is
    /// folded.
    fn jump_to_turn(&mut self, model: &UiModel, key: &transcript::TurnKey) {
        self.folded.retain(|folded| folded != key);
        let layout = transcript::turn_layout(
            &model.transcript,
            self.layout.width,
            self.transcript_options(model, self.layout.width),
        );
        let Some(turn) = layout.iter().find(|turn| turn.key == *key) else {
            return;
        };
        self.scroll_from_bottom = self.layout.max_scroll.saturating_sub(turn.start);
    }

    fn jump_turn(&mut self, model: &UiModel, step: isize) {
        let layout = transcript::turn_layout(
            &model.transcript,
            self.layout.width,
            self.transcript_options(model, self.layout.width),
        );
        let Some(current) = self.current_turn(&layout) else {
            return;
        };
        let Some(index) = layout.iter().position(|turn| turn.key == current) else {
            return;
        };
        let target = index.cast_signed() + step;
        let Some(turn) = usize::try_from(target)
            .ok()
            .and_then(|index| layout.get(index))
        else {
            return;
        };
        self.jump_to_turn(model, &turn.key);
    }
}

impl SessionView {
    fn handle_budget_approval(&mut self, key: KeyEvent, model: &UiModel) -> bool {
        if model.budget_approval.is_none() {
            return false;
        }
        self.pending_action = match key.code {
            KeyCode::Char('a') => Some(ComponentAction::ApproveBudget),
            KeyCode::Char('d') | keys::CANCEL => Some(ComponentAction::DeclineBudget),
            _ => None,
        };
        true
    }

    fn handle_import_entry(&mut self, key: KeyEvent, _model: &UiModel) -> bool {
        if self.import.is_none() {
            return false;
        }
        match key.code {
            keys::CANCEL => self.import = None,
            keys::SUBMIT => {
                if let Some(path) = self.import.take() {
                    let path = path.trim().to_owned();
                    if !path.is_empty() {
                        self.pending_action = Some(ComponentAction::ImportSession(path));
                    }
                }
            }
            keys::ERASE => {
                if let Some(path) = &mut self.import {
                    path.pop();
                }
            }
            KeyCode::Char(character) => {
                if let Some(path) = &mut self.import {
                    path.push(character);
                }
            }
            _ => {}
        }
        true
    }

    fn handle_budget_entry(&mut self, key: KeyEvent, model: &UiModel) -> bool {
        if self.composer.budget_entry.is_none() {
            return false;
        }
        match key.code {
            keys::CANCEL => self.composer.budget_entry = None,
            keys::SUBMIT => {
                if let Some(limit) = self
                    .composer
                    .budget_entry
                    .as_deref()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|limit| limit.is_finite() && *limit > 0.0)
                {
                    self.pending_action = Some(ComponentAction::SetBudget {
                        limit,
                        action: model.budget_action,
                    });
                    self.composer.budget_entry = None;
                }
            }
            keys::ERASE => {
                if let Some(value) = &mut self.composer.budget_entry {
                    value.pop();
                }
            }
            KeyCode::Char(character) if character.is_ascii_digit() || character == '.' => {
                if let Some(value) = &mut self.composer.budget_entry {
                    value.push(character);
                }
            }
            _ => {}
        }
        true
    }

    fn handle_transcript_key(&mut self, key: KeyEvent, model: &UiModel) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if model.turn_state.is_busy() {
                self.pending_action = Some(ComponentAction::CancelTurn);
            }
            return;
        }
        match key.code {
            keys::FOCUS_NEXT => {
                self.composer.focus = SessionFocus::Composer;
            }
            keys::COMPOSER if model.active_child.is_none() => {
                self.composer.focus = SessionFocus::Composer;
            }
            keys::QUEUE_MODE => {
                self.pending_action = Some(ComponentAction::OpenHeaderModal(HeaderModal::Queue));
            }
            KeyCode::Char('o') => {
                self.shell_output_expanded = self.shell_output_expanded.toggled();
            }
            keys::FOLD => {
                self.toggle_fold(model);
            }
            keys::PREV_TURN => {
                self.jump_turn(model, -1);
            }
            keys::NEXT_TURN => {
                self.jump_turn(model, 1);
            }
            KeyCode::Char('n') if model.active_child.is_none() => {
                self.pending_action = Some(ComponentAction::CreateSession);
            }
            KeyCode::Char('I') if model.active_child.is_none() => {
                self.import = Some(String::new());
            }
            KeyCode::Char(keys::BUDGET_CHAR) if model.active_child.is_none() => {
                self.composer.budget_entry = Some(String::new());
            }
            KeyCode::Char(keys::EXPORT_CHAR) if model.active_child.is_none() => {
                self.pending_action = Some(ComponentAction::View(ViewAction::ExportSession));
            }
            KeyCode::Char(keys::COMPACT_CHAR) if model.active_child.is_none() => {
                self.pending_action = Some(ComponentAction::View(ViewAction::CompactSession));
            }
            keys::DIFF => {
                self.pending_action =
                    Some(ComponentAction::SetDiffStyle(match model.cli.diff_style {
                        DiffMode::Unified => DiffMode::Split,
                        DiffMode::Split => DiffMode::Unified,
                    }));
            }
            KeyCode::Char(keys::CANCEL_TURN_CHAR) => {
                self.pending_action = Some(ComponentAction::CancelTurn);
            }
            KeyCode::Char(keys::REASONING_CHAR) if model.active_child.is_none() => {
                self.pending_action =
                    Some(ComponentAction::OpenHeaderModal(HeaderModal::Reasoning));
            }
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(1);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                self.scroll_from_bottom = self
                    .scroll_from_bottom
                    .saturating_add(1)
                    .min(self.layout.max_scroll);
            }
            _ => {}
        }
    }
}

/// The text of a burst of keys that a terminal delivered as a paste.
fn pasted_text(keys: &[KeyEvent]) -> Option<String> {
    if keys.len() < 2 {
        return None;
    }
    let mut text = String::new();
    for key in keys {
        if !key.modifiers.is_empty() {
            return None;
        }
        let KeyCode::Char(character) = key.code else {
            return None;
        };
        text.push(character);
    }
    Some(text)
}

fn session_regions(area: Rect, todo_lines: u16, composer_lines: u16) -> [Rect; 3] {
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(todo_lines),
            Constraint::Length(composer_lines),
        ])
        .split(area);
    [regions[0], regions[1], regions[2]]
}

fn active_session_label(model: &UiModel) -> Option<&str> {
    let active = model.active_session.as_ref()?;
    if let Some(session) = model
        .sessions
        .iter()
        .find(|session| session.session_id == active.as_str())
    {
        return Some(session.title.as_deref().unwrap_or("Untitled session"));
    }
    if let Some(child) = model
        .children
        .iter()
        .find(|child| child.session_id == active.as_str())
    {
        return Some(child.title.as_deref().unwrap_or("subagent"));
    }
    Some(active.as_str())
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
