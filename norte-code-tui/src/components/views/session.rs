use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
};

use crate::{
    components::{
        Component, ComponentAction, ComponentEvent, ComponentId, ComponentLifecycle, palette,
        spacing,
    },
    keys,
    ui_model::UiModel,
};

mod composer;
mod todos;
mod transcript;

use composer::{Composer, SessionFocus};

#[derive(Default)]
pub(in crate::components) struct SessionView {
    lifecycle: ComponentLifecycle,
    composer: Composer,
    scroll_from_bottom: usize,
    pending_action: Option<ComponentAction>,
    /// The turns the reader folded away.
    folded: Vec<transcript::TurnKey>,
    /// The reasoning blocks the reader folded, or unfolded, one by one.
    thoughts: Vec<transcript::ThoughtFold>,
    /// What the last draw measured.
    layout: transcript::TranscriptLayout,
    /// The tick of the thinking sweep; only meaningful while thinking
    /// streams, the style freezes past that.
    animation_phase: u16,
    /// Where the composer was last drawn, so a click can focus it.
    composer_area: Rect,
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
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        if model.budget_approval.is_some() {
            match key.code {
                KeyCode::Char('a') => {
                    self.pending_action = Some(ComponentAction::ApproveBudget);
                }
                KeyCode::Char('d') | KeyCode::Esc => {
                    self.pending_action = Some(ComponentAction::DeclineBudget);
                }
                _ => {}
            }
            return;
        }
        if self.composer.focus == SessionFocus::Composer {
            if let Some(action) = self.composer.handle_key(key, model) {
                self.pending_action = Some(action);
            }
            return;
        }
        self.handle_transcript_key(key, model);
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
                self.folded.clear();
                self.thoughts.clear();
            }
            ComponentEvent::PinCurrentTurn => self.pin_viewport_turn(),
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent, model: &UiModel) {
        if mouse.kind == crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
            && self.composer_area.contains(ratatui::layout::Position {
                x: mouse.column,
                y: mouse.row,
            })
        {
            self.composer.focus = SessionFocus::Composer;
            return;
        }
        match mouse.kind {
            crossterm::event::MouseEventKind::ScrollUp => self.scroll_transcript(3),
            crossterm::event::MouseEventKind::ScrollDown => self.scroll_transcript(-3),
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                if let Some(key) = self.thought_at(mouse.column, mouse.row) {
                    self.toggle_thought_scrollable(model, &key);
                } else if let Some(key) = self.turn_at(mouse.column, mouse.row) {
                    self.fold_turn_scrollable(model, &key);
                }
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        self.animation_phase = self.animation_phase.wrapping_add(1);
        let hint: Option<String> = None;
        let content = session_regions(
            area,
            todos::TodosPanel::height(&model.todos),
            self.composer.height(hint.is_some()),
        );
        self.composer_area = content[2];
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
                clock: &model.clock,
                theme: model.preferences.theme,
                attachment_state: model.attachment_state,
                developer_mode: model.preferences.developer_mode,
                shell_output_expanded: model.shell_output_expanded,
                thinking_phase: self.animation_phase,
                folded: &self.folded,
                thoughts: &self.thoughts,
            },
        );

        let mut lines = self.composer.visible_lines(model);
        todos::TodosPanel::render(frame, content[1], &model.todos, model.preferences.theme);
        lines.extend(self.composer.chip_lines(model.preferences.theme));
        if let Some(hint) = hint {
            lines.push(Line::from(Span::styled(
                hint,
                palette::muted_style(model.preferences.theme),
            )));
        }
        let focused = self.composer.is_focused();
        let theme = model.preferences.theme;
        let composer_style = Style::default().fg(palette::text(theme)).bg(if focused {
            palette::composer_background(theme)
        } else {
            palette::composer_background_idle(theme)
        });
        let placeholder = self.composer.text.is_empty();
        let lines = if placeholder {
            // An idle composer says how to enter it, quietly; a focused one
            // says what the keys will do.
            let text = self.composer.placeholder(model);
            let style = if focused {
                palette::muted_style(theme)
            } else {
                Style::default().fg(palette::border(theme))
            };
            vec![Line::from(Span::styled(text, style))]
        } else {
            lines
        };
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(composer_style)
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(palette::composer_border_style(theme, focused))
                        .padding(Padding::new(
                            spacing::PANEL_PADDING,
                            spacing::PANEL_PADDING,
                            0,
                            0,
                        ))
                        .style(composer_style),
                ),
            content[2],
        );
        self.render_scrollbar(frame, content[0], model);
        self.render_newest_button(frame, content[0], model);
        render_budget_approval(frame, model);
    }
}

