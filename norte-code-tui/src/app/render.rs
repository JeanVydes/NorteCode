use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph, Tabs},
};

use super::{App, Overlay};
use crate::{
    components::{Component, ComponentId, palette},
    text_builder::TextBuilder,
};

impl App {
    pub(super) fn render(&mut self, frame: &mut Frame<'_>) {
        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(frame.area());
        frame.render_widget(
            Paragraph::new(TextBuilder::header(&self.model))
                .style(palette::panel_style(self.model.preferences.theme)),
            regions[0],
        );
        let tabs = ComponentId::all()
            .into_iter()
            .map(TextBuilder::tab_label)
            .collect::<Vec<_>>();
        let selected = ComponentId::all()
            .iter()
            .position(|view| *view == self.active_view)
            .unwrap_or_default();
        frame.render_widget(
            Tabs::new(tabs)
                .select(selected)
                .block(Block::default().borders(Borders::BOTTOM))
                .style(palette::panel_style(self.model.preferences.theme))
                .highlight_style(
                    Style::default()
                        .fg(palette::accent(self.model.preferences.theme))
                        .add_modifier(Modifier::BOLD),
                ),
            regions[1],
        );
        if let Some(index) = self.view_index(self.active_view)
            && let Some(component) = self.views.get_mut(index)
        {
            component.render(frame, regions[2], &self.model);
        }
        let footer = self.model.notice.as_ref().map_or_else(
            || TextBuilder::footer_help(self.active_view),
            |notice| format!("{notice} · {}", TextBuilder::footer_help(self.active_view)),
        );
        let footer_color = if self.model.notice.is_some() {
            palette::danger()
        } else {
            palette::muted()
        };
        frame.render_widget(
            Paragraph::new(footer)
                .style(palette::panel_style(self.model.preferences.theme).fg(footer_color)),
            regions[3],
        );
        if self.overlay == Overlay::Help {
            Self::render_help(frame, self.model.preferences.theme);
        }
        if self.plan_review.is_open() {
            self.plan_review.render(frame, frame.area(), &self.model);
        }
    }

    fn render_help(frame: &mut Frame<'_>, theme: crate::ui_model::ThemeMode) {
        let area = centered_rect(50, 45, frame.area());
        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(palette::modal_style(theme)), area);
        frame.render_widget(
            Paragraph::new(TextBuilder::help_lines())
                .block(Block::default().title(TextBuilder::help_title()))
                .style(palette::modal_style(theme)),
            area,
        );
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height) / 2),
            Constraint::Percentage(height),
            Constraint::Percentage((100 - height) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width) / 2),
            Constraint::Percentage(width),
            Constraint::Percentage((100 - width) / 2),
        ])
        .split(vertical[1])[1]
}
