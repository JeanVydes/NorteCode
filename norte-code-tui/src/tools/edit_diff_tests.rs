use super::{DiffRow, DiffRows, EditDiffComponent};

#[test]
fn shows_replaced_lines_and_context() {
    let rows = DiffRows::new("one\ntwo\nold\nfour\nfive", "one\ntwo\nnew\nfour\nfive")
        .expect("small diffs are rendered");
    assert_eq!(
        rows,
        DiffRows(vec![
            DiffRow::Context("one".to_owned()),
            DiffRow::Context("two".to_owned()),
            DiffRow::Change("old".to_owned(), "new".to_owned()),
            DiffRow::Context("four".to_owned()),
            DiffRow::Context("five".to_owned()),
        ])
    );
}

#[test]
fn aligns_insertions_and_deletions() {
    let inserted = DiffRows::new("old", "new\nextra").expect("small diffs are rendered");
    assert_eq!(
        inserted,
        DiffRows(vec![
            DiffRow::Change("old".to_owned(), "new".to_owned()),
            DiffRow::Added("extra".to_owned()),
        ])
    );

    let deleted = DiffRows::new("old\nextra", "new").expect("small diffs are rendered");
    assert_eq!(
        deleted,
        DiffRows(vec![
            DiffRow::Change("old".to_owned(), "new".to_owned()),
            DiffRow::Removed("extra".to_owned()),
        ])
    );
}

#[test]
fn keeps_unchanged_lines_between_separate_edits_as_context() {
    let rows = DiffRows::new(
        "before\nkeep\nold\nmiddle\nremove\nend",
        "before\nkeep\nnew\nmiddle\nadd\nend",
    )
    .expect("small diffs are rendered");

    assert_eq!(
        rows,
        DiffRows(vec![
            DiffRow::Context("before".to_owned()),
            DiffRow::Context("keep".to_owned()),
            DiffRow::Change("old".to_owned(), "new".to_owned()),
            DiffRow::Context("middle".to_owned()),
            DiffRow::Change("remove".to_owned(), "add".to_owned()),
            DiffRow::Context("end".to_owned()),
        ])
    );
}

#[test]
fn applies_ordered_edit_specs_to_the_captured_source() {
    let arguments = serde_json::json!({
        "edits": [
            {"search": "one", "replace": "ONE"},
            {"search": "three", "replace": "THREE"}
        ]
    });

    assert_eq!(
        EditDiffComponent::apply_edits("one two three", Some(&arguments)),
        Some("ONE two THREE".to_owned())
    );
}

#[test]
fn refuses_to_guess_when_an_edit_target_is_ambiguous() {
    let arguments = serde_json::json!({
        "edits": [{"search": "same", "replace": "changed"}]
    });

    assert_eq!(
        EditDiffComponent::apply_edits("same and same", Some(&arguments)),
        None
    );
}

#[test]
fn honors_the_requested_occurrence_for_repeated_edit_targets() {
    let arguments = serde_json::json!({
        "edits": [{"search": "same", "replace": "changed", "occurrence": 2}]
    });

    assert_eq!(
        EditDiffComponent::apply_edits("same and same", Some(&arguments)),
        Some("same and changed".to_owned())
    );
}
