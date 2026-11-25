//! The request table and detail are built from the report the host supplies.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use norte_code_wire::{TelemetryRecord, TelemetryReport};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::components::table::Table;
use terminalui::{InputEvent, Theme, Tree};

use super::TelemetryPage;
use super::telemetry_page::MAX_RECORD_WINDOW;
use crate::leaderboard::{LeaderboardGroup, LeaderboardSubject};
use crate::session_stats::SharedUsage;
use crate::theme::{ThemeChoice, ThemeKind};
use crate::ui::detail_panel::DetailPanel;
use crate::ui::{build_tree, refresh_views, sync_views};
use crate::ui_model::{Tab, UiModel};

fn set_report(page: &mut TelemetryPage, report: TelemetryReport) {
    page.set_report(Some(Arc::new(report)), SharedUsage::default());
}

fn report() -> TelemetryReport {
    TelemetryReport {
        records: vec![
            TelemetryRecord {
                id: "req-2".to_owned(),
                session: "8f4b12c7-3ee4-4c82-a090-123456789abc".to_owned(),
                model: "openrouter/model".to_owned(),
                started_at: "2026-11-19T09:00:01Z".to_owned(),
                ..TelemetryRecord::default()
            },
            TelemetryRecord {
                id: "req-1".to_owned(),
                session: "8f4b12c7-3ee4-4c82-a090-123456789abc".to_owned(),
                model: "nvidia/model".to_owned(),
                started_at: "2026-11-19T10:00:01Z".to_owned(),
                input_tokens: Some(100),
                output_tokens: Some(20),
                ..TelemetryRecord::default()
            },
        ],
        ..TelemetryReport::default()
    }
}

#[test]
fn the_table_lists_the_latest_requests_first() {
    let mut page = TelemetryPage::new();
    set_report(&mut page, report());
    let table = page
        .children
        .get(4)
        .and_then(|node| node.view.as_any_ref().downcast_ref::<Table>())
        .expect("request table");
    assert_eq!(page.record_indices, vec![1, 0]);
    assert_eq!(table.state.selected, 0);
}

#[test]
fn the_report_is_shared_instead_of_copied() {
    let report = Arc::new(report());
    let mut page = TelemetryPage::new();

    page.set_report(Some(Arc::clone(&report)), SharedUsage::default());

    assert_eq!(Arc::strong_count(&report), 2);
}

#[test]
fn setting_the_same_report_does_not_rebuild_the_charts() {
    let shared = Arc::new(report());
    let usage = SharedUsage::default();
    let mut page = TelemetryPage::new();
    page.set_report(Some(Arc::clone(&shared)), Arc::clone(&usage));
    page.set_chart_scroll_back(1);

    page.set_report(Some(Arc::clone(&shared)), Arc::clone(&usage));

    assert_eq!(page.chart_scroll_back(), 1);
}

#[test]
fn the_request_lists_use_a_bounded_record_window() {
    let total = MAX_RECORD_WINDOW + 10;
    let records = (0..total)
        .map(|index| TelemetryRecord {
            id: format!("req-{index}"),
            session: "session".to_owned(),
            ..TelemetryRecord::default()
        })
        .collect();
    let mut page = TelemetryPage::new();
    set_report(
        &mut page,
        TelemetryReport {
            records,
            ..TelemetryReport::default()
        },
    );

    assert_eq!(page.record_indices.len(), MAX_RECORD_WINDOW);
    assert_eq!(page.record_indices.first(), Some(&(total - 1)));
    assert_eq!(page.record_indices.last(), Some(&10));
}

#[test]
fn the_detail_shows_the_selected_request() {
    let mut page = TelemetryPage::new();
    set_report(&mut page, report());
    page.detail_is_request = true;
    page.sync_detail();
    let panel = page
        .children
        .get(6)
        .and_then(|node| node.view.as_any_ref().downcast_ref::<DetailPanel>())
        .expect("request detail");
    assert!(panel.fields.iter().any(|field| field.value == "req-1"));
}

