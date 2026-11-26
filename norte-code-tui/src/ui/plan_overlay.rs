//! The plan review overlay: a document viewer with review controls.

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use serde::Serialize;
use terminalui::{Context, InputEvent, Outcome, Theme, View};

use super::markdown::{Markdown, open_link};

#[path = "plan_overlay_action.rs"]
mod action;
#[path = "plan_overlay_document.rs"]
mod document;
#[path = "plan_overlay_render.rs"]
mod render;

pub(crate) use action::{PLAN_REVIEW_PREFIX, PlanReviewAction};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mode {
    #[default]
    Respond,
    Feedback,
}

/// The plan currently shown, if any.
#[derive(Serialize, Default)]
pub(crate) struct PlanOverlay {
    #[serde(skip)]
    elapsed: std::time::Duration,
    #[serde(skip)]
    plans: Option<Vec<norte_code_wire::PlanSummary>>,
    #[serde(skip)]
    list_selected: usize,
    #[serde(skip)]
    list_offset: usize,
    #[serde(skip)]
    list_body: Rect,
    #[serde(skip)]
    plan: Option<norte_code_wire::PlanSummary>,
    #[serde(skip)]
    doc: Markdown,
    #[serde(skip)]
    doc_theme: Option<Theme>,
    #[serde(skip)]
    width: u16,
    #[serde(skip)]
    scroll: u16,
    #[serde(skip)]
    area: Rect,
    #[serde(skip)]
    body: Rect,
    #[serde(skip)]
    question: Option<String>,
    #[serde(skip)]
    mode: Mode,
    #[serde(skip)]
    selected: usize,
    #[serde(skip)]
    response: String,
    #[serde(skip)]
    pending_action: Option<PlanReviewAction>,
    #[serde(skip)]
    buttons: Vec<(Rect, usize)>,
}

impl PlanOverlay {
    pub(crate) fn open_list(&mut self, plans: Vec<norte_code_wire::PlanSummary>) {
        self.close();
        self.plans = Some(plans);
        self.list_selected = 0;
        self.list_offset = 0;
    }

    pub(crate) fn sync_plans(&mut self, plans: &[norte_code_wire::PlanSummary]) {
        if let Some(current) = self.plans.as_mut()
            && current != plans
        {
            current.clone_from(&plans.to_vec());
            self.list_selected = self.list_selected.min(current.len().saturating_sub(1));
        }
    }

    /// Show a plan from the beginning, as a read-only preview.
    pub(crate) fn open(&mut self, plan: norte_code_wire::PlanSummary) {
        self.plan = Some(plan);
        self.reset_interaction();
    }

    /// Open the review asked for while the plan was created.
    pub(crate) fn begin_review(&mut self, question: String, content: String) {
        self.plans = None;
        self.plan = Some(norte_code_wire::PlanSummary {
            id: question.clone(),
            title: Some(crate::ui::copy::PLAN_REVIEW.to_owned()),
            content,
            ..Default::default()
        });
        self.reset_interaction();
        self.question = Some(question);
    }

    fn reset_interaction(&mut self) {
        self.doc = Markdown::default();
        self.doc_theme = None;
        self.width = 0;
        self.scroll = 0;
        self.area = Rect::new(0, 0, 0, 0);
        self.body = Rect::new(0, 0, 0, 0);
        self.question = None;
        self.mode = Mode::Respond;
        self.selected = 0;
        self.response.clear();
        self.pending_action = None;
        self.buttons.clear();
    }

    /// Whether a plan is showing.
    pub(crate) fn is_open(&self) -> bool {
        self.plan.is_some() || self.plans.is_some()
    }

    /// The plan currently showing.
    #[cfg(test)]
    pub(crate) fn plan(&self) -> Option<&norte_code_wire::PlanSummary> {
        self.plan.as_ref()
    }

    /// Take the review answer and close the overlay.
    pub(crate) fn take_action(&mut self) -> Option<(String, PlanReviewAction)> {
        let action = self.pending_action.take()?;
        let question = self.question.clone()?;
        self.close();
        Some((question, action))
    }

    /// Close the overlay.
    pub(crate) fn close(&mut self) {
        self.plans = None;
        self.plan = None;
        self.doc = Markdown::default();
        self.doc_theme = None;
        self.scroll = 0;
        self.body = Rect::new(0, 0, 0, 0);
        self.question = None;
        self.mode = Mode::Respond;
        self.selected = 0;
        self.response.clear();
        self.pending_action = None;
        self.buttons.clear();
    }

    fn answer(&mut self, action: PlanReviewAction) {
        self.pending_action = Some(action);
        self.mode = Mode::Respond;
        self.response.clear();
    }

    fn activate(&mut self, index: usize) {
        self.selected = index;
        match index {
            0 => self.answer(PlanReviewAction::Accept),
            1 => self.answer(PlanReviewAction::Decline),
            _ => self.mode = Mode::Feedback,
        }
    }

    fn submit_feedback(&mut self) {
        let Some(feedback) = crate::review_feedback::ReviewFeedback::new(self.response.clone())
        else {
            return;
        };
        self.answer(PlanReviewAction::Feedback(feedback));
    }

    fn button_at(&self, column: u16, row: u16) -> Option<usize> {
        self.buttons
            .iter()
            .find(|(rect, _)| rect.y == row && column >= rect.x && column < rect.x + rect.width)
            .map(|(_, index)| *index)
    }

    fn scroll_document(&mut self, rows: i16) {
        let next = i32::from(self.scroll) + i32::from(rows);
        self.scroll = u16::try_from(next.max(0)).unwrap_or(u16::MAX);
    }

