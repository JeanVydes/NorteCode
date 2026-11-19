use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Frame, layout::Rect};
use std::collections::BTreeSet;

use crate::{
    components::{
        Component, ComponentAction, ComponentId, ComponentLifecycle, LifecycleEvent, LifecyclePhase,
    },
    keys,
    ui_model::UiModel,
};

#[path = "plan_review/render.rs"]
mod render;

const PLAN_REVIEW_PREFIX: &str = "NORTE_CODE_PLAN_REVIEW\n";

#[derive(Default)]
pub(crate) struct PlanReviewView {
    lifecycle: ComponentLifecycle,
    session: Option<norte_code_wire::SessionKey>,
    question: Option<String>,
    content: String,
    options: Vec<String>,
    selected_option: usize,
    selected_options: BTreeSet<usize>,
    multiple: bool,
    prompt_kind: PromptKind,
    input_mode: InputMode,
    response: String,
    /// How many rows the document is scrolled down.
    scroll: u16,
    /// Where the review last drew, so a click maps to a button.
    last_area: Rect,
    pending_action: Option<ComponentAction>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PromptKind {
    #[default]
    Plan,
    Question,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum InputMode {
    #[default]
    Respond,
    Feedback,
    Answer,
}

impl PlanReviewView {
    pub(crate) fn begin(
        &mut self,
        session: norte_code_wire::SessionKey,
        question: &str,
        prompt: &str,
        options: &[String],
        multiple: bool,
    ) -> Result<(), crate::components::LifecycleTransitionError> {
        if self.question.as_deref() == Some(question) {
            return Ok(());
        }
        let (kind, content) = prompt.strip_prefix(PLAN_REVIEW_PREFIX).map_or_else(
            || (PromptKind::Question, prompt),
            |content| (PromptKind::Plan, content),
        );
        if self.lifecycle.phase() != LifecyclePhase::Dormant {
            self.lifecycle.transition(LifecycleEvent::Unmount)?;
        }
        self.lifecycle.transition(LifecycleEvent::Mount)?;
        self.lifecycle.transition(LifecycleEvent::LoadStarted)?;
        self.lifecycle.transition(LifecycleEvent::Loaded)?;
        self.session = Some(session);
        self.question = Some(question.to_owned());
        content.clone_into(&mut self.content);
        self.options = options.to_vec();
        self.selected_option = 0;
        self.selected_options.clear();
        self.multiple = multiple;
        self.prompt_kind = kind;
        self.input_mode = InputMode::Respond;
        self.response.clear();
        self.scroll = 0;
        Ok(())
    }

    pub(crate) fn is_open(&self) -> bool {
        self.question.is_some()
    }

    pub(crate) fn close(&mut self) -> Result<(), crate::components::LifecycleTransitionError> {
        if self.lifecycle.phase() != LifecyclePhase::Dormant {
            self.lifecycle.transition(LifecycleEvent::Unmount)?;
        }
        self.session = None;
        self.question = None;
        self.content.clear();
        self.options.clear();
        self.selected_option = 0;
        self.selected_options.clear();
        self.multiple = false;
        self.prompt_kind = PromptKind::Plan;
        self.input_mode = InputMode::Respond;
        self.response.clear();
        self.scroll = 0;
        Ok(())
    }

    fn answer(&mut self, answer: String) {
        if let (Some(session), Some(question)) = (self.session.clone(), self.question.clone()) {
            self.pending_action = Some(ComponentAction::AnswerQuestion {
                session,
                question,
                answer,
            });
        }
    }
}

impl Component for PlanReviewView {
    fn id(&self) -> ComponentId {
        ComponentId::PlanReview
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn captures_global_keys(&self) -> bool {
        self.is_open()
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, model: &UiModel) {
        if !self.is_open() || self.input_mode != InputMode::Respond {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_document(-3),
            MouseEventKind::ScrollDown => self.scroll_document(3),
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(index) = self.button_at(mouse.column, mouse.row, self.last_area) else {
                    return;
                };
                self.press_button(index);
            }
            _ => {}
        }
        let _ = model;
    }

    fn handle_key(&mut self, key: KeyEvent, _model: &UiModel) {
        if !self.is_open() {
            return;
        }
        if self.input_mode != InputMode::Respond {
            match key.code {
                keys::CANCEL => self.input_mode = InputMode::Respond,
                keys::SUBMIT if !self.response.trim().is_empty() => {
                    let response = std::mem::take(&mut self.response);
                    self.answer(response);
                }
                keys::ERASE => {
                    self.response.pop();
                }
                KeyCode::Char(character) => self.response.push(character),
                _ => {}
            }
            return;
        }
        match key.code {
            keys::NAV_DOWN => {
                self.scroll_document(1);
                return;
            }
            keys::NAV_UP => {
                self.scroll_document(-1);
                return;
            }
            KeyCode::PageDown => {
                self.scroll_document(10);
                return;
            }
            KeyCode::PageUp => {
                self.scroll_document(-10);
                return;
            }
            _ => {}
        }
        match (self.prompt_kind, key.code) {
            (PromptKind::Plan, KeyCode::Char('a') | keys::SUBMIT) => {
                self.answer("Approve".to_owned());
            }
            (PromptKind::Plan, KeyCode::Char('d')) => self.answer("Reject".to_owned()),
            (PromptKind::Plan, KeyCode::Char('f')) => self.input_mode = InputMode::Feedback,
            (PromptKind::Question, keys::NAV_DOWN | keys::NAV_DOWN_ARROW) => {
                self.selected_option = self
                    .selected_option
                    .saturating_add(1)
                    .min(self.options.len().saturating_sub(1));
            }
            (PromptKind::Question, keys::NAV_UP | keys::NAV_UP_ARROW) => {
                self.selected_option = self.selected_option.saturating_sub(1);
            }
            (PromptKind::Question, KeyCode::Char(' ')) if self.multiple => {
                if self.selected_options.contains(&self.selected_option) {
                    self.selected_options.remove(&self.selected_option);
                } else if self.selected_option < self.options.len() {
                    self.selected_options.insert(self.selected_option);
                }
            }
            (PromptKind::Question, keys::SUBMIT) if !self.options.is_empty() => {
                let answer = if self.multiple && !self.selected_options.is_empty() {
                    self.selected_options
                        .iter()
                        .filter_map(|index| self.options.get(*index))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                } else if let Some(option) = self.options.get(self.selected_option) {
                    option.clone()
                } else {
                    return;
                };
                self.answer(answer);
            }
            (PromptKind::Question, KeyCode::Char('f') | keys::SUBMIT) => {
                self.input_mode = InputMode::Answer;
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        self.last_area = area;
        self.draw(frame, model);
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}

impl PlanReviewView {
    /// Scrolls the document, never past its ends.
    fn scroll_document(&mut self, rows: i16) {
        let lines = u16::try_from(self.content.lines().count()).unwrap_or(u16::MAX);
        let next = i32::from(self.scroll) + i32::from(rows);
        self.scroll = u16::try_from(next.clamp(0, i32::from(lines))).unwrap_or(0);
    }

    /// Runs the button at an index of the footer.
    fn press_button(&mut self, index: usize) {
        match (self.prompt_kind, index) {
            (PromptKind::Plan, 0) => self.answer("Approve".to_owned()),
            (PromptKind::Plan, 1) => self.answer("Reject".to_owned()),
            (PromptKind::Plan, _) => self.input_mode = InputMode::Feedback,
            (PromptKind::Question, 0) => self.input_mode = InputMode::Answer,
            (PromptKind::Question, _) => self.submit_question(),
        }
    }

    /// Confirms the option the reader is on.
    fn submit_question(&mut self) {
        if let Some(option) = self.options.get(self.selected_option) {
            self.answer(option.clone());
        } else {
            self.input_mode = InputMode::Answer;
        }
    }

    /// The action the reader asked for, while the review is open.
    pub(crate) fn action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }

    /// Sends one key to the review.
    pub(crate) fn key(&mut self, key: KeyEvent, model: &UiModel) {
        <Self as Component>::handle_key(self, key, model);
    }
}

#[cfg(test)]
#[path = "plan_review_tests.rs"]
mod tests;
