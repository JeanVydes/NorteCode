use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::Line,
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use std::collections::BTreeSet;

use crate::{
    components::{
        Component, ComponentAction, ComponentId, ComponentLifecycle, LifecycleEvent,
        LifecyclePhase, palette,
    },
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

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
        if !self.is_open() {
            return;
        }
        let dialog = centered_rect(area);
        frame.render_widget(Clear, dialog);
        let modal_style = palette::modal_style(model.preferences.theme);
        frame.render_widget(Block::default().style(modal_style), dialog);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(6),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(dialog);
        let is_plan = self.prompt_kind == PromptKind::Plan;
        let content_title = if is_plan {
            TextBuilder::plan_review_title()
        } else {
            TextBuilder::question_title()
        };
        frame.render_widget(
            Paragraph::new(self.content.as_str())
                .block(
                    Block::default()
                        .title(content_title)
                        .style(modal_style)
                        .title_style(Style::default().fg(palette::accent(model.preferences.theme))),
                )
                .wrap(Wrap { trim: false })
                .style(modal_style),
            sections[0],
        );
        if is_plan || self.options.is_empty() {
            frame.render_widget(
                Paragraph::new(if is_plan {
                    TextBuilder::plan_review_feedback_hint()
                } else {
                    TextBuilder::question_answer_hint()
                })
                .style(modal_style),
                sections[1],
            );
        } else {
            let items = self.options.iter().enumerate().map(|(index, option)| {
                let marker = if self.multiple && self.selected_options.contains(&index) {
                    "[x]"
                } else if self.selected_option == index {
                    ">"
                } else {
                    " "
                };
                ListItem::new(Line::from(format!("{marker} {option}")))
            });
            let mut list_state = ListState::default();
            list_state.select(Some(self.selected_option));
            frame.render_stateful_widget(
                List::new(items).style(modal_style).highlight_style(
                    palette::selected_style(model.preferences.theme)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ),
                sections[1],
                &mut list_state,
            );
        }
        frame.render_widget(
            Paragraph::new(match self.input_mode {
                InputMode::Feedback | InputMode::Answer => self.response.as_str(),
                InputMode::Respond => "",
            })
            .style(modal_style)
            .block(Block::default().title(if is_plan {
                TextBuilder::plan_review_feedback_title()
            } else {
                TextBuilder::question_answer_hint()
            })),
            sections[2],
        );
        frame.render_widget(
            Paragraph::new(match (self.prompt_kind, self.input_mode, self.multiple) {
                (_, InputMode::Feedback | InputMode::Answer, _) => "Enter submit · Esc cancel",
                (PromptKind::Plan, InputMode::Respond, _) => {
                    TextBuilder::plan_review_controls(false)
                }
                (PromptKind::Question, InputMode::Respond, true) => {
                    "j/k move · Space select · Enter submit · f answer"
                }
                (PromptKind::Question, InputMode::Respond, false) => {
                    "j/k move · Enter select · f answer"
                }
            })
            .style(modal_style),
            sections[3],
        );
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}

fn centered_rect(area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Percentage(70),
            Constraint::Percentage(15),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(10),
            Constraint::Percentage(80),
            Constraint::Percentage(10),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
#[path = "plan_review_tests.rs"]
mod tests;
