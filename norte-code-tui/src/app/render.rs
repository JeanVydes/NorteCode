use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
};

use super::{App, Overlay};
use crate::{
    components::{Modal, paint_scrim, palette, spacing, toast},
    text_builder::TextBuilder,
    ui_model::ThemeMode,
};

impl App {
    pub(super) fn render(&mut self, frame: &mut Frame<'_>) {
        self.frame_area = frame.area();
        let theme = self.model.preferences.theme;
        let width = frame.area().width;
        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(TextBuilder::tab_bar(&self.model, self.active_view, width).line)
                .style(palette::panel_style(theme)),
            regions[0],
        );

        if let Some(index) = self.view_index(self.active_view)
            && let Some(component) = self.views.get_mut(index)
        {
            component.render(frame, inset(regions[1], theme), &self.model);
        }

        frame.render_widget(
            Paragraph::new(crate::text_builder::status::status_line(
                &self.model,
                self.active_view,
                width,
            ))
            .style(palette::panel_style(theme)),
            regions[2],
        );

        if let Some(notice) = &self.model.notice {
            toast::draw(
                frame,
                ratatui::layout::Rect {
                    y: regions[1].y + 1,
                    ..regions[1]
                },
                notice,
                theme,
            );
        }

        self.render_overlays(frame, theme);
    }

    /// Draws the one overlay that owns the keys, over a scrim.
    fn render_overlays(&mut self, frame: &mut Frame<'_>, theme: ThemeMode) {
        let modal = self.overlay == Overlay::Help || self.menu.is_open() || self.prompt_open();
        if self.plan_review.is_open() {
            self.plan_review.draw(frame, &self.model);
            return;
        }
        if modal {
            paint_scrim(frame, theme);
        }
        if self.overlay == Overlay::Help {
            Modal {
                title: TextBuilder::help_title(),
                controls: "Esc closes",
                body: TextBuilder::help_lines(theme),
                theme,
                selected: None,
            }
            .render(frame);
            return;
        }
        if self.menu.is_open() {
            self.menu.render(frame, theme);
            return;
        }
        let borrowed = std::mem::take(&mut self.model);
        self.render_prompt(frame, &borrowed);
        self.model = borrowed;
    }
}

/// The body area, with the screen gutter applied.
fn inset(area: ratatui::layout::Rect, theme: ThemeMode) -> ratatui::layout::Rect {
    let gutter = spacing::SCREEN_GUTTER.min(area.width / 4);
    let block = Block::default()
        .borders(Borders::NONE)
        .style(palette::transcript_style(theme))
        .padding(ratatui::widgets::Padding::horizontal(gutter));
    block.inner(area)
}
