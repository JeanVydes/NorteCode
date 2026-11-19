//! Edit diffs: both argument shapes, both layouts.

use super::{Diff, DiffMode, EditArgs};

fn args(json: serde_json::Value) -> EditArgs {
    serde_json::from_value(json).expect("edit args")
}

#[test]
fn an_edits_array_becomes_a_diff() {
    let arguments = args(serde_json::json!({
        "edits": [
            { "search": "one\ntwo", "replace": "ONE" },
            { "search": "three", "replace": "THREE\nfour" }
        ]
    }));
    let diff = Diff::from_edit(&arguments).expect("diff");
    assert_eq!(diff.removed, vec!["one", "two", "three"]);
    assert_eq!(diff.added, vec!["ONE", "THREE", "four"]);
}

#[test]
fn a_direct_pair_becomes_a_diff() {
    let arguments = args(serde_json::json!({ "old_string": "before", "new_string": "after" }));
    let diff = Diff::from_edit(&arguments).expect("diff");
    assert_eq!(diff.removed, vec!["before"]);
    assert_eq!(diff.added, vec!["after"]);
}

#[test]
fn no_edit_payload_means_no_diff() {
    let arguments = args(serde_json::json!({ "path": "x" }));
    assert_eq!(Diff::from_edit(&arguments), None);
}

#[test]
fn the_layout_changes_the_row_count() {
    let diff = Diff {
        removed: vec!["a".into(), "b".into()],
        added: vec!["c".into()],
    };
    assert_eq!(diff.rows(DiffMode::Unified), 4);
    assert_eq!(diff.rows(DiffMode::Split), 3);
}
