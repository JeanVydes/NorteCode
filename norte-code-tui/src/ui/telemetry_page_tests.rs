//! The request table and detail are built from the report the host supplies.

use std::sync::Arc;

use norte_code_wire::{TelemetryRecord, TelemetryReport};
use terminalui::components::table::Table;

use super::TelemetryPage;
use crate::session_stats::SharedUsage;
use crate::ui::detail_panel::DetailPanel;

fn set_report(page: &mut TelemetryPage, report: TelemetryReport) {
    page.set_report(Some(Arc::new(report)), SharedUsage::default());
}

fn report() -> TelemetryReport {
    TelemetryReport {
        records: vec![
            TelemetryRecord {
                id: "req-2".to_owned(),
                model: "openrouter/model".to_owned(),
                started_at: "2026-11-19T09:00:01Z".to_owned(),
                ..TelemetryRecord::default()
            },
            TelemetryRecord {
                id: "req-1".to_owned(),
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