impl SessionView {
    /// The way back to the newest row, when the reader scrolled away from it.
    ///
    /// It sits at the bottom edge of the transcript, because that is where the
    /// reader who scrolled up is looking.
    fn render_newest_button(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        if self.scroll_from_bottom == 0 || area.height < 3 || area.width < 24 {
            return;
        }
        let theme = model.preferences.theme;
        let label = " newest  End ".to_owned();
        let width = u16::try_from(label.chars().count()).unwrap_or(0);
        let button = Rect {
            x: area.right().saturating_sub(width.saturating_add(2)),
            y: area.bottom().saturating_sub(2),
            width,
            height: 1,
        };
        frame.render_widget(
            ratatui::widgets::Paragraph::new(label).style(
                Style::default()
                    .fg(palette::background(theme))
                    .bg(palette::accent(theme))
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            button,
        );
    }

    /// Pins the turn the viewport starts in.
    fn pin_viewport_turn(&mut self) {
        let Some((_, key)) = self.layout.separators.first() else {
            return;
        };
        let turn_id = match key {
            transcript::TurnKey::Live(turn) => format!("turn_{turn}"),
            transcript::TurnKey::Named(id) => id.clone(),
        };
        self.pending_action = Some(ComponentAction::PinTurn {
            turn_id,
            pinned: true,
        });
    }

    /// The scrollbar, when the transcript is taller than its window.
    fn render_scrollbar(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        if self.layout.max_scroll == 0 {
            return;
        }
        let mut state = ScrollbarState::new(self.layout.max_scroll).position(
            self.layout
                .max_scroll
                .saturating_sub(self.scroll_from_bottom),
        );
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(palette::border_style(model.preferences.theme)),
            area,
            &mut state,
        );
    }

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

    fn turn_at(&self, column: u16, row: u16) -> Option<transcript::TurnKey> {
        let absolute = self.absolute_row(column, row)?;
        self.layout
            .separators
            .iter()
            .find(|(at, _)| *at == absolute)
            .map(|(_, key)| key.clone())
    }

    fn thought_at(&self, column: u16, row: u16) -> Option<transcript::ThoughtKey> {
        let absolute = self.absolute_row(column, row)?;
        self.layout
            .thoughts
            .iter()
            .find(|(at, _)| *at == absolute)
            .map(|(_, key)| key.clone())
    }

    fn absolute_row(&self, column: u16, row: u16) -> Option<usize> {
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
        Some(usize::from(row - y) + scroll)
    }

    fn toggle_thought_scrollable(&mut self, model: &UiModel, key: &transcript::ThoughtKey) {
        let before = self.layout_now(model);
        let expanded = self.thought_expanded(model, key);
        match self.thoughts.iter_mut().find(|fold| fold.key == *key) {
            Some(fold) => fold.expanded = !expanded,
            None => self.thoughts.push(transcript::ThoughtFold {
                key: key.clone(),
                expanded: !expanded,
            }),
        }
        let after = self.layout_now(model);
        let after_rows = Self::turn_rows(&after, &key.turn);
        let before_rows = Self::turn_rows(&before, &key.turn);
        let delta = after_rows.cast_signed() - before_rows.cast_signed();
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add_signed(delta);
    }

    fn thought_expanded(&self, model: &UiModel, key: &transcript::ThoughtKey) -> bool {
        self.thoughts.iter().find(|fold| fold.key == *key).map_or(
            model.preferences.thinking == crate::ui_model::ThinkingVisibility::Expanded,
            |fold| fold.expanded,
        )
    }

