//! The request table and detail are built from the report the host supplies.

use norte_code_wire::{TelemetryRecord, TelemetryReport};
use terminalui::components::table::Table;

use super::TelemetryPage;
use crate::ui::detail_panel::DetailPanel;

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
    page.set_report(Some(report()));
    let table = page
        .children
        .get(4)
        .and_then(|node| node.view.as_any_ref().downcast_ref::<Table>())
        .expect("request table");
    assert_eq!(page.records.len(), 2);
    assert_eq!(page.records[0].id, "req-1");
    assert_eq!(table.state.selected, 0);
}

#[test]
fn the_detail_shows_the_selected_request() {
    let mut page = TelemetryPage::new();
    page.set_report(Some(report()));
    page.detail_is_request = true;
    page.sync_detail();
    let panel = page
        .children
        .get(6)
        .and_then(|node| node.view.as_any_ref().downcast_ref::<DetailPanel>())
        .expect("request detail");
    assert!(panel.fields.iter().any(|field| field.value == "req-1"));
}
