use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span, Text},
    widgets::{Block, Padding, Paragraph},
};

use crate::{
    components::{
        Component, ComponentAction, ComponentEvent, ComponentId, ComponentLifecycle, Preference,
        ViewAction, palette,
    },
    keys,
    tools::DiffMode,
    ui_model::UiModel,
};

mod composer;
mod jobs;
mod markdown;
mod picker;
mod todos;
mod transcript;
mod turn_picker;

use composer::{Composer, SessionFocus};
use jobs::JobsPanel;
use picker::PickerPanel;
use turn_picker::{TurnPickerMode, TurnPickerPanel};

/// What the reader is picking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Picker {
    Turn(TurnPickerMode),
    Job,
}

#[derive(Default)]
pub(in crate::components) struct SessionView {
    lifecycle: ComponentLifecycle,
    composer: Composer,
    scroll_from_bottom: usize,
    max_scroll: usize,
    pending_action: Option<ComponentAction>,
    animation_phase: u16,
    picker: Option<Picker>,
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
            || self.picker.is_some()
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        if self.handle_budget_approval(key, model) || self.handle_budget_entry(key, model) {
            return;
        }
        if self.handle_picker(key, model) {
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
            ComponentEvent::SessionOpened => self.composer.reset(),
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        self.animation_phase = (self.animation_phase + 1) % area.width.max(1);
        let content = session_regions(
            area,
            self.picker_lines(model),
            JobsPanel::height(&model.jobs),
            todos::TodosPanel::height(&model.todos),
            self.composer.height(),
        );
        self.max_scroll = transcript::TranscriptRenderer::render(
            frame,
            content[0],
            transcript::TranscriptContext {
                items: &model.transcript,
                session: active_session_label(model),
                turn_state: model.turn_state,
                model: model.session_model.as_ref(),
                diff_mode: model.cli.diff_style,
                scroll_from_bottom: self.scroll_from_bottom,
                focused: self.composer.focus == SessionFocus::Transcript,
                thinking_visibility: model.preferences.thinking,
                show_tool_output: model.preferences.show_tool_output.is_on(),
                show_timestamps: model.preferences.show_timestamps.is_on(),
                theme: model.preferences.theme,
                attachment_state: model.attachment_state,
                thinking_phase: self.animation_phase,
            },
        );
        let input = self.composer.placeholder(model);
        let hint = Composer::hint(model);
        self.render_picker(frame, content[1], model);
        JobsPanel::render(frame, content[2], &model.jobs, model.preferences.theme);
        todos::TodosPanel::render(frame, content[3], &model.todos, model.preferences.theme);
        let mut lines = vec![Line::from(input)];
        lines.extend(self.composer.chip_lines());
        lines.push(Line::from(Span::styled(hint, Composer::hint_style())));
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(palette::panel_style(model.preferences.theme))
                .block(
                    Block::default()
                        .padding(Padding::horizontal(1))
                        .style(palette::panel_style(model.preferences.theme)),
                ),
            content[4],
        );
    }
}

impl SessionView {
    fn picker_lines(&self, model: &UiModel) -> u16 {
        match self.picker {
            Some(Picker::Turn(_)) => {
                u16::try_from(model.turn_points.len().min(9) + 1).unwrap_or(u16::MAX)
            }
            Some(Picker::Job) => {
                u16::try_from(JobsPanel::running(&model.jobs).len() + 1).unwrap_or(u16::MAX)
            }
            None => 0,
        }
    }

    fn render_picker(&self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let theme = model.preferences.theme;
        match self.picker {
            Some(Picker::Turn(mode)) => PickerPanel::render(
                frame,
                area,
                mode.title(),
                TurnPickerPanel::lines(&model.turn_points, &model.pinned_turns, theme),
                theme,
            ),
            Some(Picker::Job) => PickerPanel::render(
                frame,
                area,
                "stop a job",
                JobsPanel::picker_lines(&model.jobs, theme),
                theme,
            ),
            None => {}
        }
    }

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

