//! The SSE replay guard forgets old ids instead of growing forever.

use super::{DEFAULT_CAPACITY, SeenFrames};

#[test]
fn duplicate_ids_are_rejected_while_they_are_retained() {
    let mut seen = SeenFrames::from_initial(["a", "b"]);

    assert!(!seen.insert("a"));
    assert!(seen.insert("c"));
    assert!(!seen.insert("c"));
}

#[test]
fn an_evicted_id_can_be_accepted_again() {
    let mut seen = SeenFrames::with_capacity(2);
    seen.insert("a");
    seen.insert("b");
    seen.insert("c");

    assert!(!seen.insert("c"));
    assert!(seen.insert("a"));
    assert!(seen.insert("b"));
}

#[test]
fn the_default_capacity_is_bounded() {
    let mut seen = SeenFrames::from_initial(0..DEFAULT_CAPACITY + 10);

    assert!(seen.insert(DEFAULT_CAPACITY + 10));
    assert!(!seen.insert(DEFAULT_CAPACITY + 9));
    assert!(seen.insert(10));
}
