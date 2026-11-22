use serde_json::json;

use super::first_session;
use crate::host::stores::StoredImportPayload;

#[test]
fn an_import_without_sessions_is_rejected() {
    let payload: StoredImportPayload = serde_json::from_value(json!({
        "manifest": {
            "bundle_version": "1",
            "exported_at": 0,
            "root_session_id": "root",
            "sessions": [],
            "files": [],
            "excluded": []
        },
        "history": [],
        "title": null
    }))
    .expect("payload");

    assert!(first_session(&payload).is_err());
}