    fn handle_picker(&mut self, key: KeyEvent, model: &UiModel) -> bool {
        let Some(picker) = self.picker else {
            return false;
        };
        if key.code == keys::CANCEL {
            self.picker = None;
            return true;
        }
        let KeyCode::Char(character) = key.code else {
            return true;
        };
        let Some(choice) = character.to_digit(10) else {
            return true;
        };
        let choice = usize::try_from(choice).unwrap_or(0);
        self.picker = None;
        self.pending_action = match picker {
            Picker::Turn(mode) => Self::pick_turn(mode, choice, model),
            Picker::Job => Self::pick_job(choice, model),
        };
        true
    }

    fn pick_turn(mode: TurnPickerMode, choice: usize, model: &UiModel) -> Option<ComponentAction> {
        let points = TurnPickerPanel::offered(&model.turn_points);
        let index = PickerPanel::pick(points.len(), choice)?;
        let point = points[index].clone();
        let pinned = !model.pinned_turns.contains(&point.turn_id);
        Some(match mode {
            TurnPickerMode::Rewind => ComponentAction::Rewind {
                turn_id: point.turn_id,
            },
            TurnPickerMode::Pin => ComponentAction::PinTurn {
                turn_id: point.turn_id,
                pinned,
            },
        })
    }

    fn pick_job(choice: usize, model: &UiModel) -> Option<ComponentAction> {
        let jobs = JobsPanel::running(&model.jobs);
        let index = PickerPanel::pick(jobs.len(), choice)?;
        Some(ComponentAction::StopJob {
            job_id: jobs[index].id.clone(),
        })
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
                self.pending_action = Some(ComponentAction::SetQueueMode(next_queue_mode(
                    model.queue_mode,
                )));
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
            KeyCode::Char(keys::THINKING_TOGGLE_CHAR) => {
                self.pending_action = Some(ComponentAction::View(ViewAction::TogglePreference(
                    Preference::Thinking,
                )));
            }
            KeyCode::Char(keys::CANCEL_TURN_CHAR) => {
                self.pending_action = Some(ComponentAction::CancelTurn);
            }
            KeyCode::Char(keys::REASONING_CHAR) if model.active_child.is_none() => {
                self.pending_action = Some(ComponentAction::SetReasoning(model.reasoning.next()));
            }
            KeyCode::Char(keys::REWIND_CHAR) if model.active_child.is_none() => {
                self.picker = Some(Picker::Turn(TurnPickerMode::Rewind));
            }
            KeyCode::Char(keys::PIN_CHAR) if model.active_child.is_none() => {
                self.picker = Some(Picker::Turn(TurnPickerMode::Pin));
            }
            KeyCode::Char(keys::STOP_JOB_CHAR) if model.active_child.is_none() => {
                self.picker = Some(Picker::Job);
            }
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(1);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                self.scroll_from_bottom = self
                    .scroll_from_bottom
                    .saturating_add(1)
                    .min(self.max_scroll);
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

fn session_regions(
    area: Rect,
    picker_lines: u16,
    job_lines: u16,
    todo_lines: u16,
    composer_lines: u16,
) -> [Rect; 5] {
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(picker_lines),
            Constraint::Length(job_lines),
            Constraint::Length(todo_lines),
            Constraint::Length(composer_lines),
        ])
        .split(area);
    [regions[0], regions[1], regions[2], regions[3], regions[4]]
}

const fn next_queue_mode(mode: norte_code_wire::QueueMode) -> norte_code_wire::QueueMode {
    match mode {
        norte_code_wire::QueueMode::Reject => norte_code_wire::QueueMode::Queue,
        norte_code_wire::QueueMode::Queue => norte_code_wire::QueueMode::Interrupt,
        norte_code_wire::QueueMode::Interrupt => norte_code_wire::QueueMode::Reject,
    }
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
