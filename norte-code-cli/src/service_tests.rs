//! Service startup settings stay bounded and predictable.

use super::worker_count_from;

#[test]
fn worker_count_defaults_to_four_and_never_exceeds_the_machine() {
    assert_eq!(worker_count_from(None, 12), 4);
    assert_eq!(worker_count_from(None, 2), 2);
}

#[test]
fn worker_count_honors_a_valid_override() {
    assert_eq!(worker_count_from(Some(" 3 "), 12), 3);
    assert_eq!(worker_count_from(Some("32"), 12), 12);
}

#[test]
fn worker_count_ignores_invalid_overrides() {
    assert_eq!(worker_count_from(Some("many"), 12), 4);
    assert_eq!(worker_count_from(Some("0"), 12), 1);
}
