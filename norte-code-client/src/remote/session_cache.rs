//! Bounded per-session caches for the remote client.

use std::collections::{HashMap, VecDeque};

const DEFAULT_CAPACITY: usize = 64;

/// A small LRU map keyed by session id.
#[derive(Debug)]
pub(crate) struct SessionCache<T> {
    entries: HashMap<String, T>,
    order: VecDeque<String>,
    capacity: usize,
}

impl<T> SessionCache<T> {
    pub(crate) fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub(crate) fn get(&self, session: &str) -> Option<&T> {
        self.entries.get(session)
    }

    pub(crate) fn get_mut(&mut self, session: &str) -> Option<&mut T> {
        if !self.entries.contains_key(session) {
            return None;
        }
        self.touch(session);
        self.entries.get_mut(session)
    }

    pub(crate) fn insert(&mut self, session: impl Into<String>, value: T) {
        let session = session.into();
        if self.entries.contains_key(&session) {
            self.touch(&session);
        } else {
            self.order.push_back(session.clone());
        }
        self.entries.insert(session, value);
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.remove(&oldest);
        }
    }

    pub(crate) fn remove(&mut self, session: &str) {
        self.entries.remove(session);
        self.order.retain(|key| key != session);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    fn touch(&mut self, session: &str) {
        self.order.retain(|key| key != session);
        self.order.push_back(session.to_owned());
    }
}

impl<T> Default for SessionCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "session_cache_tests.rs"]
mod tests;
