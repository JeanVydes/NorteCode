//! Edit diffs: context from the source, both layouts.

use super::{Diff, DiffMode, EditArgs, LineKind};

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
    let diff = Diff::from_call(None, &arguments).expect("diff");
    assert_eq!(diff.lines[0].text, "one");
    assert_eq!(diff.lines[1].text, "two");
    assert_eq!(diff.lines.last().expect("line").text, "four");
}

#[test]
fn a_direct_pair_becomes_a_diff() {
    let arguments = args(serde_json::json!({ "old_string": "before", "new_string": "after" }));
    let diff = Diff::from_call(None, &arguments).expect("diff");
    assert_eq!(diff.lines[0].kind, LineKind::Removed);
    assert_eq!(diff.lines[0].text, "before");
    assert_eq!(diff.lines[1].kind, LineKind::Added);
    assert_eq!(diff.lines[1].text, "after");
}

#[test]
fn source_adds_surrounding_context() {
    let source = "first\nsecond\nold line\nfourth\nfifth";
    let arguments = args(serde_json::json!({
        "edits": [{ "search": "old line", "replace": "new line" }]
    }));
    let diff = Diff::from_call(Some(source), &arguments).expect("diff");
    let text: Vec<&str> = diff.lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(
        text,
        vec!["first", "second", "old line", "new line", "fourth", "fifth"]
    );
    assert_eq!(diff.lines[0].kind, LineKind::Context);
    assert_eq!(diff.lines[2].kind, LineKind::Removed);
    assert_eq!(diff.lines[3].kind, LineKind::Added);
    assert_eq!(diff.lines[5].kind, LineKind::Context);
}

#[test]
fn a_very_large_edit_is_truncated() {
    let search = "old\n".repeat(500);
    let replace = "new\n".repeat(500);
    let arguments = args(serde_json::json!({
        "old_string": search,
        "new_string": replace
    }));

    let diff = Diff::from_call(None, &arguments).expect("diff");

    assert_eq!(diff.lines.len(), 400);
    assert_eq!(
        diff.lines.last().expect("marker").text,
        "... diff truncated"
    );
}

#[test]
fn no_edit_payload_means_no_diff() {
    let arguments = args(serde_json::json!({ "path": "x" }));
    assert_eq!(Diff::from_call(None, &arguments), None);
}

#[test]
fn the_layout_changes_the_row_count() {
    let diff = Diff {
        lines: vec![
            super::Line {
                kind: LineKind::Context,
                text: "ctx".into(),
            },
            super::Line {
                kind: LineKind::Removed,
                text: "a".into(),
            },
            super::Line {
                kind: LineKind::Removed,
                text: "b".into(),
            },
            super::Line {
                kind: LineKind::Added,
                text: "c".into(),
            },
        ],
    };
    assert_eq!(diff.rows(DiffMode::Unified), 4);
    assert_eq!(diff.rows(DiffMode::Split), 3);
}

#[test]
fn diff_labels_round_trip_through_the_enum() {
    for mode in [DiffMode::Unified, DiffMode::Split] {
        assert_eq!(DiffMode::from_label(mode.label()), Some(mode));
    }
    assert_eq!(DiffMode::from_label("side-by-side"), None);
}
