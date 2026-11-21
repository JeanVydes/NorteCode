//! Per-session metadata cannot grow without a bound.

use super::BoundedMap;

#[test]
fn the_oldest_key_is_evicted_at_capacity() {
    let mut map = BoundedMap::with_capacity(2);
    map.insert("a", 1);
    map.insert("b", 2);
    map.insert("c", 3);

    assert_eq!(map.len(), 2);
    assert_eq!(map.get(&"a"), None);
    assert_eq!(map.get(&"b"), Some(&2));
    assert_eq!(map.get(&"c"), Some(&3));
}

#[test]
fn reading_a_key_refreshes_its_position() {
    let mut map = BoundedMap::with_capacity(2);
    map.insert("a", 1);
    map.insert("b", 2);
    map.insert("a", 10);
    map.insert("c", 3);

    assert_eq!(map.get(&"a"), Some(&10));
    assert_eq!(map.get(&"b"), None);
}

#[test]
fn removing_a_key_leaves_no_tombstone_in_the_order() {
    let mut map = BoundedMap::with_capacity(2);
    map.insert("a", 1);
    map.remove(&"a");
    map.insert("b", 2);

    assert_eq!(map.len(), 1);
    assert_eq!(map.get(&"b"), Some(&2));
}