fn leaderboard_tree() -> (Tree, UiModel, Theme) {
    let mut model = UiModel::detached();
    model.tab = Tab::Telemetry;
    model.telemetry = Some(Arc::new(report()));
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    sync_views(&mut tree, &model);
    refresh_views(&mut tree, &model);
    if let Some(page) = tree.find_mut::<TelemetryPage>("telemetry") {
        for _ in 0..3 {
            page.toggle_focus();
        }
    }
    (tree, model, theme)
}

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn tab_cycles_charts_sessions_requests_and_rankings() {
    let (mut tree, _, _) = chart_tree();
    assert_eq!(
        page(&tree).view,
        super::telemetry_page::TelemetryView::Charts
    );

    if let Some(page) = tree.find_mut::<TelemetryPage>("telemetry") {
        page.toggle_focus();
    }
    assert_eq!(
        page(&tree).view,
        super::telemetry_page::TelemetryView::Sessions
    );

    if let Some(page) = tree.find_mut::<TelemetryPage>("telemetry") {
        page.toggle_focus();
    }
    assert_eq!(
        page(&tree).view,
        super::telemetry_page::TelemetryView::Requests
    );

    if let Some(page) = tree.find_mut::<TelemetryPage>("telemetry") {
        page.toggle_focus();
    }
    assert_eq!(
        page(&tree).view,
        super::telemetry_page::TelemetryView::Leaderboards
    );
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn click(column: u16, row: u16) -> InputEvent {
    mouse(MouseEventKind::Down(MouseButton::Left), column, row)
}

fn wheel(column: u16, row: u16, modifiers: KeyModifiers) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column,
        row,
        modifiers,
    })
}

fn chart_tree() -> (Tree, UiModel, Theme) {
    let mut model = UiModel::detached();
    model.tab = Tab::Telemetry;
    model.preferences.theme = ThemeChoice::BuiltIn(ThemeKind::ByeRetina);
    model.telemetry = Some(Arc::new(report()));
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let mut tree = build_tree(&model);
    sync_views(&mut tree, &model);
    refresh_views(&mut tree, &model);
    (tree, model, theme)
}

fn render_charts(tree: &mut Tree, theme: &Theme, height: u16) -> Rect {
    let area = Rect::new(0, 0, 80, height);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, theme);
    area
}

#[test]
fn a_zero_height_session_view_does_not_underflow_its_lower_panel() {
    let (mut tree, _, theme) = chart_tree();
    if let Some(page) = tree.find_mut::<TelemetryPage>("telemetry") {
        page.toggle_focus();
    }
    let area = Rect::new(0, 0, 100, 0);
    let mut buf = Buffer::empty(area);

    tree.render(area, &mut buf, &theme);
}

#[test]
fn sessions_view_uses_a_short_id_and_shows_request_count() {
    let (mut tree, _, theme) = chart_tree();
    if let Some(page) = tree.find_mut::<TelemetryPage>("telemetry") {
        page.toggle_focus();
    }
    tree.tick(Duration::from_millis(200));
    let area = Rect::new(0, 0, 100, 30);
    let mut buffer = Buffer::empty(area);

    tree.render(area, &mut buffer, &theme);

    let rendered = buffer
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("Session 8f4b12c7"));
    assert!(rendered.contains("2 requests"));
}

#[test]
fn requests_view_renders_rows_and_request_details_in_available_height() {
    let (mut tree, _, theme) = chart_tree();
    if let Some(page) = tree.find_mut::<TelemetryPage>("telemetry") {
        page.toggle_focus();
        page.toggle_focus();
    }
    tree.tick(Duration::from_millis(200));
    let area = Rect::new(0, 0, 120, 30);
    let mut buffer = Buffer::empty(area);

    tree.render(area, &mut buffer, &theme);

    let rendered = buffer
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("nvidia/model"));
    assert!(rendered.contains("req-1"));
}

