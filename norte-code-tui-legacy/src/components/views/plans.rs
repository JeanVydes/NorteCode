//! The session's plans, listed beside the one being read.
//!
//! A plan is markdown, so the detail pane renders it the way the transcript
//! and the review do: headings, lists, code and tables drawn, not their
//! markers. The list stays on the left so switching plans costs one key.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use crate::{
    components::{Component, ComponentId, ComponentLifecycle, markdown, palette, panel, spacing},
    keys,
    text_builder::TextBuilder,
    ui_model::{ThemeMode, UiModel},
};

#[derive(Default)]
pub(in crate::components) struct PlansView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    collapsed: bool,
    detail_scroll: u16,
}

impl Component for PlansView {
    fn id(&self) -> ComponentId {
        ComponentId::Plans
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        match key.code {
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                panel::move_selection(&mut self.selected, model.plans.len(), true);
                self.detail_scroll = 0;
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                panel::move_selection(&mut self.selected, model.plans.len(), false);
                self.detail_scroll = 0;
            }
            keys::SUBMIT => self.collapsed = !self.collapsed,
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(8),
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(8),
            keys::TOP => self.detail_scroll = 0,
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let entries: Vec<Line<'static>> = model.plans.iter().map(TextBuilder::plan_row).collect();
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(area);
        panel::render_list(
            frame,
            columns[0],
            entries.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Plans,
                phase: self.lifecycle.phase(),
                selected: Some(self.selected),
                focused: true,
                empty_message: TextBuilder::empty_message(ComponentId::Plans),
            },
        );
        self.render_detail(frame, columns[1], model);
    }

    fn take_action(&mut self) -> Option<crate::components::ComponentAction> {
        None
    }
}

impl PlansView {
    /// The plan itself, as rendered markdown.
    fn render_detail(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let theme = model.preferences.theme;
        let block = Block::default()
            .title(TextBuilder::details_title())
            .padding(spacing::panel())
            .style(palette::panel_style(theme));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(plan) = model.plans.get(self.selected) else {
            frame.render_widget(
                Paragraph::new(TextBuilder::plan_selection_hint())
                    .style(palette::muted_style(theme)),
                inner,
            );
            return;
        };

        let mut lines = plan_header(plan, theme);
        if !self.collapsed {
            lines.push(Line::default());
            lines.extend(markdown::render(&plan.content, inner.width));
        }
        let total = lines.len();
        let window = usize::from(inner.height);
        // The scroll never leaves a blank page behind.
        let highest = total.saturating_sub(window);
        self.detail_scroll = self
            .detail_scroll
            .min(u16::try_from(highest).unwrap_or(u16::MAX));
        frame.render_widget(
            Paragraph::new(ratatui::text::Text::from(lines))
                .wrap(Wrap { trim: false })
                .scroll((self.detail_scroll, 0))
                .style(palette::panel_style(theme)),
            inner,
        );
        if total > window {
            let mut state = ScrollbarState::new(highest).position(usize::from(self.detail_scroll));
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .style(palette::border_style(theme)),
                inner,
                &mut state,
            );
        }
    }
}

/// The title, the state and the revision of one plan.
fn plan_header(plan: &norte_code_wire::PlanSummary, theme: ThemeMode) -> Vec<Line<'static>> {
    let status = plan.status.label();
    let color = match plan.status {
        norte_code_wire::PlanStatus::Active => palette::success(),
        norte_code_wire::PlanStatus::Pending => palette::warning(),
        norte_code_wire::PlanStatus::Canceled => palette::danger(),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            plan.title.clone().unwrap_or_else(|| plan.id.clone()),
            Style::default()
                .fg(palette::accent(theme))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            status,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  v{}", plan.revision), palette::muted_style(theme)),
    ])];
    if let Some(model) = plan.model.as_deref() {
        lines.push(Line::from(Span::styled(
            model.to_owned(),
            palette::muted_style(theme),
        )));
    }
    lines
}

#[cfg(test)]
#[path = "plans_tests.rs"]
mod tests;