    fn layout_now(&self, model: &UiModel) -> Vec<transcript::TurnLayout> {
        transcript::turn_layout(
            &model.transcript,
            self.layout.width,
            self.transcript_options(model, self.layout.width),
        )
    }

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
            clock: &model.clock,
            theme: model.preferences.theme,
            developer_mode: model.preferences.developer_mode,
            shell_output_expanded: model.shell_output_expanded,
            thinking_phase: self.animation_phase,
            thinking_streaming: matches!(model.turn_state, crate::ui_model::TurnState::Thinking),
            folded: &self.folded,
            thoughts: &self.thoughts,
        }
    }

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

    fn toggle_fold(&mut self, model: &UiModel) {
        let before = self.layout_now(model);
        let Some(current) = self.current_turn(&before) else {
            return;
        };
        self.fold_turn_scrollable(model, &current);
    }

    fn fold_turn_scrollable(&mut self, model: &UiModel, key: &transcript::TurnKey) {
        let before = self.layout_now(model);
        if let Some(position) = self.folded.iter().position(|turn| turn == key) {
            self.folded.remove(position);
        } else {
            self.folded.push(key.clone());
        }
        let after = self.layout_now(model);
        let lost = Self::turn_rows(&before, key).saturating_sub(Self::turn_rows(&after, key));
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(lost);
    }

    fn turn_rows(layout: &[transcript::TurnLayout], key: &transcript::TurnKey) -> usize {
        let Some(index) = layout.iter().position(|turn| turn.key == *key) else {
            return 0;
        };
        let start = layout[index].start;
        let end = layout.get(index + 1).map_or(start, |next| next.start);
        end.saturating_sub(start)
    }

    fn jump_to_turn(&mut self, model: &UiModel, key: &transcript::TurnKey) {
        self.folded.retain(|folded| folded != key);
        let layout = self.layout_now(model);
        let Some(turn) = layout.iter().find(|turn| turn.key == *key) else {
            return;
        };
        self.scroll_from_bottom = self.layout.max_scroll.saturating_sub(turn.start);
    }

    fn jump_turn(&mut self, model: &UiModel, step: isize) {
        let layout = self.layout_now(model);
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
        let key = turn.key.clone();
        self.jump_to_turn(model, &key);
    }

    fn handle_transcript_key(&mut self, key: KeyEvent, model: &UiModel) {
        if keys::is_cancel_turn(key) {
            if model.turn_state.is_busy() {
                self.pending_action = Some(ComponentAction::CancelTurn);
            }
            return;
        }
        match key.code {
            keys::COMPOSER if model.active_child.is_none() => {
                self.composer.focus = SessionFocus::Composer;
            }
            KeyCode::Char('z') => self.toggle_fold(model),
            KeyCode::Char('[') => self.jump_turn(model, -1),
            KeyCode::Char(']') => self.jump_turn(model, 1),
            keys::TOP => self.scroll_from_bottom = self.layout.max_scroll,
            keys::BOTTOM | KeyCode::End => self.scroll_from_bottom = 0,
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(1);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                self.scroll_from_bottom = self
                    .scroll_from_bottom
                    .saturating_add(1)
                    .min(self.layout.max_scroll);
            }
            KeyCode::PageDown => self.scroll_transcript(-10),
            KeyCode::PageUp => self.scroll_transcript(10),
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

/// The modal that asks whether to continue past the budget.
fn render_budget_approval(frame: &mut Frame<'_>, model: &UiModel) {
    use ratatui::widgets::Clear;

    let Some(approval) = model.budget_approval else {
        return;
    };
    let theme = model.preferences.theme;
    crate::components::paint_scrim(frame, theme);
    let area = crate::components::rect::centered_rect(
        60.min(frame.area().width),
        9.min(frame.area().height),
        frame.area(),
    );
    frame.render_widget(Clear, area);
    let lines = vec![
        Line::from(Span::styled(
            format!(
                "The session reached ${:.2} of its ${:.2} budget.",
                approval.spent, approval.limit
            ),
            Style::default().fg(palette::text(theme)),
        )),
        Line::default(),
        Line::from(Span::styled(
            "a approve one continuation, d decline",
            palette::muted_style(theme),
        )),
    ];
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(palette::modal_style(theme))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(palette::border_style(theme))
                    .title(" budget reached ")
                    .padding(spacing::modal())
                    .style(palette::modal_style(theme)),
            ),
        area,
    );
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
