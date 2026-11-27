//! The annual heatmap paints burned tokens above the chart stack.

use std::sync::Arc;

use chrono::{Datelike, Duration, Utc};
use norte_code_wire::{TelemetryRecord, TelemetryReport};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::{Theme, Tree};

use super::{HEATMAP_ROWS, TelemetryPage};
use crate::theme::{ThemeChoice, ThemeKind};
use crate::ui::{build_tree, refresh_views, sync_views};
use crate::ui_model::{Tab, UiModel};
use crate::usage_calendar::CALENDAR_WEEKS;

fn report() -> TelemetryReport {
    let today = Utc::now().date_naive();
    let records = (0..4i64)
        .map(|index| {
            let day = today
                .checked_sub_signed(Duration::days(index))
                .unwrap_or(today);
            TelemetryRecord {
                id: format!("day-{index}"),
                started_at: format!("{day}T12:00:00Z"),
                input_tokens: u64::try_from(40 - index * 10).ok(),
                ..TelemetryRecord::default()
            }
        })
        .collect();
    TelemetryReport {
        records,
        ..TelemetryReport::default()
    }
}

fn heatmap_tree() -> (Tree, Theme) {
    let mut model = UiModel::detached();
    model.tab = Tab::Telemetry;
    model.preferences.theme = ThemeChoice::BuiltIn(ThemeKind::ByeRetina);
    model.telemetry = Some(Arc::new(report()));
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    sync_views(&mut tree, &model);
    refresh_views(&mut tree, &model);
    (tree, theme)
}

fn page(tree: &Tree) -> &TelemetryPage {
    tree.find::<TelemetryPage>("telemetry")
        .expect("telemetry page")
}

fn busiest_cell(heatmap: Rect) -> (u16, u16) {
    let today = Utc::now().date_naive();
    let weekday = u16::try_from(today.weekday().num_days_from_sunday()).unwrap_or(0);
    (
        heatmap.x + 3 + u16::try_from(CALENDAR_WEEKS - 1).unwrap_or(0),
        heatmap.y + 1 + weekday,
    )
}

#[test]
fn the_heatmap_sits_above_the_chart_stack() {
    let (mut tree, theme) = heatmap_tree();
    let area = Rect::new(0, 0, 100, 60);
    let mut buf = Buffer::empty(area);

    tree.render(area, &mut buf, &theme);

    let heatmap = page(&tree).heatmap_area();
    assert_eq!(heatmap.height, HEATMAP_ROWS);
    assert!(page(&tree).chart_area(0).y >= heatmap.y + HEATMAP_ROWS);
}

#[test]
fn the_busiest_day_uses_the_primary_color() {
    let (mut tree, theme) = heatmap_tree();
    let area = Rect::new(0, 0, 100, 60);
    let mut buf = Buffer::empty(area);

    tree.render(area, &mut buf, &theme);

    assert_eq!(page(&tree).heatmap_weeks(), CALENDAR_WEEKS);
    let (x, y) = busiest_cell(page(&tree).heatmap_area());
    assert_eq!(buf[(x, y)].fg, theme.primary);
}

#[test]
fn unused_days_use_the_secondary_surface() {
    let (mut tree, theme) = heatmap_tree();
    let area = Rect::new(0, 0, 100, 60);
    let mut buf = Buffer::empty(area);

    tree.render(area, &mut buf, &theme);

    let heatmap = page(&tree).heatmap_area();
    assert_eq!(buf[(heatmap.x + 3, heatmap.y + 1)].symbol(), "·");
}

#[test]
fn month_names_and_the_legend_share_the_label_row() {
    let (mut tree, theme) = heatmap_tree();
    let area = Rect::new(0, 0, 100, 60);
    let mut buf = Buffer::empty(area);

    tree.render(area, &mut buf, &theme);

    let heatmap = page(&tree).heatmap_area();
    let row: String = (heatmap.x..heatmap.x + heatmap.width)
        .map(|x| buf[(x, heatmap.y)].symbol())
        .collect();
    assert!(row.contains("Nov"));
    assert!(row.contains("less"));
    assert!(row.contains("more"));
}

#[test]
fn narrow_terminals_keep_only_the_recent_weeks() {
    let (mut tree, theme) = heatmap_tree();
    let area = Rect::new(0, 0, 20, 40);
    let mut buf = Buffer::empty(area);

    tree.render(area, &mut buf, &theme);

    let weeks = page(&tree).heatmap_weeks();
    assert_eq!(weeks, 17);
    let heatmap = page(&tree).heatmap_area();
    let x = heatmap.x + 3 + u16::try_from(weeks - 1).unwrap_or(0);
    let today = Utc::now().date_naive();
    let y = heatmap.y + 1 + u16::try_from(today.weekday().num_days_from_sunday()).unwrap_or(0);
    assert_eq!(buf[(x, y)].fg, theme.primary);
}
