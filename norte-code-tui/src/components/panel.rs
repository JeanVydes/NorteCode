use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, List, ListItem, ListState, Paragraph},
};

use super::{ComponentId, LifecyclePhase};
use crate::{components::palette, text_builder::TextBuilder, ui_model::ThemeMode};

#[derive(Clone, Copy)]
pub(crate) struct ListPanel<'a> {
    pub(crate) title: ComponentId,
    pub(crate) phase: LifecyclePhase,
    pub(crate) selected: Option<usize>,
    pub(crate) focused: bool,
    pub(crate) empty_message: &'a str,
}

/// Draws a list of entries; every entry is one or more lines that are
/// selected and deleted as a unit.
pub(crate) fn render_list(
    frame: &mut Frame<'_>,
    area: Rect,
    entries: Vec<Vec<Line<'static>>>,
    theme: ThemeMode,
    panel: ListPanel<'_>,
) {
    let background = palette::panel_style(theme);
    let block = Block::default()
        .title(TextBuilder::panel_title(panel.title, panel.phase))
        .style(background)
        .padding(ratatui::widgets::Padding::horizontal(2))
        .title_style(if panel.focused {
            Style::default().fg(palette::accent(theme))
        } else {
            Style::default()
        });
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(panel.empty_message)
                .style(background)
                .block(block),
            area,
        );
        return;
    }

    let items = entries.into_iter().map(ListItem::new).collect::<Vec<_>>();
    let mut state = ListState::default();
    state.select(panel.selected.filter(|index| *index < items.len()));
    frame.render_stateful_widget(
        List::new(items)
            .style(background)
            .block(block)
            .highlight_symbol(TextBuilder::SELECTED_MARKER)
            .highlight_style(palette::selected_style(theme).add_modifier(Modifier::BOLD)),
        area,
        &mut state,
    );
}

pub(crate) fn render_text_scrolled(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    body: String,
    theme: ThemeMode,
    scroll: usize,
) {
    frame.render_widget(
        Paragraph::new(body)
            .block(
                Block::default()
                    .title(title)
                    .padding(ratatui::widgets::Padding::horizontal(2)),
            )
            .style(palette::panel_style(theme))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        area,
    );
}

pub(crate) fn render_text(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    body: String,
    theme: ThemeMode,
) {
    frame.render_widget(
        Paragraph::new(body)
            .block(
                Block::default()
                    .title(title)
                    .padding(ratatui::widgets::Padding::horizontal(2)),
            )
            .style(palette::panel_style(theme))
            .wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
}

pub(crate) fn move_selection(selected: &mut usize, length: usize, down: bool) {
    if length == 0 {
        *selected = 0;
    } else if down {
        *selected = selected.saturating_add(1).min(length - 1);
    } else {
        *selected = selected.saturating_sub(1);
    }
}