fn page(tree: &Tree) -> &TelemetryPage {
    tree.find::<TelemetryPage>("telemetry")
        .expect("telemetry page")
}

fn group(tree: &Tree) -> LeaderboardGroup {
    tree.find::<TelemetryPage>("telemetry")
        .expect("telemetry page")
        .leaderboard_group()
}

fn subject(tree: &Tree) -> LeaderboardSubject {
    tree.find::<TelemetryPage>("telemetry")
        .expect("telemetry page")
        .leaderboard_subject()
}

#[test]
fn up_and_down_switch_the_group_tabs() {
    let (mut tree, _, _) = leaderboard_tree();
    assert_eq!(group(&tree), LeaderboardGroup::Usage);

    tree.handle_event(&key(KeyCode::Up));
    assert_eq!(group(&tree), LeaderboardGroup::CacheRate);

    tree.handle_event(&key(KeyCode::Down));
    assert_eq!(group(&tree), LeaderboardGroup::Usage);

    tree.handle_event(&key(KeyCode::Char('2')));
    assert_eq!(group(&tree), LeaderboardGroup::Input);
}

#[test]
fn clicks_select_group_and_subject_tabs() {
    let (mut tree, _, theme) = leaderboard_tree();
    let area = Rect::new(0, 0, 80, 30);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let board = tree
        .find::<TelemetryPage>("telemetry")
        .expect("telemetry page")
        .leaderboard_area();

    tree.handle_event(&click(board.x + 19, board.y + 1));
    assert_eq!(group(&tree), LeaderboardGroup::Cost);

    tree.handle_event(&click(board.x + 12, board.y + 2));
    assert_eq!(subject(&tree), LeaderboardSubject::Provider);
}

#[test]
fn clicking_a_row_keeps_the_selected_ranking() {
    let (mut tree, _, theme) = leaderboard_tree();
    let area = Rect::new(0, 0, 100, 30);
    let mut buf = Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let board = tree
        .find::<TelemetryPage>("telemetry")
        .expect("telemetry page")
        .leaderboard_area();

    tree.handle_event(&click(board.x + 4, board.y + 7));

    let selected = tree
        .find::<TelemetryPage>("telemetry")
        .expect("telemetry page")
        .leaderboard_selected();
    assert_eq!(selected, 1);
}

#[test]
fn wheel_inside_the_cost_chart_scrolls_its_history_only() {
    let (mut tree, _, theme) = chart_tree();
    render_charts(&mut tree, &theme, 50);
    let area = page(&tree).chart_area(0);
    assert!(area.width > 0 && area.height > 0);

    tree.handle_event(&wheel(area.x + 20, area.y + 5, KeyModifiers::NONE));

    assert!(page(&tree).chart_scroll_back() > 0);
    assert_eq!(page(&tree).chart_scroll(), 0);
}

#[test]
fn wheel_inside_a_line_chart_scrolls_the_shared_history_only() {
    let (mut tree, _, theme) = chart_tree();
    render_charts(&mut tree, &theme, 50);
    let area = page(&tree).chart_area(1);
    assert!(area.width > 0 && area.height > 0);

    tree.handle_event(&wheel(area.x + 20, area.y + 5, KeyModifiers::NONE));

    assert!(page(&tree).chart_scroll_back() > 0);
    assert_eq!(page(&tree).chart_scroll(), 0);
}

#[test]
fn shift_wheel_over_a_chart_still_scrolls_its_history() {
    let (mut tree, _, theme) = chart_tree();
    render_charts(&mut tree, &theme, 50);
    let area = page(&tree).chart_area(0);

    tree.handle_event(&wheel(area.x + 20, area.y + 5, KeyModifiers::SHIFT));

    assert!(page(&tree).chart_scroll_back() > 0);
    assert_eq!(page(&tree).chart_scroll(), 0);
}

