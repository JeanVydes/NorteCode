use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Tabs},
};

use super::{App, HeaderSelectState, Overlay};
use crate::{
    components::{Component, ComponentId, palette, rect},
    text_builder::TextBuilder,
};

impl App {
    pub(super) fn render(&mut self, frame: &mut Frame<'_>) {
        let frame_width = usize::from(frame.area().width);
        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(frame.area());
        let header = TextBuilder::header_layout(&self.model);
        frame.render_widget(
            Paragraph::new(header.line).style(palette::panel_style(self.model.preferences.theme)),
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
            || TextBuilder::footer_ticker(self.active_view, self.ticker_offset, frame_width),
            std::clone::Clone::clone,
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
        if let Some(select) = &self.header_select {
            Self::render_header_select(frame, select, &self.model);
        }
    }

    fn render_header_select(
        frame: &mut Frame<'_>,
        select: &HeaderSelectState,
        model: &crate::ui_model::UiModel,
    ) {
        let theme = model.preferences.theme;
        let current = select.current;
        let options = &select.options;
        let width = options
            .iter()
            .map(|option| option.chars().count())
            .max()
            .unwrap_or(8)
            .saturating_add(12)
            .min(usize::from(u16::MAX));
        let width = u16::try_from(width)
            .unwrap_or(u16::MAX)
            .min(frame.area().width);
        let height = u16::try_from(options.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .min(frame.area().height);
        let area = rect::centered_rect(width, height, frame.area());
        frame.render_widget(ratatui::widgets::Clear, area);
        let lines: Vec<Line<'static>> = options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let marker = if index == current { "·" } else { " " };
                if index == select.selected {
                    Line::from(vec![
                        Span::styled(
                            "> ",
                            Style::default()
                                .fg(palette::brand())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            option.to_string(),
                            palette::selected_style(theme).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!(" {marker}"), Style::default().fg(palette::muted())),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("  ".to_owned()),
                        Span::raw(option.to_string()),
                        Span::styled(format!(" {marker}"), Style::default().fg(palette::muted())),
                    ])
                }
            })
            .collect();
        frame.render_widget(
            Paragraph::new(ratatui::text::Text::from(lines)).block(
                Block::default()
                    .title(format!(" {} ", select.kind.title()))
                    .title_style(
                        Style::default()
                            .fg(palette::brand())
                            .add_modifier(Modifier::BOLD),
                    )
                    .style(palette::modal_style(theme))
                    .padding(ratatui::widgets::Padding::horizontal(1)),
            ),
            area,
        );
    }

    fn render_help(frame: &mut Frame<'_>, theme: crate::ui_model::ThemeMode) {
        let width = frame.area().width / 2;
        let height = u16::try_from((u32::from(frame.area().height) * 45) / 100).unwrap_or(u16::MAX);
        let area = rect::centered_rect(width, height, frame.area());
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
