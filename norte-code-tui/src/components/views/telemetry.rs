use crossterm::event::KeyEvent;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

use crate::{
    components::{Component, ComponentAction, ComponentId, ComponentLifecycle, ViewAction, panel},
    keys,
    text_builder::TextBuilder,
    ui_model::UiModel,
};

const PAGE_SIZE: usize = 15;

#[derive(Default)]
pub(in crate::components) struct TelemetryView {
    lifecycle: ComponentLifecycle,
    selected: usize,
    page: usize,
    pending_action: Option<ComponentAction>,
}

impl Component for TelemetryView {
    fn id(&self) -> ComponentId {
        ComponentId::Telemetry
    }

    fn lifecycle(&self) -> &ComponentLifecycle {
        &self.lifecycle
    }

    fn lifecycle_mut(&mut self) -> &mut ComponentLifecycle {
        &mut self.lifecycle
    }

    fn handle_key(&mut self, key: KeyEvent, model: &UiModel) {
        let records = model
            .telemetry
            .as_ref()
            .map_or(&[][..], |report| report.records.as_slice());
        let page_count = page_count(records.len());
        let page_start = self.page.min(page_count.saturating_sub(1)) * PAGE_SIZE;
        let page_length = records.len().saturating_sub(page_start).min(PAGE_SIZE);
        match key.code {
            keys::NAV_DOWN | keys::NAV_DOWN_ARROW => {
                panel::move_selection(&mut self.selected, page_length, true);
            }
            keys::NAV_UP | keys::NAV_UP_ARROW => {
                panel::move_selection(&mut self.selected, page_length, false);
            }
            crossterm::event::KeyCode::Char(']') => {
                self.page = self
                    .page
                    .saturating_add(1)
                    .min(page_count.saturating_sub(1));
                self.selected = 0;
            }
            crossterm::event::KeyCode::Char('[') => {
                self.page = self.page.saturating_sub(1);
                self.selected = 0;
            }
            crossterm::event::KeyCode::Char('r') => {
                self.pending_action = Some(ComponentAction::View(ViewAction::RefreshTelemetry));
            }
            crossterm::event::KeyCode::Char('t') => {
                self.pending_action = Some(ComponentAction::View(ViewAction::ToggleTelemetryScope));
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, model: &UiModel) {
        let records = model
            .telemetry
            .as_ref()
            .map_or(&[][..], |report| report.records.as_slice());
        let page_count = page_count(records.len());
        self.page = self.page.min(page_count.saturating_sub(1));
        let page_start = self.page * PAGE_SIZE;
        let visible_records = records
            .iter()
            .skip(page_start)
            .take(PAGE_SIZE)
            .collect::<Vec<_>>();
        self.selected = self.selected.min(visible_records.len().saturating_sub(1));
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        let entries: Vec<ratatui::text::Line<'static>> = visible_records
            .iter()
            .map(|record| TextBuilder::telemetry_row(record))
            .collect();
        panel::render_list(
            frame,
            columns[0],
            entries.into_iter().map(|line| vec![line]).collect(),
            model.preferences.theme,
            panel::ListPanel {
                title: ComponentId::Telemetry,
                phase: self.lifecycle.phase(),
                selected: Some(self.selected),
                focused: true,
                empty_message: TextBuilder::empty_message(ComponentId::Telemetry),
            },
        );
        let summary = model.telemetry.as_ref().map_or_else(
            || TextBuilder::telemetry_unavailable().to_owned(),
            |report| {
                format!(
                    "{} · page {}/{}\n\n{}\n\n{}",
                    TextBuilder::telemetry_scope(model.telemetry_scope),
                    self.page + 1,
                    page_count,
                    TextBuilder::telemetry_summary(&report.summary),
                    visible_records.get(self.selected).map_or_else(
                        || "Select a record for details".to_owned(),
                        |record| TextBuilder::telemetry_record_detail(record),
                    )
                )
            },
        );
        panel::render_text(
            frame,
            columns[1],
            "Telemetry details",
            summary,
            model.preferences.theme,
        );
    }

    fn take_action(&mut self) -> Option<ComponentAction> {
        self.pending_action.take()
    }
}

const fn page_count(record_count: usize) -> usize {
    let pages = record_count.saturating_add(PAGE_SIZE - 1) / PAGE_SIZE;
    if pages == 0 { 1 } else { pages }
}
