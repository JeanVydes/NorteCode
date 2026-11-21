//! A `HashMap` with an eviction window for host metadata that grows per session.

use std::borrow::Borrow;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

const DEFAULT_CAPACITY: usize = 4096;

#[derive(Debug)]
pub(super) struct BoundedMap<K, V> {
    entries: HashMap<K, V>,
    order: VecDeque<K>,
    capacity: usize,
}

impl<K, V> Clone for BoundedMap<K, V>
where
    K: Clone,
    V: Clone,
{
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            order: self.order.clone(),
            capacity: self.capacity,
        }
    }
}

impl<K, V> Default for BoundedMap<K, V>
where
    K: Clone + Eq + Hash,
{
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl<K, V> BoundedMap<K, V>
where
    K: Clone + Eq + Hash,
{
    fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub(super) fn insert(&mut self, key: K, value: V) -> Option<V> {
        if !self.entries.contains_key(&key)
            && self.entries.len() == self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
        self.order.retain(|entry| entry != &key);
        self.order.push_back(key.clone());
        self.entries.insert(key, value)
    }

    pub(super) fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.entries.get(key)
    }

    pub(super) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.entries.contains_key(key)
    }

    #[cfg(test)]
    pub(super) fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let removed = self.entries.remove(key);
        if removed.is_some() {
            self.order.retain(|entry| entry.borrow() != key);
        }
        removed
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
#[path = "bounded_map_tests.rs"]
mod tests;
