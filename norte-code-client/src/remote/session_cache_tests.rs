//! Per-session caches stay bounded and keep active sessions.

use super::SessionCache;

#[test]
fn the_oldest_session_is_evicted_at_capacity() {
    let mut cache = SessionCache::with_capacity(2);
    cache.insert("a", 1);
    cache.insert("b", 2);
    cache.insert("c", 3);

    assert_eq!(cache.len(), 2);
    assert_eq!(cache.get("a"), None);
    assert_eq!(cache.get("b"), Some(&2));
    assert_eq!(cache.get("c"), Some(&3));
}

#[test]
fn reading_a_session_keeps_it_in_the_cache() {
    let mut cache = SessionCache::with_capacity(2);
    cache.insert("a", 1);
    cache.insert("b", 2);
    assert_eq!(cache.get_mut("a"), Some(&mut 1));
    cache.insert("c", 3);

    assert_eq!(cache.get("a"), Some(&1));
    assert_eq!(cache.get("b"), None);
}

#[test]
fn removing_a_session_drops_its_key_and_value() {
    let mut cache = SessionCache::new();
    cache.insert("a", 1);
    cache.remove("a");

    assert_eq!(cache.get("a"), None);
    assert_eq!(cache.len(), 0);
}
