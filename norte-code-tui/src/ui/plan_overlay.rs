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
    response: String,
    #[serde(skip)]
    pending_action: Option<PlanReviewAction>,
    #[serde(skip)]
    buttons: Vec<(Rect, usize)>,
}

impl PlanOverlay {
    /// Show a plan from the beginning, as a read-only preview.
    pub(crate) fn open(&mut self, plan: norte_code_wire::PlanSummary) {
        self.plan = Some(plan);
        self.reset_interaction();
    }

    /// Open the review asked for while the plan was created.
    pub(crate) fn begin_review(&mut self, question: String, content: String) {
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
        self.response.clear();
        self.pending_action = None;
        self.buttons.clear();
    }

    /// Whether a plan is showing.
    pub(crate) fn is_open(&self) -> bool {
        self.plan.is_some()
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
        self.plan = None;
        self.doc = Markdown::default();
        self.doc_theme = None;
        self.scroll = 0;
        self.body = Rect::new(0, 0, 0, 0);
        self.question = None;
        self.mode = Mode::Respond;
        self.response.clear();
        self.pending_action = None;
        self.buttons.clear();
    }

    fn answer(&mut self, action: PlanReviewAction) {
        self.pending_action = Some(action);
        self.mode = Mode::Respond;
        self.response.clear();
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
            match index {
                0 => self.answer(PlanReviewAction::Accept),
                1 => self.answer(PlanReviewAction::Decline),
                _ => self.mode = Mode::Feedback,
            }
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
                self.close();
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
            KeyCode::Char('a') | KeyCode::Enter if self.question.is_some() => {
                self.answer(PlanReviewAction::Accept);
                Outcome::Consumed
            }
            KeyCode::Char('d') if self.question.is_some() => {
                self.answer(PlanReviewAction::Decline);
                Outcome::Consumed
            }
            KeyCode::Char('f') if self.question.is_some() => {
                self.mode = Mode::Feedback;
                Outcome::Consumed
            }
            _ => Outcome::Ignored,
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
