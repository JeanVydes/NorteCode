use super::{DiffRow, DiffRows, EditDiffComponent, cut, numbered};
use crate::tools::code_box::CodeRow;

fn lines(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

#[allow(clippy::needless_pass_by_value)]
fn arguments(path: &str, edits: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"path": path, "edits": edits})
}

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

#[test]
fn a_box_keeps_only_the_change_and_the_lines_around_it() {
    let source = (1..=40)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let arguments = arguments(
        "src/lib.rs",
        serde_json::json!([
            {"search": "line 30", "replace": "line thirty"}
        ]),
    );

    let drawn = lines(&EditDiffComponent::render(
        Some(&source),
        Some(&arguments),
        super::DiffMode::Unified,
        80,
        crate::ui_model::ThemeMode::Dark,
    ));
    let body = drawn.join("\n");

    assert!(!body.contains("line 1\n"), "the far lines are gone");
    assert!(
        body.contains("line 27"),
        "the line before the change is kept"
    );
    assert!(body.contains("line thirty"), "the change is drawn");
    assert!(body.contains("⋯"), "the cut is marked");
    assert!(
        body.contains("30 │ - line 30"),
        "the removed line keeps its file number: {body}"
    );
    assert!(
        body.contains("30 │ + line thirty"),
        "the added line carries the same number: {body}"
    );
}

#[test]
fn a_box_names_the_file_and_never_says_unified() {
    let arguments = arguments(
        "src/lib.rs",
        serde_json::json!([
            {"search": "one", "replace": "ONE"}
        ]),
    );

    let drawn = lines(&EditDiffComponent::render(
        Some("one"),
        Some(&arguments),
        super::DiffMode::Unified,
        80,
        crate::ui_model::ThemeMode::Dark,
    ));
    let body = drawn.join("\n");

    assert!(drawn[0].starts_with("src/lib.rs"), "the title is the file");
    assert!(!body.contains("unified"), "the mode name is noise");
    assert!(!body.contains('┌') && !body.contains('└'), "no frames");
}

#[test]
fn a_call_replayed_without_a_source_still_draws_the_edit() {
    let arguments = arguments(
        "src/lib.rs",
        serde_json::json!([
            {"search": "old text", "replace": "new text"}
        ]),
    );

    let drawn = lines(&EditDiffComponent::render(
        None,
        Some(&arguments),
        super::DiffMode::Unified,
        80,
        crate::ui_model::ThemeMode::Dark,
    ));
    let body = drawn.join("\n");

    assert!(body.contains("- old text"), "the search is drawn");
    assert!(body.contains("+ new text"), "the replacement is drawn");
    assert!(!body.contains("edit diff unavailable"));
}

#[test]
fn numbering_counts_the_file_lines_around_a_change() {
    let rows = vec![
        DiffRow::Context("one".to_owned()),
        DiffRow::Removed("gone".to_owned()),
        DiffRow::Added("new".to_owned()),
    ];

    assert_eq!(numbered(&rows), vec![Some(1), Some(2), Some(2)]);
}

#[test]
fn a_box_cuts_the_far_lines_and_marks_the_jump() {
    let rows = (1..=20)
        .map(|line| CodeRow::context(Some(line), format!("line {line}")))
        .collect::<Vec<_>>();
    let mut with_change = rows.clone();
    with_change[10] = CodeRow::removed(Some(11), "line 11");

    let drawn = cut(&with_change);

    assert_eq!(
        drawn.len(),
        8,
        "three lines before and after, and a gap row"
    );
    assert!(matches!(drawn[0], CodeRow::Gap), "the cut is marked");
    assert!(drawn.iter().any(|row| row.text() == "line 11"));
    assert!(!drawn.iter().any(|row| row.text() == "line 1"));
    assert!(!drawn.iter().any(|row| row.text() == "line 20"));
}

#[test]
fn code_rows_carry_their_kind() {
    assert_eq!(
        CodeRow::removed(Some(4), "gone"),
        CodeRow::Removed {
            number: Some(4),
            text: "gone".to_owned()
        }
    );
}