#[test]
fn wheel_outside_the_charts_does_not_scroll_the_stack() {
    let (mut tree, _, theme) = chart_tree();
    render_charts(&mut tree, &theme, 50);

    tree.handle_event(&wheel(20, 1, KeyModifiers::NONE));

    assert_eq!(page(&tree).chart_scroll_back(), 0);
    assert_eq!(page(&tree).chart_scroll(), 0);
}

#[test]
fn arrows_scroll_the_stacked_charts() {
    let (mut tree, _, theme) = chart_tree();
    render_charts(&mut tree, &theme, 50);
    assert_eq!(page(&tree).chart_scroll(), 0);

    tree.handle_event(&key(KeyCode::Down));
    assert_eq!(page(&tree).chart_scroll(), 3);

    tree.handle_event(&key(KeyCode::Up));
    assert_eq!(page(&tree).chart_scroll(), 0);
}

#[test]
fn clicking_the_chart_scrollbar_pages_the_stack() {
    let (mut tree, _, theme) = chart_tree();
    render_charts(&mut tree, &theme, 30);
    let rail = page(&tree).chart_rail().expect("stack scrollbar");

    tree.handle_event(&click(rail.x, rail.y + rail.height - 1));

    assert!(page(&tree).chart_scroll() > 0);
    assert_eq!(page(&tree).chart_scroll_back(), 0);
}

#[test]
fn dragging_the_chart_scrollbar_thumb_moves_the_stack() {
    let (mut tree, _, theme) = chart_tree();
    render_charts(&mut tree, &theme, 30);
    let rail = page(&tree).chart_rail().expect("stack scrollbar");

    tree.handle_event(&mouse(
        MouseEventKind::Down(MouseButton::Left),
        rail.x,
        rail.y + rail.top + 1,
    ));
    tree.handle_event(&mouse(
        MouseEventKind::Drag(MouseButton::Left),
        rail.x,
        rail.y + rail.height - 1,
    ));
    assert!(page(&tree).chart_scroll() > 0);

    tree.handle_event(&mouse(
        MouseEventKind::Up(MouseButton::Left),
        rail.x,
        rail.y + rail.height - 1,
    ));
    let settled = page(&tree).chart_scroll();
    tree.handle_event(&mouse(
        MouseEventKind::Drag(MouseButton::Left),
        rail.x,
        rail.y,
    ));
    assert_eq!(page(&tree).chart_scroll(), settled);
}

#[test]
fn the_stacked_charts_draw_a_scrollbar_when_they_overflow() {
    let (mut tree, _, theme) = chart_tree();
    let area = Rect::new(0, 0, 80, 30);
    let mut buf = Buffer::empty(area);

    tree.render(area, &mut buf, &theme);

    let rail = page(&tree).chart_rail().expect("stack scrollbar");
    assert_eq!(buf[(rail.x, rail.y + rail.top)].symbol(), "█");
}

#[test]
fn chart_gaps_use_the_selected_theme_background() {
    let (mut tree, _, theme) = chart_tree();
    let area = Rect::new(0, 0, 100, 50);
    let mut buf = Buffer::empty(area);

    tree.render(area, &mut buf, &theme);

    let first = page(&tree).chart_area(0);
    assert_eq!(
        buf[(first.x + 10, first.y + first.height)].bg,
        theme.background
    );
}

#[test]
fn the_chart_composition_buffer_is_reused() {
    let (mut tree, _, theme) = chart_tree();
    render_charts(&mut tree, &theme, 50);
    let capacity = page(&tree).chart_page_capacity();
    assert!(capacity > 0);

    render_charts(&mut tree, &theme, 50);

    assert_eq!(page(&tree).chart_page_capacity(), capacity);
}

#[test]
fn both_line_charts_draw_the_time_axis() {
    let (mut tree, _, theme) = chart_tree();
    let area = render_charts(&mut tree, &theme, 70);
    let painted = crate::test_support::rendered_in(&mut tree, area, &theme);

    assert!(painted.match_indices("09:00").count() >= 3);
}