    fn scroll_to(&mut self, top: bool, lines: usize) {
        let max = lines.saturating_sub(usize::from(self.body.height));
        let target = if top { 0 } else { max };
        self.scroll = u16::try_from(target).unwrap_or(u16::MAX);
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Outcome {
        if self.plan.is_none() && self.plans.is_some() {
            return match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.list_move(-3);
                    Outcome::Consumed
                }
                MouseEventKind::ScrollDown => {
                    self.list_move(3);
                    Outcome::Consumed
                }
                MouseEventKind::Down(MouseButton::Left)
                    if self
                        .list_body
                        .contains(Position::new(mouse.column, mouse.row)) =>
                {
                    let index = self.list_offset + usize::from(mouse.row - self.list_body.y);
                    if self.plans.as_ref().is_some_and(|plans| index < plans.len()) {
                        self.list_selected = index;
                        self.open_selected();
                    }
                    Outcome::Consumed
                }
                _ => Outcome::Ignored,
            };
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_document(-3);
                Outcome::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.scroll_document(3);
                Outcome::Consumed
            }
            MouseEventKind::Down(MouseButton::Left) if self.mode == Mode::Respond => {
                self.handle_click(mouse)
            }
            _ => Outcome::Ignored,
        }
    }

    fn handle_click(&mut self, mouse: MouseEvent) -> Outcome {
        if let Some(index) = self.button_at(mouse.column, mouse.row)
            && self.question.is_some()
        {
            self.activate(index);
            return Outcome::Consumed;
        }
        let row = usize::from(mouse.row.saturating_sub(self.body.y));
        let column = usize::from(mouse.column.saturating_sub(self.body.x + 6));
        let doc_row = row.saturating_add(usize::from(self.scroll));
        if let Some(url) = self.doc.link_at(doc_row, column) {
            open_link(url);
            Outcome::Consumed
        } else {
            Outcome::Ignored
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) -> Outcome {
        if self.plan.is_none() && self.plans.is_some() {
            return match key.code {
                KeyCode::Esc => {
                    self.close();
                    Outcome::Consumed
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.list_move(-1);
                    Outcome::Consumed
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.list_move(1);
                    Outcome::Consumed
                }
                KeyCode::Enter => {
                    self.open_selected();
                    Outcome::Consumed
                }
                _ => Outcome::Ignored,
            };
        }
        if self.mode == Mode::Feedback {
            self.handle_feedback_key(key)
        } else {
            self.handle_review_key(key)
        }
    }

    fn handle_feedback_key(&mut self, key: &KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Respond,
            KeyCode::Backspace => {
                self.response.pop();
            }
            KeyCode::Enter => self.submit_feedback(),
            KeyCode::Char(character) => self.response.push(character),
            _ => return Outcome::Ignored,
        }
        Outcome::Consumed
    }

    fn handle_review_key(&mut self, key: &KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => {
                if self.plans.is_some() && self.question.is_none() {
                    self.plan = None;
                } else {
                    self.close();
                }
                Outcome::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_document(-1);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_document(1);
                Outcome::Consumed
            }
            KeyCode::PageUp | KeyCode::Char('g') => {
                self.scroll_to(true, self.doc.lines.len());
                Outcome::Consumed
            }
            KeyCode::PageDown | KeyCode::Char('G') => {
                self.scroll_to(false, self.doc.lines.len());
                Outcome::Consumed
            }
            KeyCode::Left if self.question.is_some() => {
                self.selected = self.selected.saturating_sub(1);
                Outcome::Consumed
            }
            KeyCode::Right if self.question.is_some() => {
                self.selected = (self.selected + 1).min(2);
                Outcome::Consumed
            }
            KeyCode::Enter if self.question.is_some() => {
                self.activate(self.selected);
                Outcome::Consumed
            }
            KeyCode::Char('a') if self.question.is_some() => {
                self.activate(0);
                Outcome::Consumed
            }
            KeyCode::Char('d') if self.question.is_some() => {
                self.activate(1);
                Outcome::Consumed
            }
            KeyCode::Char('f') if self.question.is_some() => {
                self.activate(2);
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
        }
    }

    fn list_move(&mut self, delta: isize) {
        let len = self.plans.as_ref().map_or(0, Vec::len);
        self.list_selected = self
            .list_selected
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    fn open_selected(&mut self) {
        if let Some(plan) = self
            .plans
            .as_ref()
            .and_then(|plans| plans.get(self.list_selected))
            .cloned()
        {
            self.open(plan);
        }
    }
}

impl View for PlanOverlay {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn kind(&self) -> &'static str {
        "plan_overlay"
    }
    fn focusable(&self) -> bool {
        true
    }

    fn tick(&mut self, delta: std::time::Duration, _cx: &mut Context) {
        if self.is_open() {
            self.elapsed = self.elapsed.saturating_add(delta);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _cx: &mut Context, theme: &Theme) {
        self.paint(area, buf, theme);
    }

    fn handle_event(&mut self, event: &InputEvent, _cx: &mut Context) -> Outcome {
        if !self.is_open() {
            return Outcome::Ignored;
        }
        match event {
            InputEvent::Paste(text) if self.mode == Mode::Feedback => {
                self.response.push_str(text);
                Outcome::Consumed
            }
            InputEvent::Mouse(mouse)
                if self.area.contains(Position::new(mouse.column, mouse.row)) =>
            {
                self.handle_mouse(*mouse)
            }
            InputEvent::Key(key) => self.handle_key(key),
            _ => Outcome::Ignored,
        }
    }
}

#[cfg(test)]
#[path = "plan_overlay_tests.rs"]
mod tests;
