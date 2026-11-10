use super::*;

/// A cell with a number and text, changed or not.
fn cell_at(number: usize, text: &str, changed: bool) -> Cell {
    Cell {
        number: Some(number),
        text: text.to_owned(),
        changed,
    }
}

#[test]
fn a_replacement_reads_as_a_removal_then_an_addition() {
    // The order is what a patch does: what was there, then what is. A
    // unified view that put the addition first would read backwards.
    let rows = vec![DiffRow {
        left: Some(cell_at(4, "old line", true)),
        right: Some(cell_at(4, "new line", true)),
    }];
    let unified = unify(&rows);
    assert_eq!(unified.len(), 2, "a changed line is two rows: {unified:?}");
    assert_eq!(unified[0].kind, UnifiedKind::Removed);
    assert_eq!(unified[0].text, "old line");
    assert_eq!(unified[0].number, Some(4), "numbered by the old file");
    assert_eq!(unified[1].kind, UnifiedKind::Added);
    assert_eq!(unified[1].text, "new line");
}

#[test]
fn an_unchanged_line_is_shown_once_and_only_one_row() {
    // Context is one line, not two: drawing it on both sides is what the
    // side-by-side view does and what this one exists not to do.
    let rows = vec![DiffRow {
        left: Some(cell_at(3, "same", false)),
        right: Some(cell_at(3, "same", false)),
    }];
    let unified = unify(&rows);
    assert_eq!(unified.len(), 1, "{unified:?}");
    assert_eq!(unified[0].kind, UnifiedKind::Context);
    assert_eq!(unified[0].text, "same");
}

#[test]
fn a_side_that_is_missing_is_an_insertion_or_a_deletion() {
    // A pure deletion has no right cell and a pure insertion has no left
    // one; neither is a change of a line, and both must still be shown.
    let rows = vec![
        DiffRow {
            left: Some(cell_at(7, "gone", true)),
            right: None,
        },
        DiffRow {
            left: None,
            right: Some(cell_at(7, "arrived", true)),
        },
    ];
    let unified = unify(&rows);
    assert_eq!(unified.len(), 2, "{unified:?}");
    assert_eq!(unified[0].kind, UnifiedKind::Removed);
    assert_eq!(unified[0].text, "gone");
    assert_eq!(unified[1].kind, UnifiedKind::Added);
    assert_eq!(unified[1].text, "arrived");
}

#[test]
fn the_marks_are_the_ones_a_patch_uses() {
    // A diff a person has read before should need no explanation, so the
    // marks are the two every tool already uses.
    assert_eq!(UnifiedKind::Context.mark(), ' ');
    assert_eq!(UnifiedKind::Removed.mark(), '-');
    assert_eq!(UnifiedKind::Added.mark(), '+');
}

#[test]
fn a_multiline_replacement_is_one_block_of_removals_then_additions() {
    // A patch hunk draws the old block first and the new block after it.
    // Interleaving them row by row is what made the unified view read as
    // a jagged alternation through a multi-line edit.
    let rows = vec![
        DiffRow {
            left: Some(cell_at(1, "old a", true)),
            right: Some(cell_at(1, "new a", true)),
        },
        DiffRow {
            left: Some(cell_at(2, "old b", true)),
            right: Some(cell_at(2, "new b", true)),
        },
        DiffRow {
            left: Some(cell_at(3, "same", false)),
            right: Some(cell_at(3, "same", false)),
        },
    ];
    let unified = unify(&rows);
    let kinds: Vec<UnifiedKind> = unified.iter().map(|row| row.kind).collect();
    assert_eq!(
        kinds,
        [
            UnifiedKind::Removed,
            UnifiedKind::Removed,
            UnifiedKind::Added,
            UnifiedKind::Added,
            UnifiedKind::Context
        ],
        "the old block, then the new, then the context: {unified:?}"
    );
}

#[test]
fn an_empty_box_unifies_to_nothing() {
    // A box with no rows is not one row of blank: the call is what draws the
    // borders, and this must not invent a line for it.
    assert_eq!(unify(&[]).len(), 0);
}

#[test]
fn the_range_says_what_both_views_say() {
    let boxed = |old: Option<(usize, usize)>, new: Option<(usize, usize)>| EditBox {
        index: 1,
        total: 1,
        old_range: old,
        new_range: new,
        approximate: false,
        rows: Vec::new(),
        omitted: 0,
    };
    // A line replaced by another line: the old range and the new one.
    assert!(edit_range(&boxed(Some((3, 3)), Some((3, 3)))).contains('3'));
    // A deletion: there is no new range.
    let deleted = edit_range(&boxed(Some((3, 5)), None));
    assert!(deleted.contains("deleted"), "{deleted}");
    // An insertion has no old range, which is what "unknown" covers.
    assert_eq!(edit_range(&boxed(None, None)), "location unknown");
}
